#![no_main]

use hayro::hayro_syntax::Pdf;
use libfuzzer_sys::fuzz_target;
use std::sync::Arc;

const MAX_PDF_INPUT: usize = 16 * 1024 * 1024;

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > MAX_PDF_INPUT || !bytes.starts_with(b"%PDF-") {
        return;
    }

    if let Ok(pdf) = Pdf::new(Arc::new(bytes.to_vec())) {
        let _ = pdf.pages().first().map(|page| page.render_dimensions());
    }
});
