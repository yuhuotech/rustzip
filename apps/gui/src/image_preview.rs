use image::ImageReader;
use std::io::Cursor;

/// Keep one decoded RGBA preview below roughly 64 MiB.
const MAX_PREVIEW_PIXELS: u64 = 16_777_216;

/// Reads dimensions from a bounded image header before the renderer decodes pixels.
pub(crate) fn dimensions_within_limit(data: &[u8]) -> bool {
    let Ok(reader) = ImageReader::new(Cursor::new(data)).with_guessed_format() else {
        return false;
    };
    let Some(format) = reader.format() else {
        return false;
    };
    if !matches!(
        format,
        image::ImageFormat::Bmp
            | image::ImageFormat::Gif
            | image::ImageFormat::Jpeg
            | image::ImageFormat::Png
            | image::ImageFormat::WebP
    ) {
        return false;
    }
    let Ok((width, height)) = reader.into_dimensions() else {
        return false;
    };
    width > 0
        && height > 0
        && u64::from(width)
            .checked_mul(u64::from(height))
            .is_some_and(|pixels| pixels <= MAX_PREVIEW_PIXELS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bmp_header(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0; 26];
        bytes[..2].copy_from_slice(b"BM");
        bytes[2..6].copy_from_slice(&26u32.to_le_bytes());
        bytes[10..14].copy_from_slice(&26u32.to_le_bytes());
        bytes[14..18].copy_from_slice(&12u32.to_le_bytes());
        bytes[18..20].copy_from_slice(&width.to_le_bytes());
        bytes[20..22].copy_from_slice(&height.to_le_bytes());
        bytes[22..24].copy_from_slice(&1u16.to_le_bytes());
        bytes[24..26].copy_from_slice(&24u16.to_le_bytes());
        bytes
    }

    #[test]
    fn preview_dimensions_are_bounded_before_pixel_decode() {
        assert!(dimensions_within_limit(&bmp_header(4096, 4096)));
        assert!(!dimensions_within_limit(&bmp_header(4097, 4096)));
        assert!(!dimensions_within_limit(&bmp_header(u16::MAX, u16::MAX)));
        assert!(!dimensions_within_limit(&bmp_header(0, 4096)));
    }

    #[test]
    fn unknown_or_malformed_images_are_not_previewed() {
        assert!(!dimensions_within_limit(b"not an image"));
        assert!(!dimensions_within_limit(b"GIF89a"));
    }
}
