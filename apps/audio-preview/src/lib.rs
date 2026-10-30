#![forbid(unsafe_code)]

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DECODED_SAMPLES: usize = 18_000_000;
pub const MAX_PLAYBACK_DURATION: Duration = Duration::from_secs(180);
const MAX_CHANNELS: usize = 2;
const MAX_SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Cancelled,
    Unsupported,
    Invalid,
    LimitExceeded,
}

pub struct DecodedAudio {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

fn is_wav(data: &[u8]) -> bool {
    data.get(0..4) == Some(b"RIFF") && data.get(8..12) == Some(b"WAVE")
}

fn has_supported_container_header(data: &[u8]) -> bool {
    is_wav(data)
        || data.starts_with(b"fLaC")
        || data.starts_with(b"OggS")
        || data.starts_with(b"ID3")
        || data
            .get(0..2)
            .is_some_and(|sync| sync[0] == 0xff && sync[1] & 0xe0 == 0xe0)
        || data.get(4..8) == Some(b"ftyp")
}

fn validate_wav_header(data: &[u8]) -> Result<(), DecodeError> {
    if !is_wav(data) {
        return Ok(());
    }
    let mut offset = 12usize;
    loop {
        let header_end = offset.checked_add(8).ok_or(DecodeError::Invalid)?;
        if header_end > data.len() {
            return Err(DecodeError::Invalid);
        }
        let chunk_id = &data[offset..offset + 4];
        let chunk_size = u32::from_le_bytes(
            data[offset + 4..header_end]
                .try_into()
                .map_err(|_| DecodeError::Invalid)?,
        ) as usize;
        let payload = header_end;
        let end = payload
            .checked_add(chunk_size)
            .ok_or(DecodeError::Invalid)?;
        if end > data.len() {
            return Err(DecodeError::Invalid);
        }
        if chunk_id == b"fmt " {
            if chunk_size < 16 {
                return Err(DecodeError::Invalid);
            }
            let channels = u16::from_le_bytes([data[payload + 2], data[payload + 3]]) as usize;
            let sample_rate = u32::from_le_bytes(
                data[payload + 4..payload + 8]
                    .try_into()
                    .map_err(|_| DecodeError::Invalid)?,
            );
            if channels == 0 || sample_rate == 0 {
                return Err(DecodeError::Invalid);
            }
            if channels > MAX_CHANNELS || sample_rate > MAX_SAMPLE_RATE {
                return Err(DecodeError::Unsupported);
            }
            return Ok(());
        }
        offset = end
            .checked_add(chunk_size & 1)
            .ok_or(DecodeError::Invalid)?;
    }
}

/// Decodes a short audio preview into an interleaved PCM buffer with a hard sample cap.
pub fn decode_audio(data: Arc<[u8]>, cancel: &AtomicBool) -> Result<DecodedAudio, DecodeError> {
    decode_audio_with_limit(data, cancel, MAX_DECODED_SAMPLES)
}

/// The fuzz target uses a smaller decoded-sample budget to bound one iteration's CPU cost.
pub fn decode_audio_with_limit(
    data: Arc<[u8]>,
    cancel: &AtomicBool,
    sample_limit: usize,
) -> Result<DecodedAudio, DecodeError> {
    if data.len() > MAX_INPUT_BYTES {
        return Err(DecodeError::LimitExceeded);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(DecodeError::Cancelled);
    }
    if !has_supported_container_header(&data) {
        return Err(DecodeError::Unsupported);
    }
    validate_wav_header(&data)?;

    let source = Box::new(Cursor::new(data));
    let stream = MediaSourceStream::new(source, Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &Hint::new(),
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|_| DecodeError::Unsupported)?;
    let (track_id, codec_params) = {
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(DecodeError::Unsupported)?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .cloned()
            .ok_or(DecodeError::Unsupported)?;
        (track.id, params)
    };
    if codec_params
        .sample_rate
        .is_some_and(|rate| rate == 0 || rate > MAX_SAMPLE_RATE)
        || codec_params
            .channels
            .as_ref()
            .is_some_and(|channels| channels.count() == 0 || channels.count() > MAX_CHANNELS)
    {
        return Err(DecodeError::Unsupported);
    }
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())
        .map_err(|_| DecodeError::Unsupported)?;

    let sample_limit = sample_limit.min(MAX_DECODED_SAMPLES);
    let mut sample_rate = None;
    let mut channels = None;
    let mut samples = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DecodeError::Cancelled);
        }
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::ResetRequired) => return Err(DecodeError::Unsupported),
            Err(_) => return Err(DecodeError::Invalid),
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => continue,
            Err(_) => return Err(DecodeError::Invalid),
        };
        let spec = decoded.spec();
        let packet_rate = spec.rate();
        let packet_channels = spec.channels().count();
        if packet_rate == 0
            || packet_rate > MAX_SAMPLE_RATE
            || packet_channels == 0
            || packet_channels > MAX_CHANNELS
        {
            return Err(DecodeError::Unsupported);
        }
        if sample_rate.is_some_and(|rate| rate != packet_rate)
            || channels.is_some_and(|count| count != packet_channels)
        {
            return Err(DecodeError::Unsupported);
        }
        sample_rate = Some(packet_rate);
        channels = Some(packet_channels);

        let packet_samples = decoded.samples_interleaved();
        let total_samples = samples
            .len()
            .checked_add(packet_samples)
            .ok_or(DecodeError::LimitExceeded)?;
        if total_samples > sample_limit {
            return Err(DecodeError::LimitExceeded);
        }
        let frames = total_samples / packet_channels;
        let max_frames = packet_rate as u64 * MAX_PLAYBACK_DURATION.as_secs();
        if frames as u64 > max_frames {
            return Err(DecodeError::LimitExceeded);
        }
        let start = samples.len();
        samples.resize(total_samples, 0.0);
        decoded.copy_to_slice_interleaved(&mut samples[start..]);
    }

    if samples.is_empty() {
        return Err(DecodeError::Invalid);
    }
    Ok(DecodedAudio {
        channels: channels
            .and_then(|count| u16::try_from(count).ok())
            .ok_or(DecodeError::Invalid)?,
        sample_rate: sample_rate.ok_or(DecodeError::Invalid)?,
        samples,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn wave() -> Vec<u8> {
        let samples = [128; 32];
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36u32 + samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(&samples);
        wav
    }

    #[test]
    fn rejects_zero_sample_rate_before_demux() {
        let mut wav = wave();
        wav[24..28].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            decode_audio(Arc::from(wav), &AtomicBool::new(false)),
            Err(DecodeError::Invalid)
        ));
    }

    #[test]
    fn rejects_the_original_offset_riff_asan_regression() {
        let wav =
            include_bytes!("../../../fuzz/seeds/audio_decoder/offset-riff-zero-sample-rate.wav");
        assert!(matches!(
            decode_audio(Arc::from(wav.as_slice()), &AtomicBool::new(false)),
            Err(DecodeError::Unsupported)
        ));
    }

    #[test]
    fn rejects_the_original_asan_zero_sample_rate_regression() {
        let wav = include_bytes!("../../../fuzz/seeds/audio_decoder/zero-sample-rate.wav");
        assert!(matches!(
            decode_audio(Arc::from(wav.as_slice()), &AtomicBool::new(false)),
            Err(DecodeError::Invalid)
        ));
    }

    #[test]
    fn decodes_valid_wave_to_a_bounded_pcm_buffer() {
        let decoded = decode_audio(Arc::from(wave()), &AtomicBool::new(false)).unwrap();
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.sample_rate, 8_000);
        assert!(!decoded.samples.is_empty());
        assert!(decoded.samples.len() <= MAX_DECODED_SAMPLES);
    }
}
