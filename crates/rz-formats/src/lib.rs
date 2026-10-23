//! Format backends: zip, 7z, tar (plain, .gz, .xz, .zst, .bz2) and single-file
//! compressed streams. All of them read and write.

mod codec;
#[cfg(feature = "rar")]
mod rar_fmt;
mod sevenz_fmt;
mod single;
mod sniff;
mod tar_fmt;
mod timeutil;
mod zip_fmt;
mod zip_out;

pub use codec::Compression;
pub use sniff::{sniff, Format};

use rz_core::{ArchiveReader, ArchiveWriter, CreateOptions, Error, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Formats that can be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Zip,
    SevenZ,
    /// tar, optionally compressed (`Compression::None` is a plain `.tar`).
    Tar(Compression),
    /// One compressed file: `.gz`, `.xz`, `.zst`, `.bz2`.
    Single(Compression),
}

impl OutputFormat {
    /// Picks the format from the output file name.
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        let ends = |exts: &[&str]| exts.iter().any(|e| name.ends_with(e));
        Some(if ends(&[".zip"]) {
            Self::Zip
        } else if ends(&[".7z"]) {
            Self::SevenZ
        } else if ends(&[".tar.gz", ".tgz"]) {
            Self::Tar(Compression::Gzip)
        } else if ends(&[".tar.xz", ".txz"]) {
            Self::Tar(Compression::Xz)
        } else if ends(&[".tar.zst", ".tar.zstd", ".tzst"]) {
            Self::Tar(Compression::Zstd)
        } else if ends(&[".tar.bz2", ".tbz2", ".tbz"]) {
            Self::Tar(Compression::Bzip2)
        } else if ends(&[".tar"]) {
            Self::Tar(Compression::None)
        } else if ends(&[".gz"]) {
            Self::Single(Compression::Gzip)
        } else if ends(&[".xz"]) {
            Self::Single(Compression::Xz)
        } else if ends(&[".zst", ".zstd"]) {
            Self::Single(Compression::Zstd)
        } else if ends(&[".bz2"]) {
            Self::Single(Compression::Bzip2)
        } else {
            return None;
        })
    }

    /// True when the format can protect content with a password.
    pub fn supports_password(self) -> bool {
        matches!(self, Self::Zip | Self::SevenZ)
    }
}

fn compression_of(format: Format) -> Option<Compression> {
    match format {
        Format::Gzip => Some(Compression::Gzip),
        Format::Xz => Some(Compression::Xz),
        Format::Zstd => Some(Compression::Zstd),
        Format::Bzip2 => Some(Compression::Bzip2),
        _ => None,
    }
}

/// Looks inside a compressed stream: tar archives carry "ustar" at offset 257.
fn contains_tar(path: &Path, comp: Compression) -> Result<bool> {
    let mut stream = comp.reader(File::open(path)?)?;
    let mut head = [0u8; 262];
    let mut got = 0;
    while got < head.len() {
        match stream.read(&mut head[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) => return Err(Error::Corrupt(e.to_string())),
        }
    }
    Ok(got >= 262 && &head[257..262] == b"ustar")
}

/// Opens an archive, detecting the format from its magic bytes.
pub fn open(path: &Path) -> Result<Box<dyn ArchiveReader>> {
    let mut file = File::open(path)?;
    let format = sniff(&mut file)?;
    match format {
        Format::Zip => Ok(Box::new(zip_fmt::ZipReader::new(file, path)?)),
        Format::SevenZip => Ok(Box::new(sevenz_fmt::SevenReader::new(path)?)),
        Format::Tar => Ok(Box::new(tar_fmt::TarReader::new(path, Compression::None))),
        #[cfg(feature = "rar")]
        Format::Rar => Ok(Box::new(rar_fmt::RarReader::new(path)?)),
        #[cfg(not(feature = "rar"))]
        Format::Rar => Err(Error::Unsupported(
            "rar (this build has no RAR support)".into(),
        )),
        compressed => {
            let comp = compression_of(compressed).ok_or(Error::UnknownFormat)?;
            if contains_tar(path, comp)? {
                Ok(Box::new(tar_fmt::TarReader::new(path, comp)))
            } else {
                Ok(Box::new(single::SingleReader::new(path, comp)))
            }
        }
    }
}

/// Creates a new archive file at `path`, replacing any existing file.
pub fn create(
    path: &Path,
    format: OutputFormat,
    opts: &CreateOptions,
) -> Result<Box<dyn ArchiveWriter>> {
    if opts.password.as_deref().is_some_and(|p| !p.is_empty()) && !format.supports_password() {
        return Err(Error::Unsupported(
            "this format cannot be encrypted; use .zip or .7z".into(),
        ));
    }
    let file = File::create(path)?;
    Ok(match format {
        OutputFormat::Zip => Box::new(zip_out::ZipOut::new(file, opts)),
        OutputFormat::SevenZ => Box::new(sevenz_fmt::SevenOut::new(file, opts)?),
        OutputFormat::Tar(c) => Box::new(tar_fmt::TarOut::new(file, c, opts)?),
        OutputFormat::Single(c) => Box::new(single::SingleOut::new(file, c, opts)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn output_format_from_extension() {
        let f = |s: &str| OutputFormat::from_path(&PathBuf::from(s));
        assert_eq!(f("a.zip"), Some(OutputFormat::Zip));
        assert_eq!(f("A.ZIP"), Some(OutputFormat::Zip));
        assert_eq!(f("a.7z"), Some(OutputFormat::SevenZ));
        assert_eq!(f("a.tar"), Some(OutputFormat::Tar(Compression::None)));
        assert_eq!(f("a.tar.gz"), Some(OutputFormat::Tar(Compression::Gzip)));
        assert_eq!(f("a.tgz"), Some(OutputFormat::Tar(Compression::Gzip)));
        assert_eq!(f("a.tar.xz"), Some(OutputFormat::Tar(Compression::Xz)));
        assert_eq!(f("a.tar.zst"), Some(OutputFormat::Tar(Compression::Zstd)));
        assert_eq!(f("a.tar.bz2"), Some(OutputFormat::Tar(Compression::Bzip2)));
        assert_eq!(
            f("notes.txt.gz"),
            Some(OutputFormat::Single(Compression::Gzip))
        );
        assert_eq!(
            f("disk.img.zst"),
            Some(OutputFormat::Single(Compression::Zstd))
        );
        assert_eq!(f("a.rar"), None);
        assert_eq!(f("noext"), None);
    }
}
