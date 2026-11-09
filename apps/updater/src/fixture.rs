//! Native CI fixture. Never included in a distribution package.

use ed25519_dalek::{Signer, SigningKey};
use rz_app::update::{hex, signed_message, Asset, Update};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|arg| arg == "--fixture-macos-handoff")
    {
        let helper = PathBuf::from(args.get(1).ok_or("missing macOS helper")?);
        rz_app::update_runtime::launch_macos_helper(&helper, &args[2..])?;
        return Ok(());
    }
    let config_path = std::env::current_exe()?
        .parent()
        .and_then(Path::parent)
        .map(|p| p.join("Resources/smoke-config.json"));
    let config: serde_json::Value = config_path
        .and_then(|p| fs::read(p).ok())
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default();
    if args.first().is_some_and(|a| a == "--fixture-update") {
        let source = PathBuf::from(args.get(1).ok_or("missing source")?);
        let archive = PathBuf::from(args.get(2).ok_or("missing archive")?);
        let platform = if cfg!(windows) {
            "windows-x86_64-zip".to_owned()
        } else {
            format!("linux-{}", std::env::consts::ARCH)
        };
        let version = "0.2.0";
        make_package(&source, &archive, version, &platform)?;
        let bytes = fs::read(&archive)?;
        let key = SigningKey::from_bytes(&[7; 32]);
        let sha = hex(&Sha256::digest(&bytes));
        let asset = Asset {
            platform: platform.clone(),
            url: "https://example.com/update.asset".into(),
            size: bytes.len() as u64,
            sha256: sha.clone(),
            signature: hex(&key
                .sign(&signed_message(version, &platform, &sha))
                .to_bytes()),
        };
        let manifest: rz_app::update::Manifest = serde_json::from_value(
            serde_json::json!({ "version": version, "notes": "fixture", "assets": [asset] }),
        )?;
        let update: Update = rz_app::update::select_update(
            &manifest,
            env!("CARGO_PKG_VERSION"),
            platform.strip_suffix("-zip").unwrap_or(&platform),
        )?
        .ok_or("no fixture update")?;
        rz_app::update_runtime::launch_update(&update, &archive, &key.verifying_key().to_bytes())?;
        return Ok(());
    }
    let restarting = args.iter().any(|a| a == "--rustzip-update-start");
    if restarting {
        let version = if config.is_object() {
            args.last()
                .and_then(|v| v.to_str())
                .unwrap_or("0.2.0")
                .to_owned()
        } else {
            "0.2.0".into()
        };
        rz_app::update_runtime::begin_startup(&mut args, &version)?;
        if std::env::var("RZ_UPDATE_FIXTURE_MODE").ok().as_deref() == Some("fail")
            || config["fail"].as_bool() == Some(true)
        {
            std::process::exit(86);
        }
        rz_app::update_runtime::complete_startup()?;
    }
    if let Some(path) = config["pid_file"].as_str() {
        fs::write(path, std::process::id().to_string())?;
    }
    if let Ok(path) = std::env::var("RZ_UPDATE_FIXTURE_RESULT") {
        fs::write(path, if restarting { "updated" } else { "rolled-back" })?;
    }
    if let Some(ms) = config["sleep_ms"].as_u64() {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    if let Some(path) = config["finished"].as_str() {
        fs::write(path, "finished")?;
    }
    Ok(())
}

fn make_package(
    source: &Path,
    archive: &Path,
    version: &str,
    platform: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if platform.ends_with("-zip") {
        let mut writer = zip::ZipWriter::new(fs::File::create(archive)?);
        for file in files(source)? {
            let relative = file
                .strip_prefix(source)?
                .to_str()
                .ok_or("non UTF-8 fixture name")?
                .replace('\\', "/");
            writer.start_file(relative.clone(), zip::write::SimpleFileOptions::default())?;
            if relative == "update-layout.json" {
                writer.write_all(
                    serde_json::to_string(
                        &serde_json::json!({"schema":1,"version":version,"platform":platform}),
                    )?
                    .as_bytes(),
                )?;
            } else {
                std::io::copy(&mut fs::File::open(file)?, &mut writer)?;
            }
        }
        writer.finish()?;
    } else {
        let gzip =
            flate2::write::GzEncoder::new(fs::File::create(archive)?, flate2::Compression::fast());
        let mut tar = tar::Builder::new(gzip);
        let root = format!("RustZIP-{version}-{platform}");
        for file in files(source)? {
            let relative = file.strip_prefix(source)?;
            let mut bytes = Vec::new();
            if relative == Path::new("update-layout.json") {
                bytes = serde_json::to_vec(
                    &serde_json::json!({"schema":1,"version":version,"platform":platform}),
                )?;
            } else {
                fs::File::open(&file)?.read_to_end(&mut bytes)?;
            }
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, Path::new(&root).join(relative), &bytes[..])?;
        }
        tar.into_inner()?.finish()?;
    }
    Ok(())
}

fn files(root: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            files.extend(self::files(&entry.path())?);
        } else if entry.file_type()?.is_file() {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("update fixture: {error}");
        std::process::exit(1);
    }
}
