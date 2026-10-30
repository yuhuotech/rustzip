#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

use std::io::{Read, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::{render, RenderCache, RenderSettings};
use rz_preview_budget::BudgetAllocator;

const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_777_216;
const MAX_AXIS: f64 = 8192.0;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(10);

#[global_allocator]
static ALLOCATOR: BudgetAllocator = BudgetAllocator::new();

fn read_bounded_pdf() -> Result<Vec<u8>, &'static str> {
    let stdin = std::io::stdin();
    let mut input = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut reader = stdin.lock();
    loop {
        let read = reader.read(&mut buffer).map_err(|_| "input read failed")?;
        if read == 0 {
            break;
        }
        if input.len().saturating_add(read) > MAX_INPUT_BYTES {
            return Err("PDF source exceeds the preview input limit");
        }
        input.extend_from_slice(&buffer[..read]);
    }
    if !input.starts_with(b"%PDF-") {
        return Err("input is not a PDF document");
    }
    Ok(input)
}

fn output_dimensions(width: f64, height: f64) -> Result<(f64, u16, u16), &'static str> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("PDF page dimensions are invalid");
    }

    let scale = (MAX_PIXELS as f64 / (width * height))
        .sqrt()
        .min(MAX_AXIS / width)
        .min(MAX_AXIS / height)
        .min(1.0);
    if !scale.is_finite() || scale <= 0.0 {
        return Err("PDF page dimensions exceed the preview limit");
    }
    let output_width = (width * scale).floor().clamp(1.0, MAX_AXIS) as u16;
    let output_height = (height * scale).floor().clamp(1.0, MAX_AXIS) as u16;
    if u64::from(output_width) * u64::from(output_height) > MAX_PIXELS {
        return Err("PDF page exceeds the preview pixel limit");
    }
    Ok((scale, output_width, output_height))
}

fn render_first_page(input: Vec<u8>) -> Result<Vec<u8>, &'static str> {
    let pdf = Pdf::new(Arc::new(input)).map_err(|_| "PDF parsing failed")?;
    let page = pdf.pages().first().ok_or("PDF has no pages")?;
    let (width, height) = page.render_dimensions();
    let (width, height) = (f64::from(width), f64::from(height));
    let (scale, output_width, output_height) = output_dimensions(width, height)?;

    let cache = RenderCache::new();
    let pixmap = render(
        page,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale as f32,
            y_scale: scale as f32,
            width: Some(output_width),
            height: Some(output_height),
            ..Default::default()
        },
    );
    let png = pixmap.into_png().map_err(|_| "PDF image encoding failed")?;
    if png.len() > MAX_OUTPUT_BYTES {
        return Err("PDF preview image exceeds the output limit");
    }
    Ok(png)
}

fn run() -> Result<(), &'static str> {
    let input = read_bounded_pdf()?;
    let png = render_first_page(input)?;
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    writer
        .write_all(&png)
        .map_err(|_| "preview output failed")?;
    writer.flush().map_err(|_| "preview output failed")
}

fn main() -> ExitCode {
    if std::thread::Builder::new()
        .name("rustzip-pdf-time-limit".into())
        .spawn(|| {
            std::thread::sleep(PROCESS_TIMEOUT);
            eprintln!("PDF rendering exceeded the process time limit");
            std::process::exit(1);
        })
        .is_err()
    {
        eprintln!("PDF rendering time limit could not be started");
        return ExitCode::FAILURE;
    }

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("{reason}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn dimensions_scale_before_allocating_a_large_square_page() {
        let (scale, width, height) = output_dimensions(1_000_000.0, 1_000_000.0).unwrap();
        assert_eq!((width, height), (4096, 4096));
        assert!(scale.is_finite() && scale > 0.0);
    }
}
