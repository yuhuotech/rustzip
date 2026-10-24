//! The environment facts a statistics report carries. Only the operating system's name and
//! version, the CPU architecture and the interface language: nothing about the user.

use rz_app::stats::Environment;
use std::process::Command;

fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn os_version() -> String {
    let found = match std::env::consts::OS {
        "macos" => run("sw_vers", &["-productVersion"]),
        "windows" => run("cmd", &["/C", "ver"]).map(|v| {
            // "Microsoft Windows [Version 10.0.22631.4037]" -> "10.0.22631.4037"
            v.split("Version ")
                .nth(1)
                .map_or(v.clone(), |s| s.trim_end_matches(']').to_string())
        }),
        _ => std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("VERSION_ID="))
                    .map(|v| v.trim_matches('"').to_string())
            }),
    };
    found.unwrap_or_else(|| "unknown".into())
}

fn language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty() && v != "C"))
        .map(|v| {
            // "zh_CN.UTF-8" -> "zh-CN"
            v.split('.').next().unwrap_or("").replace('_', "-")
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

pub fn environment() -> Environment {
    Environment {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        os_version: os_version(),
        arch: std::env::consts::ARCH.to_string(),
        language: language(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_has_every_field_and_nothing_personal() {
        let e = environment();
        assert!(!e.app_version.is_empty());
        assert!(!e.os.is_empty() && !e.os_version.is_empty());
        assert!(!e.arch.is_empty() && !e.language.is_empty());
        assert!(!e.language.contains('.') && !e.language.contains('_'));
    }
}
