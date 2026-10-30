#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

use rz_preview_budget::BudgetAllocator;
use std::io::{Read, Write};
use std::process::ExitCode;
use std::time::Duration;

const PROCESS_TIMEOUT: Duration = Duration::from_secs(10);

#[global_allocator]
static ALLOCATOR: BudgetAllocator = BudgetAllocator::new();

fn read_bounded_input() -> Result<Vec<u8>, &'static str> {
    let stdin = std::io::stdin();
    let mut input = Vec::new();
    let mut limited = stdin
        .lock()
        .take(rz_video_renderer::MAX_INPUT_BYTES as u64 + 1);
    limited
        .read_to_end(&mut input)
        .map_err(|_| "video input read failed")?;
    if input.len() > rz_video_renderer::MAX_INPUT_BYTES {
        return Err("video source exceeds the preview input limit");
    }
    Ok(input)
}

fn run() -> Result<(), &'static str> {
    let input = read_bounded_input()?;
    let png = rz_video_renderer::render_first_frame(&input)?;
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    writer
        .write_all(&png)
        .map_err(|_| "video preview output failed")?;
    writer.flush().map_err(|_| "video preview output failed")
}

fn main() -> ExitCode {
    if std::thread::Builder::new()
        .name("rustzip-video-time-limit".into())
        .spawn(|| {
            std::thread::sleep(PROCESS_TIMEOUT);
            eprintln!("video preview exceeded the process time limit");
            std::process::exit(1);
        })
        .is_err()
    {
        eprintln!("video preview time limit could not be started");
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
