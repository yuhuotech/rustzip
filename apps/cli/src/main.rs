//! `rz` command line. Mirrors the 7z verbs and adds convert, repair and hash.

use rz_app::cli as archive_ops;
use rz_app::hash::HashAlgorithm;
use rz_app::settings::OnConflict;
use rz_core::{CancelToken, CreateOptions, EntryKind, Error, Progress, ProgressSink, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;
use std::time::Instant;
use zeroize::Zeroizing;

const USAGE: &str = "RustZIP\n\n\
Usage:\n  \
rz a <archive> <path>... [-l 0-9] [-p -|@file] [-t threads] [-v size] [-d size] [-s size|off] create\n  \
rz l <archive> [-p -|@file] [-e charset]                        list contents\n  \
rz x <archive> [-o dir] [-p -|@file] [-t threads] [-e charset] [-y|-n]\n                                                                  extract (-y replace, -n skip existing)\n  \
rz t <archive> [-p -|@file] [-e charset]                        test integrity\n  \
rz c|convert <archive> <output> [-p -|@file] [-q -|@file] [-e charset] [-l 0-9] [-t threads] [-d size] [-s size|off]\n                                                                  convert to output format\n  \
rz r|repair <archive> <output> [-p -|@file] [-q -|@file] [-e charset] [-l 0-9] [-t threads] [-d size] [-s size|off]\n                                                                  repair ZIP / 7z into a new archive\n\n\
rz h <file>... [-a sha256|blake3]                             calculate hashes\n\n\
-e names the character set of old zip file names (gbk, shift_jis, euc-kr, ...); it is guessed by default.\n\
-a selects SHA-256 or BLAKE3; omitting it prints both.\n\n\
-p - prompts for a password without echoing it; -p @file reads its first line from a file.\n\
-p is the source password for c / r; -q is the output password for c / r.\n\
-q accepts '-' for a hidden prompt or '@file' for the first line of a password file.\n\n\
-v sets the maximum volume size, e.g. 700M or 2G (ZIP / 7z only).\n\n\
-d sets the LZMA2 dictionary size, e.g. 8M (7z only; bounded by the memory budget).\n\
-s sets the maximum 7z solid block size, e.g. 256M; use 0 for non-solid blocks.\n\n\
Create formats: .zip .7z .tar .tar.gz .tar.xz .tar.zst .tar.bz2 .tar.lz4\n\
                (also .tgz .txz .tzst .tbz2)\n\
                .gz .xz .zst .bz2 .lz4 (one file)\n\
Read formats:   all of the above, plus .iso .cab .cpio .ar .lha .lzh .xar .rpm (.deb uses AR)\n\
                .rar is available only when built with the optional `rar` feature\n";

#[derive(Default)]
struct Args {
    positional: Vec<String>,
    out: Option<PathBuf>,
    password: Option<Zeroizing<String>>,
    password_source: Option<PasswordSource>,
    output_password: Option<Zeroizing<String>>,
    output_password_source: Option<PasswordSource>,
    level: Option<u32>,
    threads: Option<usize>,
    encoding: Option<String>,
    overwrite: Option<OnConflict>,
    volume_size: Option<u64>,
    dictionary_size: Option<u32>,
    solid_block_size: Option<u64>,
    hash_algorithm: Option<HashAlgorithm>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PasswordSource {
    Prompt,
    File(PathBuf),
}

fn resolve_password(
    source: Option<PasswordSource>,
    prompt: &str,
) -> std::result::Result<Option<Zeroizing<String>>, String> {
    let Some(source) = source else {
        return Ok(None);
    };
    let password = match source {
        PasswordSource::Prompt => Zeroizing::new(
            rpassword::prompt_password(prompt)
                .map_err(|error| format!("could not read password: {error}"))?,
        ),
        PasswordSource::File(path) => {
            let file = std::fs::File::open(path)
                .map_err(|_| "could not read password file".to_string())?;
            let mut password = Zeroizing::new(String::new());
            file.take(4097)
                .read_to_string(&mut password)
                .map_err(|_| "password file is not valid UTF-8".to_string())?;
            if password.len() > 4096 {
                return Err("password file exceeds 4 KiB".into());
            }
            if password.ends_with('\n') {
                password.pop();
                if password.ends_with('\r') {
                    password.pop();
                }
            }
            password
        }
    };
    Ok(Some(password))
}

fn cancel_token() -> std::result::Result<CancelToken, String> {
    static TOKEN: OnceLock<std::result::Result<CancelToken, String>> = OnceLock::new();
    TOKEN
        .get_or_init(|| {
            let token = CancelToken::new();
            let signal_token = token.clone();
            ctrlc::set_handler(move || signal_token.cancel())
                .map_err(|error| format!("cannot install Ctrl+C handler: {error}"))?;
            Ok(token)
        })
        .clone()
}

#[derive(Default)]
struct TerminalProgress {
    last_update: Option<Instant>,
    line_open: bool,
}

impl ProgressSink for TerminalProgress {
    fn update(&mut self, progress: Progress) {
        let now = Instant::now();
        let complete = (progress.bytes_total > 0 && progress.bytes_done >= progress.bytes_total)
            || (progress.files_total > 0 && progress.files_done >= progress.files_total);
        if self
            .last_update
            .is_some_and(|last| now.duration_since(last).as_millis() < 150)
            && !complete
        {
            return;
        }
        self.last_update = Some(now);
        if progress.bytes_total > 0 {
            let percent =
                (u128::from(progress.bytes_done) * 100 / u128::from(progress.bytes_total)).min(100);
            eprint!(
                "\r{percent:>3}%  {} / {} bytes",
                progress.bytes_done, progress.bytes_total
            );
            self.line_open = true;
            if progress.bytes_done >= progress.bytes_total {
                eprintln!();
                self.line_open = false;
            }
        } else {
            eprint!(
                "\r{} items, {} bytes",
                progress.files_done, progress.bytes_done
            );
            self.line_open = true;
            if progress.files_total > 0 && progress.files_done >= progress.files_total {
                eprintln!();
                self.line_open = false;
            }
        }
    }
}

impl Drop for TerminalProgress {
    fn drop(&mut self) {
        if self.line_open {
            eprintln!();
        }
    }
}

fn parse(args: &[String]) -> std::result::Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "-o" => a.out = Some(PathBuf::from(value("-o")?)),
            "-p" => {
                let value = value("-p")?;
                a.password_source = Some(if value == "-" {
                    PasswordSource::Prompt
                } else if let Some(path) = value.strip_prefix('@').filter(|path| !path.is_empty()) {
                    PasswordSource::File(PathBuf::from(path))
                } else {
                    return Err("-p accepts '-' for a hidden prompt or '@file' for a password file; literal passwords are not accepted".into());
                });
            }
            "-q" => {
                let value = value("-q")?;
                a.output_password_source = Some(if value == "-" {
                    PasswordSource::Prompt
                } else if let Some(path) = value.strip_prefix('@').filter(|path| !path.is_empty()) {
                    PasswordSource::File(PathBuf::from(path))
                } else {
                    return Err(
                        "-q accepts '-' for a hidden prompt or '@file' for an output password file"
                            .into(),
                    );
                });
            }
            "-e" => a.encoding = Some(value("-e")?),
            "-l" => {
                let v = value("-l")?;
                a.level = Some(
                    v.parse::<u32>()
                        .ok()
                        .filter(|l| *l <= 9)
                        .ok_or_else(|| format!("-l expects 0-9, got {v}"))?,
                );
            }
            "-t" => {
                let v = value("-t")?;
                a.threads = Some(
                    v.parse::<usize>()
                        .ok()
                        .filter(|t| (1..=256).contains(t))
                        .ok_or_else(|| format!("-t expects 1-256, got {v}"))?,
                );
            }
            "-v" => a.volume_size = Some(parse_size("-v", &value("-v")?)?),
            "-d" => {
                let bytes = parse_size("-d", &value("-d")?)?;
                a.dictionary_size =
                    Some(u32::try_from(bytes).map_err(|_| "-d size is out of range".to_string())?);
            }
            "-s" => {
                let size = value("-s")?;
                a.solid_block_size = Some(if size.eq_ignore_ascii_case("off") || size == "0" {
                    0
                } else {
                    parse_size("-s", &size)?
                });
            }
            "-a" => {
                a.hash_algorithm = Some(match value("-a")?.to_ascii_lowercase().as_str() {
                    "sha256" | "sha-256" => HashAlgorithm::Sha256,
                    "blake3" => HashAlgorithm::Blake3,
                    other => return Err(format!("-a expects sha256 or blake3, got {other}")),
                })
            }
            "-y" => a.overwrite = Some(OnConflict::Replace),
            "-n" => a.overwrite = Some(OnConflict::Skip),
            f if f.starts_with('-') && f.len() > 1 => return Err(format!("unknown option {f}")),
            _ => a.positional.push(arg.clone()),
        }
    }
    Ok(a)
}

fn parse_size(flag: &str, value: &str) -> std::result::Result<u64, String> {
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let (digits, suffix) = value.split_at(split);
    let number = digits
        .parse::<u64>()
        .map_err(|_| format!("{flag} expects a positive byte size, got {value}"))?;
    let multiplier = match suffix.to_ascii_lowercase().as_str() {
        "" => 1,
        "k" | "kb" => 1u64 << 10,
        "m" | "mb" => 1u64 << 20,
        "g" | "gb" => 1u64 << 30,
        _ => return Err(format!("{flag} suffix must be K, M, or G, got {value}")),
    };
    number
        .checked_mul(multiplier)
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("{flag} size is out of range: {value}"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("rz: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> std::result::Result<(), String> {
    let Some((cmd, rest)) = args.split_first() else {
        print!("{USAGE}");
        return Ok(());
    };
    if matches!(cmd.as_str(), "-h" | "--help" | "help") {
        print!("{USAGE}");
        return Ok(());
    }
    let mut a = parse(rest)?;
    if cmd == "h" {
        if a.password_source.is_some() || a.output_password_source.is_some() {
            return Err("hashing does not use an archive password".into());
        }
        return hash_cmd(&a);
    }
    let recover = match cmd.as_str() {
        "c" | "convert" => Some(false),
        "r" | "repair" => Some(true),
        _ => None,
    };
    if let Some(recover) = recover {
        if a.out.is_some() || a.overwrite.is_some() {
            return Err("-o, -y and -n do not apply to conversion or repair".into());
        }
        if a.volume_size.is_some() {
            return Err("volume output is not supported for conversion or repair".into());
        }
        if a.positional.len() != 2 {
            return Err(format!(
                "{} requires a source archive and a new output path\n\n{USAGE}",
                if recover { "repair" } else { "convert" }
            ));
        }
        a.password = resolve_password(a.password_source.take(), "Source password: ")?;
        a.output_password = resolve_password(a.output_password_source.take(), "Output password: ")?;
        return transform_cmd(&a, recover).map_err(|error| error.to_string());
    }
    if a.output_password_source.is_some() {
        return Err("-q is only used by convert and repair".into());
    }
    if !matches!(cmd.as_str(), "a" | "l" | "x" | "t") {
        return Err(format!("unknown command {cmd}\n\n{USAGE}"));
    }
    let Some(archive) = a.positional.first().map(PathBuf::from) else {
        return Err(format!("{cmd}: missing archive path\n\n{USAGE}"));
    };
    if cmd == "a" && a.positional.len() == 1 {
        return Err(Error::Unsupported("nothing to add".into()).to_string());
    }
    a.password = resolve_password(a.password_source.take(), "Password: ")?;
    let result = match cmd.as_str() {
        "a" => add(&archive, &a),
        "l" => list(&archive, &a),
        "x" => extract_cmd(&archive, &a),
        "t" => test(&archive, &a),
        _ => Err(Error::Unsupported(format!("unknown command {cmd}"))),
    };
    result.map_err(|e| e.to_string())
}

fn hash_cmd(args: &Args) -> std::result::Result<(), String> {
    if args.positional.is_empty() {
        return Err(format!("h: missing file path\n\n{USAGE}"));
    }
    let algorithms = args.hash_algorithm.map_or_else(
        || vec![HashAlgorithm::Sha256, HashAlgorithm::Blake3],
        |algorithm| vec![algorithm],
    );
    let cancel = cancel_token()?;
    let mut progress = TerminalProgress::default();
    for item in &args.positional {
        let path = PathBuf::from(item);
        let results = rz_app::hash::hash_file(&path, &algorithms, &cancel, &mut progress)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        for result in results {
            println!("{}  {}  {}", result.algorithm.name(), result.hex, item);
        }
    }
    Ok(())
}

fn add(archive: &Path, a: &Args) -> Result<()> {
    let sources: Vec<PathBuf> = a.positional[1..].iter().map(PathBuf::from).collect();
    if sources.is_empty() {
        return Err(Error::Unsupported("nothing to add".into()));
    }
    let opts = CreateOptions {
        level: a.level.unwrap_or(6),
        password: a
            .password
            .as_ref()
            .map(|password| password.as_str().to_owned()),
        threads: a.threads.unwrap_or(0),
        volume_size: a.volume_size,
        dictionary_size: a.dictionary_size,
        solid_block_size: a.solid_block_size,
        ..CreateOptions::default()
    };
    let cancel = cancel_token().map_err(|error| Error::Io(std::io::Error::other(error)))?;
    let mut progress = TerminalProgress::default();
    let rep = archive_ops::create_archive(&sources, archive, &opts, &cancel, &mut progress)?;
    println!(
        "{}: {} files, {} dirs, {} bytes ({} skipped)",
        archive.display(),
        rep.files,
        rep.dirs,
        rep.bytes,
        rep.skipped
    );
    Ok(())
}

fn list(path: &Path, a: &Args) -> Result<()> {
    let cancel = cancel_token().map_err(|error| Error::Io(std::io::Error::other(error)))?;
    let mut progress = TerminalProgress::default();
    let listing = archive_ops::list_archive(
        path,
        a.password.as_ref().map(|password| password.as_str()),
        a.encoding.as_deref(),
        &cancel,
        &mut progress,
    )?;
    let mut total = 0u64;
    println!("{:>12}  Name", "Size");
    for e in &listing.entries {
        total += e.size;
        let lock = if e.encrypted { " *" } else { "" };
        let slash = if e.kind == EntryKind::Dir { "/" } else { "" };
        println!(
            "{:>12}  {}{slash}{lock}",
            e.size,
            e.path.trim_end_matches('/')
        );
    }
    println!(
        "{:>12}  {} entries ({})",
        total,
        listing.entries.len(),
        listing.format
    );
    Ok(())
}

fn extract_cmd(archive: &Path, a: &Args) -> Result<()> {
    let dest = a.out.clone().unwrap_or_else(|| PathBuf::from("."));
    let cancel = cancel_token().map_err(|error| Error::Io(std::io::Error::other(error)))?;
    let mut progress = TerminalProgress::default();
    let rep = archive_ops::extract_archive(
        archive,
        &dest,
        archive_ops::CliExtractOptions {
            password: a
                .password
                .as_ref()
                .map(|password| password.as_str().to_owned()),
            encoding: a.encoding.clone(),
            threads: a.threads.unwrap_or(0),
            conflict: a.overwrite.unwrap_or(OnConflict::Rename),
        },
        &cancel,
        &mut progress,
    )?;
    println!(
        "{} files, {} dirs, {} bytes ({} skipped)",
        rep.files, rep.dirs, rep.bytes, rep.skipped
    );
    Ok(())
}

/// Reads every file through the decoder and checks its checksum, writing nothing.
fn test(path: &Path, a: &Args) -> Result<()> {
    let cancel = cancel_token().map_err(|error| Error::Io(std::io::Error::other(error)))?;
    let mut progress = TerminalProgress::default();
    let report = archive_ops::test_archive(
        path,
        a.password.as_ref().map(|password| password.as_str()),
        a.encoding.as_deref(),
        &cancel,
        &mut progress,
    )?;
    println!(
        "OK: {} entries, {} bytes verified",
        report.entries, report.bytes
    );
    Ok(())
}

fn transform_cmd(a: &Args, recover: bool) -> Result<()> {
    let source = Path::new(&a.positional[0]);
    let output = Path::new(&a.positional[1]);
    let cancel = cancel_token().map_err(|error| Error::Io(std::io::Error::other(error)))?;
    let mut progress = TerminalProgress::default();
    let report = archive_ops::transform_archive(
        source,
        output,
        archive_ops::CliTransformOptions {
            input_password: a
                .password
                .as_ref()
                .map(|password| Zeroizing::new(password.as_str().to_owned())),
            output_password: a
                .output_password
                .as_ref()
                .map(|password| Zeroizing::new(password.as_str().to_owned())),
            encoding: a.encoding.clone(),
            threads: a.threads.unwrap_or(0),
            level: a.level.unwrap_or(6),
            dictionary_size: a.dictionary_size,
            solid_block_size: a.solid_block_size,
        },
        recover,
        &cancel,
        &mut progress,
    )?;
    if recover {
        println!(
            "Recovered {} items, {} bytes; skipped {} -> {}",
            report.files,
            report.bytes,
            report.skipped,
            output.display()
        );
        for issue in report.issues {
            eprintln!("warning: {issue}");
        }
    } else {
        println!(
            "Converted {} items, {} bytes -> {}",
            report.files,
            report.bytes,
            output.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_flags_and_positionals() {
        let a = parse(&s(&[
            "x.zip",
            "-o",
            "out",
            "-p",
            "@password",
            "-q",
            "@output-password",
            "-l",
            "9",
            "-t",
            "4",
            "-v",
            "700M",
            "-d",
            "8M",
            "-s",
            "256M",
            "-y",
        ]))
        .unwrap_or_default();
        assert_eq!(a.positional, vec!["x.zip"]);
        assert_eq!(a.out, Some(PathBuf::from("out")));
        assert_eq!(
            a.password_source,
            Some(PasswordSource::File(PathBuf::from("password")))
        );
        assert_eq!(
            a.output_password_source,
            Some(PasswordSource::File(PathBuf::from("output-password")))
        );
        assert_eq!(a.level, Some(9));
        assert_eq!(a.threads, Some(4));
        assert_eq!(a.overwrite, Some(OnConflict::Replace));
        assert_eq!(a.volume_size, Some(700 * 1024 * 1024));
        assert_eq!(a.dictionary_size, Some(8 * 1024 * 1024));
        assert_eq!(a.solid_block_size, Some(256 * 1024 * 1024));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&s(&["-l", "10"])).is_err());
        assert!(parse(&s(&["-l", "x"])).is_err());
        assert!(parse(&s(&["-t", "0"])).is_err());
        assert!(parse(&s(&["-o"])).is_err());
        assert!(parse(&s(&["-v", "0"])).is_err());
        assert!(parse(&s(&["-v", "1TB"])).is_err());
        assert!(parse(&s(&["-d", "64G"])).is_err());
        assert_eq!(
            parse(&s(&["-s", "off"]))
                .unwrap_or_default()
                .solid_block_size,
            Some(0)
        );
        assert!(parse(&s(&["-a", "md5"])).is_err());
        assert!(parse(&s(&["-p", "literal-secret"])).is_err());
        assert!(parse(&s(&["-q", "literal-secret"])).is_err());
        assert_eq!(
            parse(&s(&["-p", "-"])).unwrap_or_default().password_source,
            Some(PasswordSource::Prompt)
        );
        assert!(parse(&s(&["--nope"])).is_err());
    }
}
