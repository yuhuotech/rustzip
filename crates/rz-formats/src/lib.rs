//! Format backends. Currently zip, tar and tar.gz (read and write).

mod sniff;
mod tar_gz;
mod timeutil;
mod zip_fmt;

pub use sniff::{sniff, Format};

use rz_core::{ArchiveReader, ArchiveWriter, CreateOptions, Error, Result};
use std::fs::File;
use std::path::Path;

/// Formats that can be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Zip,
    Tar,
    TarGz,
}

impl OutputFormat {
    /// Picks the format from the output file name (`.zip`, `.tar`, `.tar.gz`, `.tgz`).
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        if name.ends_with(".zip") {
            Some(Self::Zip)
        } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if name.ends_with(".tar") {
            Some(Self::Tar)
        } else {
            None
        }
    }
}

/// Opens an archive, detecting the format from its magic bytes.
pub fn open(path: &Path) -> Result<Box<dyn ArchiveReader>> {
    let mut file = File::open(path)?;
    match sniff(&mut file)? {
        Format::Zip => Ok(Box::new(zip_fmt::ZipReader::new(file)?)),
        Format::Tar => Ok(Box::new(tar_gz::TarReader::new(path, false))),
        Format::Gzip => Ok(Box::new(tar_gz::TarReader::new(path, true))),
        other => Err(Error::Unsupported(other.name().to_string())),
    }
}

/// Creates a new archive file at `path`, replacing any existing file.
pub fn create(
    path: &Path,
    format: OutputFormat,
    opts: &CreateOptions,
) -> Result<Box<dyn ArchiveWriter>> {
    let file = File::create(path)?;
    Ok(match format {
        OutputFormat::Zip => Box::new(zip_fmt::ZipOut::new(file, opts)),
        OutputFormat::Tar => Box::new(tar_gz::TarOut::new(file, false, opts)?),
        OutputFormat::TarGz => Box::new(tar_gz::TarOut::new(file, true, opts)?),
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
        assert_eq!(f("a.tar.gz"), Some(OutputFormat::TarGz));
        assert_eq!(f("a.tgz"), Some(OutputFormat::TarGz));
        assert_eq!(f("a.tar"), Some(OutputFormat::Tar));
        assert_eq!(f("a.7z"), None);
        assert_eq!(f("noext"), None);
    }
}
