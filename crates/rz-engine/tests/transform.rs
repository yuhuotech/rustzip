#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_core::{CancelToken, CreateOptions, EntryKind, Error, NewEntry, NoProgress, Progress};
use rz_engine::{transform, ExtractOptions, TransformOptions};
use rz_formats::{Compression, OutputFormat};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

fn options(format: OutputFormat) -> TransformOptions {
    TransformOptions {
        format,
        create: CreateOptions {
            threads: 1,
            ..Default::default()
        },
        limits: ExtractOptions::default(),
        encoding: None,
    }
}

fn make_zip(path: &Path, names: &[(&str, &[u8])]) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in names {
        zip.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

fn contents(path: &Path, password: Option<&str>) -> BTreeMap<String, Vec<u8>> {
    let mut r = rz_formats::open(path).unwrap();
    if let Some(p) = password {
        r.set_password(p).unwrap();
    }
    let mut files = BTreeMap::new();
    r.for_each(&mut |e, data| {
        if e.kind == EntryKind::File {
            let mut out = Vec::new();
            data.read_to_end(&mut out)?;
            files.insert(e.path.clone(), out);
        }
        Ok(())
    })
    .unwrap();
    files
}

#[test]
fn conversion_round_trips_every_archive_output_and_preserves_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.zip");
    let long_name = format!("long/{}/说明.txt", "文".repeat(200));
    make_zip(
        &source,
        &[
            ("项目/说明.txt", "中文日本語한국어".as_bytes()),
            ("empty", b""),
            ("nested/data", b"payload"),
            (&long_name, b"long-name-content"),
        ],
    );
    let original = fs::read(&source).unwrap();
    let expected = contents(&source, None);
    for ext in ["zip", "7z", "tar", "tar.gz", "tar.xz", "tar.zst", "tar.bz2"] {
        let output = dir.path().join(format!("converted.{ext}"));
        let opts = options(OutputFormat::from_path(&output).unwrap());
        let mut seen = Vec::new();
        let rep = transform(
            &source,
            &output,
            &opts,
            false,
            &CancelToken::new(),
            &mut |p: Progress| seen.push(p),
        )
        .unwrap();
        assert_eq!(rep.files, 4);
        assert_eq!(contents(&output, None), expected, "{ext}");
        assert_eq!(fs::read(&source).unwrap(), original);
        let last = seen.last().unwrap();
        assert_eq!(last.bytes_done, last.bytes_total);
        assert_eq!(last.files_done, last.files_total);
        // Exercise sequential input as well as sequential output.
        let back = dir.path().join(format!("back-{ext}.zip"));
        transform(
            &output,
            &back,
            &options(OutputFormat::Zip),
            false,
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(contents(&back, None), expected);
    }
}

#[test]
fn source_and_output_passwords_are_independent_and_wrong_password_leaves_no_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("encrypted.7z");
    let mut writer = rz_formats::create(
        &source,
        OutputFormat::SevenZ,
        &CreateOptions {
            password: Some("input-secret".into()),
            threads: 1,
            ..Default::default()
        },
    )
    .unwrap();
    writer
        .add_file(
            &NewEntry {
                name: "文档.txt".into(),
                size: 4,
                unix_mode: None,
                modified: None,
            },
            &mut Cursor::new(b"text"),
        )
        .unwrap();
    writer.finish().unwrap();
    let out = dir.path().join("output.zip");
    let mut opts = options(OutputFormat::Zip);
    opts.limits.password = Some("wrong".into());
    assert!(matches!(
        transform(
            &source,
            &out,
            &opts,
            false,
            &CancelToken::new(),
            &mut NoProgress
        ),
        Err(Error::Password)
    ));
    assert!(!out.exists());
    opts.limits.password = Some("input-secret".into());
    opts.create.password = Some("output-secret".into());
    transform(
        &source,
        &out,
        &opts,
        false,
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(
        contents(&out, Some("output-secret"))
            .get("文档.txt")
            .unwrap(),
        b"text"
    );
    assert!(!format!("{opts:?}").contains("secret"));
}

#[test]
fn zip_repair_skips_bad_crc_and_recovers_later_entries() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("broken.zip");
    make_zip(
        &source,
        &[("bad", b"BROKEN-PAYLOAD"), ("good", b"SAFE-PAYLOAD")],
    );
    let mut bytes = fs::read(&source).unwrap();
    let offset = bytes
        .windows(14)
        .position(|s| s == b"BROKEN-PAYLOAD")
        .unwrap();
    bytes[offset] ^= 0xFF;
    fs::write(&source, &bytes).unwrap();
    let output = dir.path().join("recovered.zip");
    let rep = transform(
        &source,
        &output,
        &options(OutputFormat::Zip),
        true,
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!((rep.files, rep.skipped), (1, 1));
    assert!(rep.issues.iter().any(|s| s.contains("bad")));
    assert_eq!(
        contents(&output, None),
        BTreeMap::from([("good".into(), b"SAFE-PAYLOAD".to_vec())])
    );
    assert_eq!(fs::read(&source).unwrap(), bytes);
}

#[test]
fn missing_zip_directory_is_rebuilt_from_local_headers() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("lost-directory.zip");
    make_zip(&source, &[("说明.txt", b"hello"), ("empty", b"")]);
    let mut bytes = fs::read(&source).unwrap();
    let end = bytes.windows(4).position(|s| s == b"PK\x01\x02").unwrap();
    bytes.truncate(end);
    fs::write(&source, &bytes).unwrap();
    assert!(rz_formats::open(&source).is_err());
    let output = dir.path().join("recovered.7z");
    let rep = transform(
        &source,
        &output,
        &options(OutputFormat::SevenZ),
        true,
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!((rep.files, rep.skipped), (2, 0));
    assert!(!rep.issues.is_empty());
    assert_eq!(contents(&output, None).get("说明.txt").unwrap(), b"hello");
}

#[test]
fn sevenz_repair_continues_after_a_damaged_independent_block() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("broken.7z");
    let mut writer = rz_formats::create(
        &source,
        OutputFormat::SevenZ,
        &CreateOptions {
            level: 0,
            threads: 1,
            ..Default::default()
        },
    )
    .unwrap();
    for (name, bytes) in [
        ("bad", b"BROKEN-PAYLOAD".as_slice()),
        ("good", b"SAFE-PAYLOAD".as_slice()),
    ] {
        writer
            .add_file(
                &NewEntry {
                    name: name.into(),
                    size: bytes.len() as u64,
                    unix_mode: None,
                    modified: None,
                },
                &mut Cursor::new(bytes),
            )
            .unwrap();
    }
    writer.finish().unwrap();
    let mut bytes = fs::read(&source).unwrap();
    let offset = bytes
        .windows(14)
        .position(|s| s == b"BROKEN-PAYLOAD")
        .unwrap();
    bytes[offset] ^= 0xFF;
    fs::write(&source, &bytes).unwrap();
    let out = dir.path().join("recovered.zip");
    let rep = transform(
        &source,
        &out,
        &options(OutputFormat::Zip),
        true,
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!((rep.files, rep.skipped), (1, 1), "{rep:?}");
    assert_eq!(contents(&out, None).get("good").unwrap(), b"SAFE-PAYLOAD");
}

#[test]
fn unsafe_names_conflicts_and_limits_never_publish_conversion_results() {
    let dir = tempfile::tempdir().unwrap();
    for (ix, names) in [
        vec!["../escape"],
        vec!["/absolute"],
        vec!["C:\\escape"],
        vec!["a", "a/b"],
        vec!["a/b", "a"],
        vec!["A", "a"],
    ]
    .iter()
    .enumerate()
    {
        let source = dir.path().join(format!("unsafe-{ix}.zip"));
        make_zip(
            &source,
            &names
                .iter()
                .map(|n| (*n, b"x".as_slice()))
                .collect::<Vec<_>>(),
        );
        let out = dir.path().join("no-output.zip");
        assert!(transform(
            &source,
            &out,
            &options(OutputFormat::Zip),
            false,
            &CancelToken::new(),
            &mut NoProgress
        )
        .is_err());
        assert!(!out.exists());
    }
    let source = dir.path().join("limit.zip");
    make_zip(&source, &[("safe", b"12345")]);
    let mut opts = options(OutputFormat::Zip);
    opts.limits.max_total = 4;
    assert!(matches!(
        transform(
            &source,
            &dir.path().join("no-output.zip"),
            &opts,
            true,
            &CancelToken::new(),
            &mut NoProgress
        ),
        Err(Error::LimitExceeded(_))
    ));
    assert!(!dir.path().join("no-output.zip").exists());
    assert!(!dir.path().join("escape").exists());
}

#[test]
fn cancellation_cleans_temporary_outputs_and_overwrite_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.zip");
    make_zip(&source, &[("large", &vec![42; 2 << 20])]);
    let before: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect();
    let token = CancelToken::new();
    let stop = token.clone();
    let output = dir.path().join("cancelled.zip");
    assert!(matches!(
        transform(
            &source,
            &output,
            &options(OutputFormat::Zip),
            false,
            &token,
            &mut move |p: Progress| {
                if p.bytes_done > 0 {
                    stop.cancel();
                }
            }
        ),
        Err(Error::Cancelled)
    ));
    let after: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect();
    assert_eq!(before, after);
    fs::write(&output, b"KEEP").unwrap();
    assert!(transform(
        &source,
        &output,
        &options(OutputFormat::Zip),
        false,
        &CancelToken::new(),
        &mut NoProgress
    )
    .is_err());
    assert_eq!(fs::read(&output).unwrap(), b"KEEP");
    assert!(transform(
        &source,
        &source,
        &options(OutputFormat::Zip),
        true,
        &CancelToken::new(),
        &mut NoProgress
    )
    .is_err());
}

#[test]
fn empty_archives_convert_and_unrecoverable_input_does_not_create_an_empty_success() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("empty.zip");
    make_zip(&source, &[]);
    let out = dir.path().join("empty.7z");
    transform(
        &source,
        &out,
        &options(OutputFormat::SevenZ),
        false,
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert!(contents(&out, None).is_empty());
    let repair = dir.path().join("repair.zip");
    assert!(transform(
        &source,
        &repair,
        &options(OutputFormat::Tar(Compression::None)),
        true,
        &CancelToken::new(),
        &mut NoProgress
    )
    .is_err());
    assert!(!repair.exists());
}

#[test]
fn links_are_refused_by_conversion_and_reported_as_skipped_by_repair() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("links.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&source).unwrap());
    zip.add_symlink(
        "link",
        "../outside",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.start_file("safe", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"safe").unwrap();
    zip.finish().unwrap();
    let out = dir.path().join("converted.zip");
    assert!(matches!(
        transform(
            &source,
            &out,
            &options(OutputFormat::Zip),
            false,
            &CancelToken::new(),
            &mut NoProgress
        ),
        Err(Error::Unsupported(_))
    ));
    assert!(!out.exists());
    let rep = transform(
        &source,
        &out,
        &options(OutputFormat::Zip),
        true,
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!((rep.files, rep.skipped), (1, 1));
    assert!(rep.issues.iter().any(|s| s.contains("link")));
    assert_eq!(contents(&out, None).len(), 1);
}

#[test]
fn local_header_recovery_honors_cancel_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("missing-directory.zip");
    make_zip(&source, &[("large", &vec![23; 2 << 20])]);
    let mut bytes = fs::read(&source).unwrap();
    let end = bytes.windows(4).position(|s| s == b"PK\x01\x02").unwrap();
    bytes.truncate(end);
    fs::write(&source, bytes).unwrap();
    let token = CancelToken::new();
    let stop = token.clone();
    let out = dir.path().join("recovered.zip");
    assert!(matches!(
        transform(
            &source,
            &out,
            &options(OutputFormat::Zip),
            true,
            &token,
            &mut move |p: Progress| {
                if p.bytes_done > 0 {
                    stop.cancel();
                }
            }
        ),
        Err(Error::Cancelled)
    ));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(!out.exists());
}

#[test]
fn single_stream_inputs_convert_and_bound_the_size_measurement() {
    let dir = tempfile::tempdir().unwrap();
    for (extension, codec) in [
        ("gz", Compression::Gzip),
        ("xz", Compression::Xz),
        ("zst", Compression::Zstd),
        ("bz2", Compression::Bzip2),
    ] {
        let source = dir.path().join(format!("data.{extension}"));
        let mut writer = rz_formats::create(
            &source,
            OutputFormat::Single(codec),
            &CreateOptions::default(),
        )
        .unwrap();
        writer
            .add_file(
                &NewEntry {
                    name: "data".into(),
                    size: 5,
                    unix_mode: None,
                    modified: None,
                },
                &mut Cursor::new(b"hello"),
            )
            .unwrap();
        writer.finish().unwrap();
        let output = dir.path().join(format!("{extension}.zip"));
        transform(
            &source,
            &output,
            &options(OutputFormat::Zip),
            false,
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(contents(&output, None).get("data").unwrap(), b"hello");
        let limited = dir.path().join(format!("limited-{extension}.zip"));
        let mut opts = options(OutputFormat::Zip);
        opts.limits.max_total = 4;
        assert!(matches!(
            transform(
                &source,
                &limited,
                &opts,
                false,
                &CancelToken::new(),
                &mut NoProgress
            ),
            Err(Error::LimitExceeded(_))
        ));
        assert!(!limited.exists());
        let token = CancelToken::new();
        let mut reader = rz_formats::open(&source).unwrap();
        reader.set_read_control(token.clone(), 1024);
        token.cancel();
        assert!(matches!(reader.entries(), Err(Error::Cancelled)));
    }
}

#[cfg(feature = "rar")]
#[test]
fn rar_is_a_conversion_source_only() {
    let dir = tempfile::tempdir().unwrap();
    let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rz-formats/tests/fixtures/container.rar");
    let original = fs::read(&source).unwrap();
    let out = dir.path().join("converted.7z");
    transform(
        &source,
        &out,
        &options(OutputFormat::SevenZ),
        false,
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(
        contents(&out, None).get("container").unwrap(),
        b"#!/bin/sh\nexit 0\n"
    );
    assert_eq!(fs::read(&source).unwrap(), original);
    assert!(OutputFormat::from_path(Path::new("cannot-create.rar")).is_none());
}
