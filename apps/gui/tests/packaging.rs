#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

const WINDOWS: &str = include_str!("../../../packaging/windows/rustzip.iss");
const LINUX_DESKTOP: &str = include_str!("../../../packaging/linux/rustzip.desktop");
const LINUX_NEMO: &str = include_str!("../../../packaging/linux/rustzip-extract.nemo_action");
const WINDOWS_INSTALLER: &str = include_str!("../../../packaging/windows/rustzip.iss");
const MACOS_INFO: &str = include_str!("../../../packaging/macos/Info.plist.in");
const MACOS_BUILD: &str = include_str!("../../../packaging/macos/build-app.sh");
const LINUX_TARBALL: &str = include_str!("../../../packaging/linux/build-tarball.sh");

#[test]
fn windows_open_with_and_extract_support_the_same_archive_extensions() {
    let mut open = BTreeSet::new();
    let mut extract = BTreeSet::new();
    let mut rar_enabled = false;
    let mut rar_entries = 0;
    for line in WINDOWS.lines() {
        if line.trim() == "#if RarEnabled == 1" {
            rar_enabled = true;
        } else if line.trim() == "#endif" {
            rar_enabled = false;
        }
        let marker = if line.contains("OpenWithProgids") {
            Some(("Software\\Classes\\.", &mut open))
        } else if line.contains("SystemFileAssociations") {
            Some(("SystemFileAssociations\\.", &mut extract))
        } else {
            None
        };
        if let Some((prefix, formats)) = marker {
            let extension = line
                .split_once(prefix)
                .unwrap()
                .1
                .split('\\')
                .next()
                .unwrap();
            if extension == "rar" {
                assert!(rar_enabled, "RAR registration must be conditional");
                rar_entries += 1;
            }
            formats.insert(extension);
        }
    }
    assert_eq!(open, extract);
    assert_eq!(rar_entries, 3);
    for extension in [
        "zip", "7z", "rar", "lz4", "iso", "cab", "cpio", "deb", "rpm", "chm",
    ] {
        assert!(open.contains(extension), "{extension} is missing");
    }
}

#[test]
fn optional_rar_is_not_advertised_in_default_linux_descriptors() {
    for descriptor in [LINUX_DESKTOP, LINUX_NEMO] {
        assert!(!descriptor.contains("application/vnd.rar"));
        assert!(!descriptor.contains("application/x-rar"));
        assert!(descriptor.contains("application/zip"));
    }
}

#[test]
fn chm_is_advertised_in_linux_descriptors() {
    for descriptor in [LINUX_DESKTOP, LINUX_NEMO] {
        assert!(descriptor.contains("application/vnd.ms-htmlhelp"));
    }
}

#[test]
fn chm_is_listed_as_a_read_only_macos_document_type() {
    assert!(MACOS_INFO.contains("<string>chm</string>"));
    assert!(MACOS_INFO.contains("<key>CFBundleTypeRole</key><string>Viewer</string>"));
    assert!(MACOS_BUILD.contains("packaging/licenses/libchm.txt"));
    assert!(LINUX_TARBALL.contains("packaging/licenses/libchm.txt"));
    assert!(LINUX_TARBALL.contains("libchm-license.txt"));
}

#[test]
fn parallel_bzip_decoder_licenses_are_in_every_desktop_package() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in ["pbz2.txt", "pbz2-notice.txt"] {
        assert!(root.join("../../packaging/licenses").join(file).is_file());
    }
    assert!(MACOS_BUILD.contains("packaging/licenses/pbz2.txt"));
    assert!(MACOS_BUILD.contains("packaging/licenses/pbz2-notice.txt"));
    assert!(LINUX_TARBALL.contains("packaging/licenses/pbz2.txt"));
    assert!(LINUX_TARBALL.contains("packaging/licenses/pbz2-notice.txt"));
    assert!(WINDOWS_INSTALLER.contains(r#"DestName: "pbz2.txt""#));
    assert!(WINDOWS_INSTALLER.contains(r#"DestName: "pbz2-notice.txt""#));
}

#[test]
fn windows_installer_uses_the_bundled_simplified_chinese_language_file() {
    let language = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/windows/ChineseSimplified.isl");
    assert!(language.is_file());
    assert!(WINDOWS_INSTALLER.contains(r#"MessagesFile: "ChineseSimplified.isl""#));
    assert!(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/licenses/inno-chinese-translation.txt")
            .is_file()
    );
}

#[test]
fn windows_installer_includes_the_chm_reader_license() {
    assert!(WINDOWS_INSTALLER.contains(r#"DestName: "libchm.txt""#));
    assert!(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/licenses/libchm.txt")
            .is_file()
    );
}

#[cfg(not(feature = "rar"))]
#[test]
fn default_gui_build_reports_rar_as_unsupported() {
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/rz-formats/tests/fixtures/container.rar");
    assert!(matches!(
        rustzip_gui::view::load_archive(&archive, None),
        Err(rz_core::Error::Unsupported(_))
    ));
}

#[cfg(unix)]
#[test]
fn linux_packaging_keeps_rar_features_and_descriptors_in_sync() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Command;

    fn copy_files(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
            }
        }
    }

    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    copy_files(
        &project.join("packaging/linux"),
        &root.join("packaging/linux"),
    );
    copy_files(
        &project.join("packaging/icons/hicolor"),
        &root.join("packaging/icons/hicolor"),
    );
    copy_files(
        &project.join("packaging/licenses"),
        &root.join("packaging/licenses"),
    );
    fs::write(root.join("Cargo.toml"), "version = \"0.1.0\"\n").unwrap();
    fs::create_dir_all(root.join("target/release")).unwrap();
    fs::write(root.join("target/release/rustzip"), "fixture").unwrap();
    fs::create_dir_all(root.join("fake-bin")).unwrap();
    let cargo = root.join("fake-bin/cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$RZ_FAKE_CARGO_ARGS\"\n",
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        root.join("fake-bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let invalid_output = root.join("invalid-out");
    let invalid = Command::new("bash")
        .arg(root.join("packaging/linux/build-tarball.sh"))
        .arg(&invalid_output)
        .env("PATH", &path)
        .env("RZ_WITH_RAR", "true")
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(!invalid_output.exists());
    for enabled in [false, true] {
        let output = root.join(if enabled { "rar-out" } else { "default-out" });
        let args = root.join(if enabled { "rar-args" } else { "default-args" });
        let result = Command::new("bash")
            .arg(root.join("packaging/linux/build-tarball.sh"))
            .arg(&output)
            .env("PATH", &path)
            .env("RZ_FAKE_CARGO_ARGS", &args)
            .env("RZ_WITH_RAR", if enabled { "1" } else { "0" })
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let arguments = fs::read_to_string(args).unwrap();
        assert_eq!(arguments.lines().any(|arg| arg == "rar"), enabled);
        let archive = fs::read_dir(&output)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let extract = root.join(if enabled {
            "rar-extract"
        } else {
            "default-extract"
        });
        fs::create_dir(&extract).unwrap();
        assert!(
            Command::new("tar")
                .args(["-xzf"])
                .arg(&archive)
                .arg("-C")
                .arg(&extract)
                .status()
                .unwrap()
                .success()
        );
        let package = fs::read_dir(&extract)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        for name in ["rustzip.desktop", "rustzip-extract.nemo_action"] {
            let descriptor = fs::read_to_string(package.join(name)).unwrap();
            assert_eq!(
                descriptor.contains("application/vnd.rar"),
                enabled,
                "{name}"
            );
        }
        assert_eq!(package.join("unrar-license.txt").exists(), enabled);
    }
}
