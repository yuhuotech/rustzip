use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
use mp4::{MediaType, Mp4Reader};
use rust_h264::decoder::{Decoder, Frame};
use rust_h264::nal::{parse_avcc, parse_avcc_config, parse_sps_info};
use std::io::{self, Cursor, Read, Seek, SeekFrom};

pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 16_777_216;
pub const MAX_AXIS: u32 = 8_192;
const MAX_SAMPLE_BYTES: u32 = 8 * 1024 * 1024;
const MAX_MP4_IO_OPERATIONS: usize = 16_384;
const MAX_REFERENCE_FRAMES: u32 = 16;

struct BoundedCursor<'a> {
    cursor: Cursor<&'a [u8]>,
    operations: usize,
}

impl<'a> BoundedCursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            cursor: Cursor::new(data),
            operations: 0,
        }
    }

    fn charge(&mut self) -> io::Result<()> {
        self.operations = self
            .operations
            .checked_add(1)
            .ok_or_else(|| io::Error::other("MP4 parse operation count overflow"))?;
        if self.operations > MAX_MP4_IO_OPERATIONS {
            return Err(io::Error::other("MP4 parse operation limit exceeded"));
        }
        Ok(())
    }
}

impl Read for BoundedCursor<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.charge()?;
        self.cursor.read(buffer)
    }
}

impl Seek for BoundedCursor<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.charge()?;
        self.cursor.seek(position)
    }
}

fn avcc_record(track: &mp4::Mp4Track) -> Result<Vec<u8>, &'static str> {
    let avc1 = track
        .trak
        .mdia
        .minf
        .stbl
        .stsd
        .avc1
        .as_ref()
        .ok_or("track is not H.264")?;
    let avcc = &avc1.avcc;
    if avcc.configuration_version != 1
        || avcc.sequence_parameter_sets.is_empty()
        || avcc.picture_parameter_sets.is_empty()
        || avcc.sequence_parameter_sets.len() > 31
        || avcc.picture_parameter_sets.len() > 255
        || avcc.length_size_minus_one > 3
    {
        return Err("unsupported H.264 decoder configuration");
    }

    let total = avcc
        .sequence_parameter_sets
        .iter()
        .chain(&avcc.picture_parameter_sets)
        .try_fold(7usize, |size, nal| size.checked_add(2 + nal.bytes.len()))
        .ok_or("H.264 decoder configuration is too large")?;
    if total > MAX_INPUT_BYTES {
        return Err("H.264 decoder configuration exceeds the preview limit");
    }

    let mut record = Vec::with_capacity(total);
    record.extend_from_slice(&[
        avcc.configuration_version,
        avcc.avc_profile_indication,
        avcc.profile_compatibility,
        avcc.avc_level_indication,
        0xfc | avcc.length_size_minus_one,
        0xe0 | avcc.sequence_parameter_sets.len() as u8,
    ]);
    for nal in &avcc.sequence_parameter_sets {
        let length = u16::try_from(nal.bytes.len()).map_err(|_| "H.264 SPS is too large")?;
        record.extend_from_slice(&length.to_be_bytes());
        record.extend_from_slice(&nal.bytes);
    }
    record.push(avcc.picture_parameter_sets.len() as u8);
    for nal in &avcc.picture_parameter_sets {
        let length = u16::try_from(nal.bytes.len()).map_err(|_| "H.264 PPS is too large")?;
        record.extend_from_slice(&length.to_be_bytes());
        record.extend_from_slice(&nal.bytes);
    }
    Ok(record)
}

fn validate_configuration(config: &rust_h264::nal::AvccConfig<'_>) -> Result<(), &'static str> {
    for sps in &config.sps_nals {
        let info = parse_sps_info(sps).map_err(|_| "H.264 sequence header is invalid")?;
        let coded_pixels = u64::from(info.coded_width)
            .checked_mul(u64::from(info.coded_height))
            .ok_or("H.264 frame dimensions overflow")?;
        let output_pixels = u64::from(info.width)
            .checked_mul(u64::from(info.height))
            .ok_or("H.264 frame dimensions overflow")?;
        if info.width == 0
            || info.height == 0
            || info.width > MAX_AXIS
            || info.height > MAX_AXIS
            || info.coded_width > MAX_AXIS
            || info.coded_height > MAX_AXIS
            || coded_pixels > MAX_PIXELS
            || output_pixels > MAX_PIXELS
        {
            return Err("H.264 frame dimensions exceed the preview limit");
        }
        if info.chroma_format != 1
            || info.bit_depth_luma != 8
            || info.bit_depth_chroma != 8
            || !info.progressive
            || info.reference_frames > MAX_REFERENCE_FRAMES
            || info.max_reorder_frames.unwrap_or(0) > MAX_REFERENCE_FRAMES
        {
            return Err("this H.264 profile is not supported for preview");
        }
    }
    Ok(())
}

fn first_sync_sample(track: &mp4::Mp4Track) -> Result<(u32, u32), &'static str> {
    let sample_count = track.sample_count();
    if sample_count == 0 {
        return Err("MP4 video track has no samples");
    }
    let sample_id = match &track.trak.mdia.minf.stbl.stss {
        Some(sync_samples) => sync_samples
            .entries
            .iter()
            .copied()
            .min()
            .ok_or("MP4 video track has no keyframe")?,
        None => 1,
    };
    if sample_id == 0 || sample_id > sample_count {
        return Err("MP4 keyframe index is invalid");
    }
    let sizes = &track.trak.mdia.minf.stbl.stsz;
    let sample_size = if sizes.sample_size > 0 {
        sizes.sample_size
    } else {
        *sizes
            .sample_sizes
            .get(sample_id as usize - 1)
            .ok_or("MP4 keyframe size is missing")?
    };
    if sample_size == 0 || sample_size > MAX_SAMPLE_BYTES {
        return Err("MP4 keyframe exceeds the preview input limit");
    }
    Ok((sample_id, sample_size))
}

fn yuv420_to_png(frame: Frame) -> Result<Vec<u8>, &'static str> {
    let (width, height) = (frame.width, frame.height);
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or("H.264 frame dimensions overflow")?;
    if width == 0
        || height == 0
        || width > MAX_AXIS
        || height > MAX_AXIS
        || pixels > MAX_PIXELS
        || width % 2 != 0
        || height % 2 != 0
    {
        return Err("H.264 frame dimensions exceed the preview limit");
    }
    let luma_len = usize::try_from(pixels).map_err(|_| "H.264 frame is too large")?;
    let chroma_len = luma_len / 4;
    if frame.y.len() != luma_len || frame.u.len() != chroma_len || frame.v.len() != chroma_len {
        return Err("H.264 decoder returned an invalid frame");
    }
    let rgb_len = luma_len.checked_mul(3).ok_or("H.264 frame is too large")?;
    let mut rgb = Vec::with_capacity(rgb_len);
    let width_usize = width as usize;
    for y in 0..height as usize {
        for x in 0..width_usize {
            let luma = (f32::from(frame.y[y * width_usize + x]) - 16.0).max(0.0);
            let chroma_index = (y / 2) * (width_usize / 2) + (x / 2);
            let cb = f32::from(frame.u[chroma_index]) - 128.0;
            let cr = f32::from(frame.v[chroma_index]) - 128.0;
            let c = 1.164 * luma;
            rgb.push((c + 1.596 * cr).round().clamp(0.0, 255.0) as u8);
            rgb.push((c - 0.392 * cb - 0.813 * cr).round().clamp(0.0, 255.0) as u8);
            rgb.push((c + 2.017 * cb).round().clamp(0.0, 255.0) as u8);
        }
    }

    let image = ImageBuffer::<Rgb<u8>, _>::from_raw(width, height, rgb)
        .ok_or("H.264 frame could not be converted to RGB")?;
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|_| "video preview PNG encoding failed")?;
    if output.get_ref().len() > MAX_OUTPUT_BYTES {
        return Err("video preview image exceeds the output limit");
    }
    Ok(output.into_inner())
}

/// Decodes the first progressive 8-bit 4:2:0 H.264 keyframe from an MP4 file.
pub fn render_first_frame(input: &[u8]) -> Result<Vec<u8>, &'static str> {
    if input.len() > MAX_INPUT_BYTES {
        return Err("video source exceeds the preview input limit");
    }
    if input.len() < 8 || &input[4..8] != b"ftyp" {
        return Err("input is not an MP4 video");
    }

    let mut reader = Mp4Reader::read_header(BoundedCursor::new(input), input.len() as u64)
        .map_err(|_| "MP4 container parsing failed")?;
    if reader.is_fragmented() {
        return Err("fragmented MP4 video is not supported for preview");
    }
    let track_id = reader
        .tracks()
        .iter()
        .filter_map(|(id, track)| matches!(track.media_type(), Ok(MediaType::H264)).then_some(*id))
        .min()
        .ok_or("MP4 contains no supported H.264 video track")?;
    let (sample_id, sample_size, avcc_record) = {
        let track = reader
            .tracks()
            .get(&track_id)
            .ok_or("MP4 H.264 track disappeared")?;
        let (sample_id, sample_size) = first_sync_sample(track)?;
        (sample_id, sample_size, avcc_record(track)?)
    };
    if sample_size as usize > input.len() {
        return Err("MP4 keyframe size exceeds the source file");
    }

    let config =
        parse_avcc_config(&avcc_record).map_err(|_| "H.264 decoder configuration is invalid")?;
    validate_configuration(&config)?;
    let mut decoder = Decoder::new();
    for nal in config.sps_nals.iter().chain(&config.pps_nals) {
        decoder
            .decode_nal(nal)
            .map_err(|_| "H.264 parameter set could not be decoded")?;
    }

    let sample = reader
        .read_sample(track_id, sample_id)
        .map_err(|_| "MP4 keyframe could not be read")?
        .ok_or("MP4 keyframe is missing")?;
    if sample.bytes.len() != sample_size as usize {
        return Err("MP4 keyframe size does not match its table");
    }
    let mut frame = None;
    for nal in parse_avcc(sample.bytes.as_ref(), config.length_size) {
        if let Some(decoded) = decoder
            .decode_nal(&nal)
            .map_err(|_| "H.264 keyframe could not be decoded")?
        {
            frame = Some(decoded);
            break;
        }
    }
    let frame = frame
        .or_else(|| decoder.flush())
        .ok_or("H.264 keyframe had no frame")?;
    yuv420_to_png(frame)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use image::GenericImageView as _;

    #[test]
    fn renders_the_first_frame_of_a_small_mp4() {
        let mp4 = include_bytes!("../tests/fixtures/small-h264.mp4");
        assert_first_frame_is_red(mp4);
    }

    #[test]
    fn renders_the_first_frame_of_a_small_quicktime_movie() {
        let mov = include_bytes!("../tests/fixtures/small-h264.mov");
        assert_first_frame_is_red(mov);
    }

    #[test]
    fn rejects_an_mp4_with_a_nonadvancing_nested_box() {
        let malformed = include_bytes!("../tests/fixtures/mp4-box-loop.mp4");
        assert!(render_first_frame(malformed).is_err());
    }

    fn assert_first_frame_is_red(video: &[u8]) {
        let png = render_first_frame(video).unwrap();
        let image = image::load_from_memory(&png).unwrap();
        assert_eq!(image.dimensions(), (64, 64));
        let pixel = image.to_rgb8().get_pixel(32, 32).0;
        assert!(pixel[0] > pixel[1]);
        assert!(pixel[0] > pixel[2]);
    }

    #[test]
    fn rejects_non_mp4_and_oversized_video_inputs() {
        assert!(render_first_frame(b"not an mp4").is_err());
        assert!(render_first_frame(&vec![0; MAX_INPUT_BYTES + 1]).is_err());
    }
}
