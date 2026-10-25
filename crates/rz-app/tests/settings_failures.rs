//! LOC-07: a settings save that fails anywhere must leave the last good file
//! readable, report the failure, and concurrent savers must never interleave
//! their writes into a broken file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_app::settings::{Settings, Theme};
use std::fs;
use std::path::Path;

fn tmp_files(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".rz-") && n.ends_with(".tmp"))
        .collect()
}

/// Root can write into read-only folders; permission faults would not fire.
#[cfg(unix)]
fn permission_faults_fire(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
    let probe = dir.join("probe");
    let blocked = fs::File::create(&probe).is_err();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    let _ = fs::remove_file(&probe);
    blocked
}

#[test]
#[ignore = "requires a non-root Unix account; injected write/sync/publish faults run on every platform"]
fn a_save_into_a_read_only_folder_fails_and_keeps_the_last_good_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let good = Settings {
        theme: Theme::Dark,
        accent: "teal".into(),
        ..Settings::default()
    };
    good.save(&path).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(
            permission_faults_fire(dir.path()),
            "run this environment-specific test as a non-root user"
        );
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
        let broken = Settings {
            theme: Theme::Light,
            ..good.clone()
        };
        let res = broken.save(&path);
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(res.is_err(), "a read-only folder accepted the save");
        assert_eq!(Settings::load(&path), good, "the last good file was lost");
        assert!(tmp_files(dir.path()).is_empty(), "temporary left behind");
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[test]
fn a_directory_in_the_way_of_the_settings_file_fails_the_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::create_dir_all(&path).unwrap(); // a folder where the file should go

    let res = Settings::default().save(&path);
    assert!(res.is_err(), "a directory accepted the save");
    assert!(path.is_dir(), "the folder was damaged");
    assert!(tmp_files(dir.path()).is_empty(), "temporary left behind");
    // A regular save next to it still works.
    let other = dir.path().join("other.json");
    Settings::default().save(&other).unwrap();
    assert_eq!(Settings::load(&other), Settings::default());
}

#[test]
fn concurrent_savers_never_produce_a_broken_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let a = Settings {
        theme: Theme::Dark,
        accent: "a".into(),
        ..Settings::default()
    };
    let b = Settings {
        theme: Theme::Light,
        accent: "b".into(),
        ..Settings::default()
    };
    a.save(&path).unwrap();

    let mut handles = Vec::new();
    for t in 0..4 {
        let path = path.clone();
        let variant = if t % 2 == 0 { a.clone() } else { b.clone() };
        handles.push(std::thread::spawn(move || {
            for _ in 0..25 {
                variant.save(&path).unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let loaded = Settings::load(&path);
    assert!(
        loaded == a || loaded == b,
        "interleaved writes corrupted the file: {loaded:?}"
    );
    assert!(tmp_files(dir.path()).is_empty(), "temporaries left behind");
}

#[test]
fn rapid_successive_saves_end_with_the_last_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    for i in 0..50 {
        Settings {
            accent: format!("accent-{i}"),
            ..Settings::default()
        }
        .save(&path)
        .unwrap();
    }
    let loaded = Settings::load(&path);
    assert_eq!(loaded.accent, "accent-49");
    assert!(tmp_files(dir.path()).is_empty());
}
