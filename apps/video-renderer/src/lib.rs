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
const MAX_MP4_BOX_DEPTH: usize = 16;
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

fn read_be_u32(input: &[u8], offset: usize, end: usize) -> Result<u32, &'static str> {
    let bytes = input
        .get(offset..offset.checked_add(4).ok_or("MP4 box offset overflow")?)
        .filter(|_| offset.saturating_add(4) <= end)
        .ok_or("MP4 box header is truncated")?;
    let bytes: [u8; 4] = bytes
        .try_into()
        .map_err(|_| "MP4 box header is truncated")?;
    Ok(u32::from_be_bytes(bytes))
}

fn child_box_prefix(
    input: &[u8],
    box_type: [u8; 4],
    parent_type: Option<[u8; 4]>,
    payload_start: usize,
    box_end: usize,
) -> Result<Option<usize>, &'static str> {
    let prefix = match &box_type {
        b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"dinf" | b"mvex" | b"moof"
        | b"traf" | b"udta" | b"ilst" | b"wave" => Some(0),
        b"dref" | b"stsd" => Some(8),
        b"avc1" | b"hev1" | b"vp09" => Some(78),
        b"mp4a" => Some(28),
        _ if parent_type == Some(*b"ilst")
            && [*b"\xA9nam", *b"\xA9day", *b"covr", *b"desc"].contains(&box_type) =>
        {
            Some(0)
        }
        _ => None,
    };
    if box_type == *b"meta" {
        let version_and_flags = read_be_u32(input, payload_start, box_end)?;
        if version_and_flags == 0 {
            return Ok(Some(4));
        }
        let possible_hdlr =
            input.get(payload_start.saturating_add(4)..payload_start.saturating_add(8));
        if possible_hdlr == Some(b"hdlr") {
            return Ok(Some(0));
        }
        return Err("MP4 metadata box header is invalid");
    }
    Ok(prefix)
}

fn validate_box_region(
    input: &[u8],
    start: usize,
    end: usize,
    depth: usize,
    parent_type: Option<[u8; 4]>,
    box_count: &mut usize,
) -> Result<(), &'static str> {
    if depth > MAX_MP4_BOX_DEPTH || start > end || end > input.len() {
        return Err("MP4 box tree exceeds the preview limit");
    }

    let mut offset = start;
    while offset < end {
        *box_count = box_count.checked_add(1).ok_or("MP4 box count overflow")?;
        if *box_count > MAX_MP4_IO_OPERATIONS {
            return Err("MP4 box count exceeds the preview limit");
        }
        let short_size = read_be_u32(input, offset, end)?;
        let type_start = offset.checked_add(4).ok_or("MP4 box offset overflow")?;
        let type_end = type_start.checked_add(4).ok_or("MP4 box offset overflow")?;
        let box_type: [u8; 4] = input
            .get(type_start..type_end)
            .filter(|_| type_end <= end)
            .ok_or("MP4 box header is truncated")?
            .try_into()
            .map_err(|_| "MP4 box header is truncated")?;
        if short_size == 1 {
            return Err("MP4 extended-size boxes are not supported for preview");
        }
        let box_size = if short_size == 0 {
            end - offset
        } else {
            usize::try_from(short_size).map_err(|_| "MP4 box size is invalid")?
        };
        let remaining = end - offset;
        if box_size < 8 || box_size > remaining {
            return Err("MP4 box size exceeds its container");
        }
        let box_end = offset
            .checked_add(box_size)
            .ok_or("MP4 box offset overflow")?;
        let payload_start = offset.checked_add(8).ok_or("MP4 box offset overflow")?;
        if let Some(prefix) =
            child_box_prefix(input, box_type, parent_type, payload_start, box_end)?
        {
            let children_start = payload_start
                .checked_add(prefix)
                .ok_or("MP4 box offset overflow")?;
            if children_start > box_end {
                return Err("MP4 container header exceeds its box size");
            }
            validate_box_region(
                input,
                children_start,
                box_end,
                depth + 1,
                Some(box_type),
                box_count,
            )?;
        }
        offset = box_end;
        if short_size == 0 {
            break;
        }
    }
    if offset != end {
        return Err("MP4 box tree does not end at the declared boundary");
    }
    Ok(())
}

fn validate_mp4_box_tree(input: &[u8]) -> Result<(), &'static str> {
    let mut box_count = 0;
    validate_box_region(input, 0, input.len(), 0, None, &mut box_count)
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

fn validate_sample_time(track: &mp4::Mp4Track, sample_id: u32) -> Result<(), &'static str> {
    let mut first_sample = 1u32;
    let mut elapsed = 0u64;
    for entry in &track.trak.mdia.minf.stbl.stts.entries {
        let end_sample = first_sample
            .checked_add(entry.sample_count)
            .ok_or("MP4 time-to-sample table overflows")?;
        if sample_id < end_sample {
            let sample_offset = u64::from(sample_id - first_sample)
                .checked_mul(u64::from(entry.sample_delta))
                .ok_or("MP4 sample time overflows")?;
            elapsed
                .checked_add(sample_offset)
                .ok_or("MP4 sample time overflows")?;
            return Ok(());
        }
        first_sample = end_sample;
        let run_duration = u64::from(entry.sample_count)
            .checked_mul(u64::from(entry.sample_delta))
            .ok_or("MP4 time-to-sample duration overflows")?;
        elapsed = elapsed
            .checked_add(run_duration)
            .ok_or("MP4 time-to-sample duration overflows")?;
    }
    Err("MP4 time-to-sample table does not cover the keyframe")
}

fn first_sync_sample(track: &mp4::Mp4Track) -> Result<(u32, u32), &'static str> {
    let sample_to_chunk = &track.trak.mdia.minf.stbl.stsc.entries;
    if sample_to_chunk.is_empty()
        || sample_to_chunk
            .iter()
            .any(|entry| entry.first_chunk == 0 || entry.samples_per_chunk == 0)
    {
        return Err("MP4 sample-to-chunk table is invalid");
    }
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
    validate_sample_time(track, sample_id)?;
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
    validate_mp4_box_tree(input)?;

    let mut reader = Mp4Reader::read_header(BoundedCursor::new(input), input.len() as u64)
        .map_err(|_| "MP4 container parsing failed")?;
    if reader.timescale() == 0 {
        return Err("MP4 movie time scale is invalid");
    }
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
        if track.timescale() == 0 {
            return Err("MP4 video track time scale is invalid");
        }
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

    #[test]
    fn rejects_zero_samples_per_chunk_without_panicking() {
        let malformed = include_bytes!("../tests/fixtures/zero-samples-per-chunk.mp4");
        assert!(render_first_frame(malformed).is_err());
    }

    #[test]
    fn rejects_incomplete_time_to_sample_table_without_panicking() {
        let malformed = include_bytes!("../tests/fixtures/missing-stts-entry.mp4");
        assert!(render_first_frame(malformed).is_err());
    }

    #[test]
    fn rejects_extended_box_sizes_before_untrusted_allocations() {
        let malformed = include_bytes!("../tests/fixtures/extended-box-size.mp4");
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
