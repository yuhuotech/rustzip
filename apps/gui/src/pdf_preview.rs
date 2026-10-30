//! Runs PDF parsing in a separate, allocation-bounded helper process.

use rz_core::{CancelToken, Error, Result};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAX_PDF_INPUT: usize = 16 * 1024 * 1024;
const MAX_PDF_OUTPUT: usize = 8 * 1024 * 1024;
const RENDER_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

fn helper_path() -> Result<PathBuf> {
    let filename = if cfg!(target_os = "windows") {
        "rz-pdf-renderer.exe"
    } else {
        "rz-pdf-renderer"
    };
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or_else(|| Error::Unsupported("PDF preview helper is unavailable".into()))?;
    let sibling = directory.join(filename);
    if sibling.is_file() {
        return Ok(sibling);
    }

    // Cargo places integration-test executables under `deps`, beside the target bin directory.
    if directory.file_name().is_some_and(|name| name == "deps")
        && let Some(target_dir) = directory.parent()
    {
        let test_sibling = target_dir.join(filename);
        if test_sibling.is_file() {
            return Ok(test_sibling);
        }
    }

    Err(Error::Unsupported(
        "PDF preview helper is unavailable; open this file with a system PDF viewer".into(),
    ))
}

fn read_bounded_png(mut reader: impl Read) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if output.len().saturating_add(read) > MAX_PDF_OUTPUT {
            return Err(Error::LimitExceeded(
                "PDF preview image exceeds the output limit".into(),
            ));
        }
        output.extend_from_slice(&buffer[..read]);
    }
    if !output.starts_with(PNG_SIGNATURE) {
        return Err(Error::Unsupported(
            "PDF preview could not be rendered; open it with a system PDF viewer".into(),
        ));
    }
    if !crate::image_preview::dimensions_within_limit(&output) {
        return Err(Error::LimitExceeded(
            "PDF preview image exceeds the pixel limit".into(),
        ));
    }
    Ok(output)
}

fn terminate_child(child: &mut Child, reader: JoinHandle<()>) {
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
}

/// Renders the first page under the helper's heap, pixel, and output limits.
pub fn render_first_page(pdf: &[u8], cancel: &CancelToken) -> Result<Vec<u8>> {
    if pdf.len() > MAX_PDF_INPUT {
        return Err(Error::LimitExceeded(
            "PDF source exceeds the preview input limit".into(),
        ));
    }
    cancel.check()?;
    let helper = helper_path()?;
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Error::Io(std::io::Error::other(
            "PDF helper output pipe is unavailable",
        )));
    };

    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = match thread::Builder::new()
        .name("rustzip-pdf-output".into())
        .spawn(move || {
            let _ = sender.send(read_bounded_png(stdout));
        }) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Io(error));
        }
    };

    let Some(mut stdin) = child.stdin.take() else {
        terminate_child(&mut child, reader);
        return Err(Error::Io(std::io::Error::other(
            "PDF helper input pipe is unavailable",
        )));
    };
    for chunk in pdf.chunks(64 * 1024) {
        if cancel.is_cancelled() {
            drop(stdin);
            terminate_child(&mut child, reader);
            return Err(Error::Cancelled);
        }
        if let Err(error) = stdin.write_all(chunk) {
            drop(stdin);
            terminate_child(&mut child, reader);
            return Err(Error::Io(error));
        }
    }
    drop(stdin);

    let started = Instant::now();
    let mut output = None;
    loop {
        if cancel.is_cancelled() {
            terminate_child(&mut child, reader);
            return Err(Error::Cancelled);
        }
        if started.elapsed() >= RENDER_TIMEOUT {
            terminate_child(&mut child, reader);
            return Err(Error::LimitExceeded(
                "PDF rendering exceeded the time limit".into(),
            ));
        }

        match receiver.try_recv() {
            Ok(Ok(bytes)) => output = Some(bytes),
            Ok(Err(error)) => {
                terminate_child(&mut child, reader);
                return Err(error);
            }
            Err(TryRecvError::Disconnected) => {
                terminate_child(&mut child, reader);
                return Err(Error::Io(std::io::Error::other(
                    "PDF helper output reader stopped",
                )));
            }
            Err(TryRecvError::Empty) => {}
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                let output = match output {
                    Some(output) => output,
                    None => match receiver.recv_timeout(Duration::from_secs(1)) {
                        Ok(Ok(output)) => output,
                        Ok(Err(error)) => return Err(error),
                        Err(error) => {
                            return Err(Error::Io(std::io::Error::other(error.to_string())));
                        }
                    },
                };
                let _ = reader.join();
                if !status.success() {
                    return Err(Error::LimitExceeded(
                        "PDF rendering stopped at a resource limit; open it with a system PDF viewer"
                            .into(),
                    ));
                }
                return Ok(output);
            }
            Ok(None) => {}
            Err(error) => {
                terminate_child(&mut child, reader);
                return Err(Error::Io(error));
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
}
