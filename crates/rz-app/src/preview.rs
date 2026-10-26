//! What the details panel shows for a file: a readable type name and, for pictures,
//! the picture itself.

use rz_core::{Entry, Error};
use std::io::Read;
use std::path::Path;

/// The largest picture that is unpacked just to be shown. Bigger ones get the
/// type tile instead; they are better extracted and opened in a viewer.
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
/// Text is rendered in the detail panel, so keep both decoding and layout work small.
pub const MAX_TEXT_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}

/// The picture format of a file name, when it is one that can be previewed.
pub fn image_format(name: &str) -> Option<ImageFormat> {
    match extension(name).as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "gif" => Some(ImageFormat::Gif),
        "webp" => Some(ImageFormat::Webp),
        "bmp" => Some(ImageFormat::Bmp),
        _ => None,
    }
}

/// Whether a file can be shown as plain UTF-8 text without handing it to a renderer.
pub fn is_text_format(name: &str) -> bool {
    matches!(
        extension(name).as_str(),
        "txt"
            | "md"
            | "rst"
            | "log"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "xml"
            | "csv"
            | "tsv"
            | "rs"
            | "py"
            | "go"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "java"
            | "sh"
            | "sql"
            | "js"
            | "mjs"
            | "ts"
            | "css"
    )
}

/// Decodes bounded preview bytes only when they are text rather than binary data.
pub fn decode_text(data: Vec<u8>) -> Option<String> {
    if data.contains(&0) {
        return None;
    }
    let text = String::from_utf8(data).ok()?;
    Some(text.strip_prefix('\u{feff}').unwrap_or(&text).to_string())
}

/// A human name for a file type, such as `JavaScript` for `app.js`.
pub fn type_name(name: &str) -> String {
    let ext = extension(name);
    let known = match ext.as_str() {
        "js" | "mjs" => "JavaScript",
        "ts" => "TypeScript",
        "json" => "JSON",
        "css" => "CSS 样式表",
        "html" | "htm" => "HTML 网页",
        "md" => "Markdown 文档",
        "txt" => "文本文件",
        "pdf" => "PDF 文档",
        "doc" | "docx" => "Word 文档",
        "xls" | "xlsx" => "Excel 表格",
        "ppt" | "pptx" => "PowerPoint 演示",
        "csv" => "CSV 表格",
        "svg" => "SVG 矢量图",
        "png" => "PNG 图片",
        "jpg" | "jpeg" => "JPEG 图片",
        "gif" => "GIF 图片",
        "webp" => "WebP 图片",
        "bmp" => "BMP 图片",
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" => "音频",
        "mp4" | "mov" | "mkv" | "avi" | "webm" => "视频",
        "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "zst" | "bz2" | "tgz" => "压缩包",
        "rs" => "Rust 源码",
        "py" => "Python 脚本",
        "go" => "Go 源码",
        "c" | "h" => "C 源码",
        "cpp" | "hpp" => "C++ 源码",
        "java" => "Java 源码",
        "sh" => "Shell 脚本",
        "toml" | "yaml" | "yml" => "配置文件",
        "xml" => "XML",
        "sql" => "SQL",
        "sqlite" | "db" => "数据库",
        "exe" | "dll" | "so" | "dylib" => "程序",
        "" => "文件",
        _ => return format!("{} 文件", ext.to_uppercase()),
    };
    known.to_string()
}

/// Reads one entry into memory, refusing anything larger than `limit` bytes.
///
/// The archive is untrusted: the declared size is checked first and the read itself
/// is capped too, so a lying header cannot make this allocate more than `limit`.
/// Runs off the UI thread.
pub fn read_entry(
    archive: &Path,
    entry: &Entry,
    password: Option<&str>,
    encoding: Option<&str>,
    limit: u64,
) -> Result<Vec<u8>, Error> {
    if entry.size > limit {
        return Err(Error::LimitExceeded(format!(
            "{} bytes is above the preview limit",
            entry.size
        )));
    }
    let mut reader = rz_formats::open(archive)?;
    if let Some(pw) = password {
        reader.set_password(pw)?;
    }
    if let Some(enc) = encoding {
        reader.set_encoding(enc)?;
    }
    // Several formats only know their entries once the list has been read.
    reader.entries()?;
    let mut data = Vec::new();
    reader.open(entry)?.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(Error::LimitExceeded("entry is larger than declared".into()));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_have_names_and_pictures_are_recognised() {
        assert_eq!(type_name("app.js"), "JavaScript");
        assert_eq!(type_name("Makefile"), "文件");
        assert_eq!(type_name("a.XYZ"), "XYZ 文件");
        assert_eq!(image_format("A.JPG"), Some(ImageFormat::Jpeg));
        assert_eq!(image_format("logo.svg"), None);
        assert_eq!(image_format("noext"), None);
        assert!(is_text_format("README.MD"));
        assert!(!is_text_format("page.html"));
        assert!(!is_text_format("program.exe"));
        assert_eq!(
            decode_text(b"\xef\xbb\xbfhello".to_vec()).as_deref(),
            Some("hello")
        );
        assert!(decode_text(vec![b'a', 0, b'b']).is_none());
    }

    #[test]
    fn an_entry_is_read_and_a_big_one_is_refused() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("a.png", opts).unwrap();
        zip.write_all(&[7u8; 100]).unwrap();
        zip.finish().unwrap();

        let mut reader = rz_formats::open(&path).unwrap();
        let entry = reader.entries().unwrap().remove(0);
        let data = read_entry(&path, &entry, None, None, 1000).unwrap();
        assert_eq!(data, vec![7u8; 100]);
        assert!(matches!(
            read_entry(&path, &entry, None, None, 50),
            Err(Error::LimitExceeded(_))
        ));
    }

    #[test]
    fn text_preview_reads_are_bounded_by_the_detail_panel_limit() {
        use std::io::Write;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("text.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("notes.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hello").unwrap();
        zip.finish().unwrap();

        let mut reader = rz_formats::open(&path).unwrap();
        let entry = reader.entries().unwrap().remove(0);
        let data = read_entry(&path, &entry, None, None, MAX_TEXT_BYTES).unwrap();
        assert_eq!(decode_text(data).as_deref(), Some("hello"));
        assert!(matches!(
            read_entry(&path, &entry, None, None, 4),
            Err(Error::LimitExceeded(_))
        ));
    }
}
