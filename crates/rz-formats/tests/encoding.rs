#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use encoding_rs::Encoding;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Builds a zip whose names are stored in `enc` without the UTF-8 flag, like
/// archives made by old Windows tools. The zip writer only writes UTF-8, so
/// names go in as same-length ASCII placeholders and are patched afterwards,
/// in both the local headers and the central directory.
fn legacy_zip(path: &Path, enc: &'static Encoding, names: &[&str]) {
    let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    let mut patches = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let (bytes, _, lossy) = enc.encode(name);
        assert!(!lossy, "{name} does not fit {}", enc.name());
        let mut holder = format!("Q{i:03}").into_bytes();
        holder.resize(bytes.len(), b'x');
        if name.ends_with('/') {
            *holder.last_mut().unwrap() = b'/';
        }
        let holder_name = String::from_utf8(holder.clone()).unwrap();
        if name.ends_with('/') {
            w.add_directory(holder_name.trim_end_matches('/'), opts)
                .unwrap();
        } else {
            w.start_file(&holder_name, opts).unwrap();
            w.write_all(format!("content of {i}").as_bytes()).unwrap();
        }
        patches.push((holder, bytes.into_owned()));
    }
    w.finish().unwrap();
    let mut data = fs::read(path).unwrap();
    for (from, to) in patches {
        assert_eq!(from.len(), to.len());
        let mut start = 0;
        let mut found = 0;
        while let Some(pos) = data[start..].windows(from.len()).position(|w| w == from) {
            let at = start + pos;
            data[at..at + to.len()].copy_from_slice(&to);
            start = at + to.len();
            found += 1;
        }
        assert_eq!(found, 2, "name must appear in local header and directory");
    }
    fs::write(path, data).unwrap();
}

fn names_of(path: &Path, configure: impl FnOnce(&mut dyn rz_core::ArchiveReader)) -> Vec<String> {
    let mut r = rz_formats::open(path).unwrap();
    configure(&mut *r);
    r.entries().unwrap().into_iter().map(|e| e.path).collect()
}

const CHINESE: [&str; 5] = [
    "项目文档/",
    "项目文档/需求说明书.docx",
    "项目文档/设计方案.pdf",
    "报告_2024年度总结.txt",
    "测试结果汇总表.xlsx",
];

const JAPANESE: [&str; 4] = [
    "設計書/",
    "設計書/仕様書.docx",
    "テスト結果報告.xlsx",
    "日本語のファイル名.txt",
];

#[test]
fn gbk_names_are_detected() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("gbk.zip");
    legacy_zip(&p, encoding_rs::GBK, &CHINESE);
    assert_eq!(names_of(&p, |_| {}), CHINESE);
}

#[test]
fn shift_jis_names_are_detected() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("sjis.zip");
    legacy_zip(&p, encoding_rs::SHIFT_JIS, &JAPANESE);
    assert_eq!(names_of(&p, |_| {}), JAPANESE);
}

#[test]
fn euc_kr_names_are_detected() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("kr.zip");
    let names = [
        "문서/",
        "문서/요구사항명세서.docx",
        "테스트결과보고서.xlsx",
        "한국어파일이름.txt",
    ];
    legacy_zip(&p, encoding_rs::EUC_KR, &names);
    assert_eq!(names_of(&p, |_| {}), names);
}

#[test]
fn a_forced_encoding_overrides_the_guess() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("gbk.zip");
    legacy_zip(&p, encoding_rs::GBK, &CHINESE);
    let forced = names_of(&p, |r| r.set_encoding("gbk").unwrap());
    assert_eq!(forced, CHINESE);
    // Decoding as the wrong set gives different text rather than failing.
    let wrong = names_of(&p, |r| r.set_encoding("shift_jis").unwrap());
    assert_ne!(wrong, CHINESE);
    assert_eq!(wrong.len(), CHINESE.len());
}

#[test]
fn unknown_encoding_labels_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.zip");
    legacy_zip(&p, encoding_rs::GBK, &CHINESE);
    let mut r = rz_formats::open(&p).unwrap();
    assert!(r.set_encoding("not-a-charset").is_err());
}

#[test]
fn utf8_and_ascii_names_are_never_reinterpreted() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("utf8.zip");
    let names = [
        "readme.txt",
        "文档/说明.md",
        "日本語/ファイル.txt",
        "ñandú.txt",
    ];
    let mut w = zip::ZipWriter::new(fs::File::create(&p).unwrap());
    for n in names {
        w.start_file(n, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(b"x").unwrap();
    }
    w.finish().unwrap();
    assert_eq!(names_of(&p, |_| {}), names);
}

#[test]
fn legacy_names_survive_reopen_for_parallel_extraction() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("gbk.zip");
    legacy_zip(&p, encoding_rs::GBK, &CHINESE);
    let mut r = rz_formats::open(&p).unwrap();
    r.set_encoding("gbk").unwrap();
    let mut other = r.reopen().unwrap().unwrap();
    let names: Vec<String> = other
        .entries()
        .unwrap()
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert_eq!(names, CHINESE);
}
