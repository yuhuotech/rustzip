//! Blocking HTTP for the two things RustZIP sends over the network.
//!
//! Both run on a background thread and neither may stall startup, so every request has
//! a timeout. Redirects are followed because GitHub serves release files from another
//! host; they must stay on https.

use rz_app::stats::{Report, Sender};
use rz_app::update::Fetcher;
use rz_core::{Error, Result};
use std::time::Duration;
use ureq::Agent;

fn agent(timeout: Duration, https_only: bool) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(timeout))
        .https_only(https_only)
        .user_agent(concat!("RustZIP/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn net_error(e: impl std::fmt::Display) -> Error {
    Error::Io(std::io::Error::other(format!("network: {e}")))
}

pub struct HttpFetcher {
    agent: Agent,
}

impl HttpFetcher {
    /// A client for the real servers: https only, generous time for a big download.
    pub fn new() -> Self {
        Self {
            agent: agent(Duration::from_secs(600), true),
        }
    }

    /// Plain http, for tests against a local server. Never use it for real downloads.
    pub fn insecure_for_tests() -> Self {
        Self {
            agent: agent(Duration::from_secs(10), false),
        }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher for HttpFetcher {
    fn get(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let mut response = self.agent.get(url).call().map_err(net_error)?;
        // The limit is enforced while reading, so a hostile server cannot make us buffer more.
        response
            .body_mut()
            .with_config()
            .limit(max_bytes)
            .read_to_vec()
            .map_err(net_error)
    }
}

/// Posts a statistics report as JSON.
pub struct HttpSender {
    agent: Agent,
    url: String,
}

impl HttpSender {
    pub fn new(url: &str) -> Self {
        Self {
            agent: agent(Duration::from_secs(8), true),
            url: url.to_string(),
        }
    }

    pub fn insecure_for_tests(url: &str) -> Self {
        Self {
            agent: agent(Duration::from_secs(5), false),
            url: url.to_string(),
        }
    }
}

impl Sender for HttpSender {
    fn send(&self, report: &Report) -> std::result::Result<(), String> {
        let body = serde_json::to_vec(report).map_err(|e| e.to_string())?;
        self.agent
            .post(&self.url)
            .header("Content-Type", "application/json")
            .send(&body[..])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    /// Serves one request with `reply` and returns what the client sent.
    fn serve_once(reply: &'static [u8]) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            // Read until the headers (and a small body) have arrived.
            loop {
                let n = s.read(&mut buf).unwrap();
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got).to_string();
                if let Some(h) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length: ")?
                                .parse()
                                .ok()
                        })
                        .unwrap_or(0usize);
                    if got.len() >= h + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            s.write_all(reply).unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        (url, handle)
    }

    #[test]
    fn fetches_a_body_and_enforces_the_size_limit() {
        let (url, h) =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        let f = HttpFetcher::insecure_for_tests();
        assert_eq!(f.get(&url, 100).unwrap(), b"hello");
        h.join().unwrap();

        let (url, h) =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        assert!(
            f.get(&url, 3).is_err(),
            "a body over the limit must be refused"
        );
        h.join().unwrap();
    }

    #[test]
    fn a_server_error_is_an_error() {
        let (url, h) =
            serve_once(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert!(HttpFetcher::insecure_for_tests().get(&url, 100).is_err());
        h.join().unwrap();
    }

    #[test]
    fn real_clients_refuse_plain_http() {
        let (url, _h) =
            serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert!(HttpFetcher::new().get(&url, 100).is_err());
    }

    #[test]
    fn a_report_is_posted_as_json_with_only_the_documented_fields() {
        let (url, h) = serve_once(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        let report = Report {
            event: "activate",
            install_id: "id-1".into(),
            app_version: "0.1.0".into(),
            os: "macos".into(),
            os_version: "15".into(),
            arch: "aarch64".into(),
            language: "zh-CN".into(),
        };
        HttpSender::insecure_for_tests(&url).send(&report).unwrap();
        let request = h.join().unwrap();
        assert!(request.starts_with("POST / HTTP/1.1"));
        let body = request.split("\r\n\r\n").nth(1).unwrap();
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "app_version",
                "arch",
                "event",
                "install_id",
                "language",
                "os",
                "os_version"
            ]
        );
    }

    #[test]
    fn an_unreachable_server_is_an_error_not_a_hang() {
        // Port 1 on localhost refuses connections.
        let sender = HttpSender::insecure_for_tests("http://127.0.0.1:1/");
        let report = Report {
            event: "heartbeat",
            install_id: "x".into(),
            app_version: "0".into(),
            os: "o".into(),
            os_version: "v".into(),
            arch: "a".into(),
            language: "l".into(),
        };
        assert!(sender.send(&report).is_err());
    }
}
