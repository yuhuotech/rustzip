//! `rz` command line. Mirrors the 7z verbs: `a` add, `l` list, `x` extract, `t` test.

use rz_core::{CancelToken, CreateOptions, EntryKind, Error, NoProgress, Result};
use rz_engine::{compress, extract, ExtractOptions, Overwrite};
use rz_formats::OutputFormat;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "RustZIP\n\n\
Usage:\n  \
rz a <archive> <path>... [-l 0-9] [-p password]   create (.zip .tar .tar.gz .tgz)\n  \
rz l <archive>                                    list contents\n  \
rz x <archive> [-o dir] [-p password] [-y|-n]     extract (-y replace, -n skip existing)\n  \
rz t <archive> [-p password]                      test integrity\n";

#[derive(Default)]
struct Args {
    positional: Vec<String>,
    out: Option<PathBuf>,
    password: Option<String>,
    level: Option<u32>,
    overwrite: Option<Overwrite>,
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
            "-p" => a.password = Some(value("-p")?),
            "-l" => {
                let v = value("-l")?;
                a.level = Some(
                    v.parse::<u32>()
                        .ok()
                        .filter(|l| *l <= 9)
                        .ok_or_else(|| format!("-l expects 0-9, got {v}"))?,
                );
            }
            "-y" => a.overwrite = Some(Overwrite::Replace),
            "-n" => a.overwrite = Some(Overwrite::Skip),
            f if f.starts_with('-') && f.len() > 1 => return Err(format!("unknown option {f}")),
            _ => a.positional.push(arg.clone()),
        }
    }
    Ok(a)
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
    let a = parse(rest)?;
    let Some(archive) = a.positional.first().map(PathBuf::from) else {
        return Err(format!("{cmd}: missing archive path\n\n{USAGE}"));
    };
    let result = match cmd.as_str() {
        "a" => add(&archive, &a),
        "l" => list(&archive),
        "x" => extract_cmd(&archive, &a),
        "t" => test(&archive, &a),
        other => return Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    result.map_err(|e| e.to_string())
}

fn add(archive: &Path, a: &Args) -> Result<()> {
    let sources: Vec<PathBuf> = a.positional[1..].iter().map(PathBuf::from).collect();
    if sources.is_empty() {
        return Err(Error::Unsupported("nothing to add".into()));
    }
    let format = OutputFormat::from_path(archive).ok_or_else(|| {
        Error::Unsupported("cannot create this format; use .zip, .tar, .tar.gz or .tgz".into())
    })?;
    let opts = CreateOptions {
        level: a.level.unwrap_or(6),
        password: a.password.clone(),
    };
    let rep = compress(
        &sources,
        archive,
        format,
        &opts,
        &CancelToken::new(),
        &mut NoProgress,
    )?;
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

fn list(path: &Path) -> Result<()> {
    let mut r = rz_formats::open(path)?;
    let entries = r.entries()?;
    let mut total = 0u64;
    println!("{:>12}  Name", "Size");
    for e in &entries {
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
        entries.len(),
        r.format_name()
    );
    Ok(())
}

fn extract_cmd(archive: &Path, a: &Args) -> Result<()> {
    let dest = a.out.clone().unwrap_or_else(|| PathBuf::from("."));
    let mut r = rz_formats::open(archive)?;
    let opts = ExtractOptions {
        overwrite: a.overwrite.unwrap_or(Overwrite::Error),
        password: a.password.clone(),
        ..ExtractOptions::default()
    };
    let rep = extract(&mut *r, &dest, &opts, &CancelToken::new(), &mut NoProgress)?;
    println!(
        "{} files, {} dirs, {} bytes ({} skipped)",
        rep.files, rep.dirs, rep.bytes, rep.skipped
    );
    Ok(())
}

fn test(path: &Path, a: &Args) -> Result<()> {
    let mut r = rz_formats::open(path)?;
    if let Some(pw) = &a.password {
        r.set_password(pw)?;
    }
    let entries = r.entries()?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut bytes = 0u64;
    for e in entries.iter().filter(|e| e.kind == EntryKind::File) {
        let mut src = r.open(e)?;
        loop {
            let n = src
                .read(&mut buf)
                .map_err(|err| Error::Corrupt(format!("{}: {err}", e.path)))?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
        }
    }
    println!("OK: {} entries, {} bytes verified", entries.len(), bytes);
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
        let a = parse(&s(&["x.zip", "-o", "out", "-p", "pw", "-l", "9", "-y"])).ok();
        let a = a.unwrap_or_default();
        assert_eq!(a.positional, vec!["x.zip"]);
        assert_eq!(a.out, Some(PathBuf::from("out")));
        assert_eq!(a.password.as_deref(), Some("pw"));
        assert_eq!(a.level, Some(9));
        assert_eq!(a.overwrite, Some(Overwrite::Replace));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&s(&["-l", "10"])).is_err());
        assert!(parse(&s(&["-l", "x"])).is_err());
        assert!(parse(&s(&["-o"])).is_err());
        assert!(parse(&s(&["--nope"])).is_err());
    }
}
