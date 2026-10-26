#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};

fn rz(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rz"))
        .args(args)
        .output()
        .unwrap()
}

fn s(o: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn sample(root: &Path) {
    fs::create_dir_all(root.join("proj/docs")).unwrap();
    fs::write(root.join("proj/readme.txt"), "hello rustzip").unwrap();
    fs::write(root.join("proj/docs/说明.md"), "# 中文文件名").unwrap();
    fs::write(root.join("proj/empty.bin"), "").unwrap();
}

fn password_file(root: &Path, name: &str, password: &str) -> String {
    let path = root.join(name);
    fs::write(&path, format!("{password}\n")).unwrap();
    format!("@{}", p(&path))
}

#[test]
fn no_args_prints_usage_and_succeeds() {
    let o = rz(&[]);
    assert!(o.status.success());
    assert!(s(&o).0.contains("Usage"));
}

#[test]
fn add_list_test_extract_round_trip_for_every_format() {
    for ext in [
        "zip", "7z", "tar", "tar.gz", "tgz", "tar.xz", "tar.zst", "tar.bz2",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        sample(tmp.path());
        let archive = tmp.path().join(format!("out.{ext}"));
        let out = tmp.path().join("extracted");

        let o = rz(&["a", p(&archive), p(&tmp.path().join("proj"))]);
        assert!(o.status.success(), "{ext}: add failed: {:?}", s(&o));

        let o = rz(&["l", p(&archive)]);
        let (stdout, _) = s(&o);
        assert!(o.status.success());
        assert!(stdout.contains("proj/readme.txt"), "{ext}: {stdout}");
        assert!(stdout.contains("proj/docs/说明.md"), "{ext}: {stdout}");

        let o = rz(&["t", p(&archive)]);
        assert!(o.status.success(), "{ext}: {:?}", s(&o));
        assert!(s(&o).0.starts_with("OK"));

        let o = rz(&["x", p(&archive), "-o", p(&out)]);
        assert!(o.status.success(), "{ext}: {:?}", s(&o));
        assert_eq!(
            fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
            "hello rustzip"
        );
        assert_eq!(
            fs::read_to_string(out.join("proj/docs/说明.md")).unwrap(),
            "# 中文文件名"
        );
        assert_eq!(fs::read(out.join("proj/empty.bin")).unwrap().len(), 0);
    }
}

#[test]
fn encrypted_zip_needs_the_right_password() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    let password = password_file(tmp.path(), "secret.txt", "s3cret");
    let wrong_password = password_file(tmp.path(), "wrong.txt", "wrong");
    let archive = tmp.path().join("secret.zip");
    let src = tmp.path().join("proj");
    let o = rz(&["a", p(&archive), p(&src), "-p", &password]);
    assert!(o.status.success(), "{:?}", s(&o));

    let out = tmp.path().join("o1");
    let o = rz(&["x", p(&archive), "-o", p(&out)]);
    assert!(!o.status.success());
    assert!(s(&o).1.to_lowercase().contains("password"));

    let o = rz(&["x", p(&archive), "-o", p(&out), "-p", &wrong_password]);
    assert!(!o.status.success());

    let out = tmp.path().join("o2");
    let o = rz(&["x", p(&archive), "-o", p(&out), "-p", &password]);
    assert!(o.status.success(), "{:?}", s(&o));
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "hello rustzip"
    );
}

#[test]
fn extract_keeps_both_unless_told_otherwise() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    let archive = tmp.path().join("a.zip");
    assert!(rz(&["a", p(&archive), p(&tmp.path().join("proj"))])
        .status
        .success());
    let out = tmp.path().join("out");
    assert!(rz(&["x", p(&archive), "-o", p(&out)]).status.success());
    fs::write(out.join("proj/readme.txt"), "edited").unwrap();

    // By default both are kept: the edited folder stays, the new one is numbered.
    assert!(rz(&["x", p(&archive), "-o", p(&out)]).status.success());
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "edited"
    );
    assert_eq!(
        fs::read_to_string(out.join("proj (1)/readme.txt")).unwrap(),
        "hello rustzip"
    );

    assert!(rz(&["x", p(&archive), "-o", p(&out), "-n"])
        .status
        .success());
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "edited"
    );

    assert!(rz(&["x", p(&archive), "-o", p(&out), "-y"])
        .status
        .success());
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "hello rustzip"
    );
}

#[test]
fn extract_blocks_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("evil.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    w.start_file("../escaped.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    w.write_all(b"x").unwrap();
    w.finish().unwrap();

    let out = tmp.path().join("out");
    let o = rz(&["x", p(&archive), "-o", p(&out)]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("unsafe path"));
    assert!(!tmp.path().join("escaped.txt").exists());
}

#[test]
fn single_file_formats_round_trip_through_the_cli() {
    for ext in ["gz", "xz", "zst", "bz2"] {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("log.txt");
        fs::write(&file, "one line\n".repeat(5000)).unwrap();
        let archive = tmp.path().join(format!("log.txt.{ext}"));
        let o = rz(&["a", p(&archive), p(&file)]);
        assert!(o.status.success(), "{ext}: {:?}", s(&o));
        let o = rz(&["t", p(&archive)]);
        assert!(o.status.success(), "{ext}: {:?}", s(&o));
        let out = tmp.path().join("out");
        let o = rz(&["x", p(&archive), "-o", p(&out)]);
        assert!(o.status.success(), "{ext}: {:?}", s(&o));
        assert_eq!(
            fs::read_to_string(out.join("log.txt")).unwrap(),
            "one line\n".repeat(5000),
            "{ext}"
        );
    }
}

#[test]
fn encrypted_7z_lists_only_with_the_password() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    let password = password_file(tmp.path(), "secret.txt", "pw");
    let archive = tmp.path().join("secret.7z");
    let o = rz(&[
        "a",
        p(&archive),
        p(&tmp.path().join("proj")),
        "-p",
        &password,
        "-t",
        "2",
    ]);
    assert!(o.status.success(), "{:?}", s(&o));

    let o = rz(&["l", p(&archive)]);
    assert!(!o.status.success());
    assert!(s(&o).1.to_lowercase().contains("password"));

    let o = rz(&["l", p(&archive), "-p", &password]);
    assert!(o.status.success(), "{:?}", s(&o));
    assert!(s(&o).0.contains("proj/readme.txt"));

    let o = rz(&["t", p(&archive), "-p", &password]);
    assert!(o.status.success(), "{:?}", s(&o));
}

#[test]
fn sevenz_dictionary_and_solid_block_flags_create_readable_archives() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    let archive = tmp.path().join("advanced.7z");
    let o = rz(&[
        "a",
        p(&archive),
        p(&tmp.path().join("proj")),
        "-d",
        "2M",
        "-s",
        "1",
    ]);
    assert!(o.status.success(), "{:?}", s(&o));
    assert!(rz(&["t", p(&archive)]).status.success());

    let out = tmp.path().join("out");
    let o = rz(&["x", p(&archive), "-o", p(&out)]);
    assert!(o.status.success(), "{:?}", s(&o));
    assert_eq!(
        fs::read_to_string(out.join("proj/docs/说明.md")).unwrap(),
        "# 中文文件名"
    );

    let refused = tmp.path().join("over-budget.7z");
    let o = rz(&["a", p(&refused), p(&tmp.path().join("proj")), "-d", "64M"]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("memory budget"));
    assert!(!refused.exists());
}

#[test]
fn thread_count_does_not_change_the_result() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    for threads in ["1", "4"] {
        let archive = tmp.path().join(format!("t{threads}.zip"));
        let out = tmp.path().join(format!("o{threads}"));
        let src = tmp.path().join("proj");
        assert!(rz(&["a", p(&archive), p(&src), "-t", threads])
            .status
            .success());
        assert!(rz(&["x", p(&archive), "-o", p(&out), "-t", threads])
            .status
            .success());
        assert_eq!(
            fs::read_to_string(out.join("proj/docs/说明.md")).unwrap(),
            "# 中文文件名"
        );
    }
    assert!(!rz(&["a", "x.zip", "y", "-t", "0"]).status.success());
}

#[test]
fn single_file_formats_reject_folders_and_passwords_clearly() {
    let tmp = tempfile::tempdir().unwrap();
    sample(tmp.path());
    let password = password_file(tmp.path(), "secret.txt", "pw");
    let o = rz(&[
        "a",
        p(&tmp.path().join("x.gz")),
        p(&tmp.path().join("proj")),
    ]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("one file"));
    assert!(!tmp.path().join("x.gz").exists());

    let o = rz(&[
        "a",
        p(&tmp.path().join("y.tar.gz")),
        p(&tmp.path().join("proj")),
        "-p",
        &password,
    ]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("cannot be encrypted"));
}

/// A zip whose one file is named "测试文档.txt" in GBK without the UTF-8 flag,
/// as old Chinese Windows tools wrote them.
fn gbk_zip(path: &Path) {
    let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
    w.start_file("QQQQQQQQQQQQ", zip::write::SimpleFileOptions::default())
        .unwrap();
    w.write_all("正文".as_bytes()).unwrap();
    w.finish().unwrap();
    let gbk: Vec<u8> = [
        &[0xB2, 0xE2, 0xCA, 0xD4, 0xCE, 0xC4, 0xB5, 0xB5][..],
        b".txt",
    ]
    .concat();
    let mut data = fs::read(path).unwrap();
    let mut patched = 0;
    while let Some(at) = data.windows(12).position(|w| w == b"QQQQQQQQQQQQ") {
        data[at..at + 12].copy_from_slice(&gbk);
        patched += 1;
    }
    assert_eq!(patched, 2);
    fs::write(path, data).unwrap();
}

#[test]
fn legacy_gbk_names_can_be_named_with_the_encoding_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("old.zip");
    gbk_zip(&archive);

    let o = rz(&["l", p(&archive), "-e", "gbk"]);
    assert!(o.status.success(), "{:?}", s(&o));
    assert!(s(&o).0.contains("测试文档.txt"), "{}", s(&o).0);

    let out = tmp.path().join("out");
    let o = rz(&["x", p(&archive), "-o", p(&out), "-e", "gbk"]);
    assert!(o.status.success(), "{:?}", s(&o));
    assert_eq!(
        fs::read_to_string(out.join("测试文档.txt")).unwrap(),
        "正文"
    );

    let o = rz(&["t", p(&archive), "-e", "gbk"]);
    assert!(o.status.success(), "{:?}", s(&o));

    let o = rz(&["l", p(&archive), "-e", "no-such-charset"]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("unknown character set"));
}

#[test]
fn reports_clear_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let junk = tmp.path().join("junk.zip");
    fs::write(&junk, "this is not an archive").unwrap();
    let o = rz(&["l", p(&junk)]);
    assert!(!o.status.success());
    assert!(
        s(&o).1.contains("unrecognized archive format"),
        "unexpected archive error: {:?}",
        s(&o)
    );

    let o = rz(&["l", p(&tmp.path().join("missing.zip"))]);
    assert!(!o.status.success());

    // RAR is read-only by design.
    let o = rz(&["a", p(&tmp.path().join("x.rar")), p(&junk)]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("cannot create"));

    let o = rz(&["bogus", "x"]);
    assert!(!o.status.success());
    assert!(s(&o).1.contains("unknown command"));

    let o = rz(&["a", p(&tmp.path().join("y.zip"))]);
    assert!(!o.status.success());
}

#[test]
fn hashes_files_with_sha256_and_blake3_or_a_selected_algorithm() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sample.bin");
    fs::write(&path, b"abc").unwrap();

    let both = rz(&["h", p(&path)]);
    assert!(both.status.success(), "{:?}", s(&both));
    let output = s(&both).0;
    assert!(output.contains("SHA-256"));
    assert!(output.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
    assert!(output.contains("BLAKE3"));
    assert!(output.contains("6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"));

    let selected = rz(&["h", p(&path), "-a", "sha256"]);
    assert!(selected.status.success(), "{:?}", s(&selected));
    assert!(s(&selected).0.contains("SHA-256"));
    assert!(!s(&selected).0.contains("BLAKE3"));
}

#[test]
fn failed_add_leaves_no_partial_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("never.zip");
    let o = rz(&["a", p(&archive), p(&tmp.path().join("does-not-exist"))]);
    assert!(!o.status.success());
    assert!(!archive.exists());
    let leftovers: Vec<String> = fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".never.zip.rz-") || n.starts_with("never.zip.rz-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
