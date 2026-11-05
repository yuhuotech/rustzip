use std::io::{self, Read};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dmg_apfs_compressed.dmg");
    let iterations = std::env::var("RZ_APFS_BENCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(1000)
        .max(1);
    let expected = b"The quick brown fox jumps over the lazy dog. ".repeat(4000);
    let mut reader = rz_formats::open(&path)?;
    let entry = reader
        .entries()?
        .into_iter()
        .find(|entry| entry.path == "compressed.txt")
        .ok_or_else(|| io::Error::other("APFS benchmark file missing"))?;
    let start = Instant::now();
    let mut output = Vec::with_capacity(expected.len());
    for _ in 0..iterations {
        output.clear();
        reader.open(&entry)?.read_to_end(&mut output)?;
        if output != expected {
            return Err(io::Error::other("APFS benchmark output mismatch").into());
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    let mib = expected.len() as f64 * f64::from(iterations) / 1_048_576.0;
    println!(
        "APFS LZVN resource fork: {iterations} reads, {mib:.1} MiB, {seconds:.3} s, {:.1} MiB/s",
        mib / seconds
    );
    Ok(())
}
