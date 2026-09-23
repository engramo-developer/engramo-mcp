//! PCM16 mono → MP3 encoding, behind a small [`AudioEncoder`] seam.
//!
//! Ported from `engram-api/premium-tts/src/audio.rs`: same encoder settings
//! (mono, CBR 128 kbps, `Quality::Best`, `FlushNoGap`), same buffer-sizing and
//! `unsafe` `set_len` handling. The only change is the error type — this module
//! has no `crate::error` to borrow from `engram-api`, so it defines its own.

use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, MonoPcm, Quality, max_required_buffer_size};

/// Gemini TTS returns 24kHz mono 16-bit PCM.
pub const GEMINI_PCM_SAMPLE_RATE: u32 = 24_000;

/// Extra headroom (bytes) reserved on top of `max_required_buffer_size` to give
/// `Encoder::flush` room to write its trailing frame into the same buffer.
const FLUSH_HEADROOM_BYTES: usize = 7200;

/// Errors from encoding PCM audio to a compressed format.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio encoding failed: {0}")]
    Encode(String),
}

/// An audio encoder that turns 16-bit little-endian mono PCM into a compressed
/// format, so the TTS layer can be tested (and, if the vendored MP3 encoder ever
/// becomes a distribution problem, swapped) without touching call sites.
pub trait AudioEncoder: Send + Sync {
    /// Encodes little-endian 16-bit mono PCM at `sample_rate` Hz.
    fn encode_pcm16_mono(&self, pcm: &[u8], sample_rate: u32) -> Result<Vec<u8>, AudioError>;
    /// MIME type of the encoded output (e.g. for `upload_media`).
    fn content_type(&self) -> &'static str;
    /// File extension of the encoded output, without a leading dot.
    fn file_extension(&self) -> &'static str;
}

/// [`AudioEncoder`] backed by `mp3lame-encoder` (vendored LAME, statically
/// linked — see `THIRD_PARTY_LICENSES`).
pub struct Mp3Encoder;

impl AudioEncoder for Mp3Encoder {
    fn encode_pcm16_mono(&self, pcm: &[u8], sample_rate: u32) -> Result<Vec<u8>, AudioError> {
        pcm16_mono_to_mp3(pcm, sample_rate)
    }

    fn content_type(&self) -> &'static str {
        "audio/mpeg"
    }

    fn file_extension(&self) -> &'static str {
        "mp3"
    }
}

/// Transcodes little-endian 16-bit mono PCM to MP3.
///
/// A dangling trailing byte (odd-length input) is silently dropped
/// (`chunks_exact(2)`). Empty input is not an error: LAME may still emit a
/// trailing (e.g. VBR/Xing) frame on flush, so this returns `Ok` with whatever
/// (possibly empty) bytes the encoder produces rather than a synthetic error.
pub fn pcm16_mono_to_mp3(pcm_bytes: &[u8], sample_rate: u32) -> Result<Vec<u8>, AudioError> {
    let samples: Vec<i16> = pcm_bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();

    let mut builder =
        Builder::new().ok_or_else(|| AudioError::Encode("lame init failed".into()))?;
    builder
        .set_num_channels(1)
        .map_err(|e| AudioError::Encode(format!("set_num_channels: {e:?}")))?;
    builder
        .set_sample_rate(sample_rate)
        .map_err(|e| AudioError::Encode(format!("set_sample_rate: {e:?}")))?;
    builder
        .set_brate(Bitrate::Kbps128)
        .map_err(|e| AudioError::Encode(format!("set_brate: {e:?}")))?;
    builder
        .set_quality(Quality::Best)
        .map_err(|e| AudioError::Encode(format!("set_quality: {e:?}")))?;
    let mut encoder = builder
        .build()
        .map_err(|e| AudioError::Encode(format!("build: {e:?}")))?;

    let mut mp3_buf =
        Vec::with_capacity(max_required_buffer_size(samples.len()) + FLUSH_HEADROOM_BYTES);

    let encoded_size = encoder
        .encode(MonoPcm(&samples), mp3_buf.spare_capacity_mut())
        .map_err(|e| AudioError::Encode(format!("encode: {e:?}")))?;
    // Real runtime guard (not `debug_assert!`, which compiles to nothing in the release
    // build this crate ships via npm) against a future regression in the buffer-sizing
    // assumption below (e.g. if `Bitrate`/`Quality` become configurable and increase the
    // worst-case output size) — fails closed instead of letting `set_len` silently extend
    // the `Vec`'s logical length past its allocation (UB: reading uninitialized memory).
    if mp3_buf.len() + encoded_size > mp3_buf.capacity() {
        return Err(AudioError::Encode(
            "encoder wrote past the reserved buffer".to_string(),
        ));
    }
    // SAFETY: `encode` reports exactly how many bytes of `spare_capacity_mut` it
    // initialized; checked above that this fits within the reserved capacity.
    unsafe {
        mp3_buf.set_len(mp3_buf.len() + encoded_size);
    }

    let flush_size = encoder
        .flush::<FlushNoGap>(mp3_buf.spare_capacity_mut())
        .map_err(|e| AudioError::Encode(format!("flush: {e:?}")))?;
    if mp3_buf.len() + flush_size > mp3_buf.capacity() {
        return Err(AudioError::Encode(
            "encoder wrote past the reserved buffer on flush".to_string(),
        ));
    }
    // SAFETY: same guarantee as above — `flush` reports the exact byte count it
    // initialized, checked above that this fits within the reserved capacity.
    unsafe {
        mp3_buf.set_len(mp3_buf.len() + flush_size);
    }

    Ok(mp3_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// 1s of a 440Hz sine wave at 24kHz mono 16-bit — a real signal (not
    /// silence), so LAME's psychoacoustic model has something to chew on.
    fn sine_pcm_1s() -> Vec<u8> {
        let sample_rate = GEMINI_PCM_SAMPLE_RATE as usize;
        let freq = 440.0f32;
        let mut bytes = Vec::with_capacity(sample_rate * 2);
        for n in 0..sample_rate {
            let t = n as f32 / sample_rate as f32;
            let sample = (t * freq * 2.0 * PI).sin() * i16::MAX as f32 * 0.5;
            bytes.extend_from_slice(&(sample as i16).to_le_bytes());
        }
        bytes
    }

    /// Every MP3 frame header starts with an 11-bit sync word: 0xFF followed by
    /// the top 3 bits of the next byte set (0xE0 mask). LAME may prefix the
    /// stream with an ID3/Xing header, so scan the first few KB rather than
    /// assuming byte 0 is the sync.
    fn find_frame_sync(mp3: &[u8]) -> Option<usize> {
        let scan_len = mp3.len().min(4096);
        mp3[..scan_len]
            .windows(2)
            .position(|w| w[0] == 0xFF && w[1] & 0xE0 == 0xE0)
    }

    #[test]
    fn test_pcm16_mono_to_mp3_sine_wave_has_frame_sync() {
        let mp3 = pcm16_mono_to_mp3(&sine_pcm_1s(), GEMINI_PCM_SAMPLE_RATE).unwrap();
        assert!(!mp3.is_empty(), "MP3 output must not be empty");
        assert!(
            find_frame_sync(&mp3).is_some(),
            "expected an MP3 frame sync (0xFF 0xE?) within the first 4KB"
        );
    }

    #[test]
    fn test_pcm16_mono_to_mp3_output_smaller_than_input() {
        let pcm = sine_pcm_1s();
        let mp3 = pcm16_mono_to_mp3(&pcm, GEMINI_PCM_SAMPLE_RATE).unwrap();
        // 1s @ 24kHz/16-bit mono PCM is 48000 bytes; 128kbps CBR MP3 for 1s is
        // ~16000 bytes. Assert well under half as a loose sanity bound.
        assert!(
            mp3.len() < pcm.len() / 2,
            "expected MP3 ({} bytes) to be much smaller than PCM ({} bytes)",
            mp3.len(),
            pcm.len()
        );
    }

    #[test]
    fn test_pcm16_mono_to_mp3_odd_length_drops_trailing_byte() {
        // chunks_exact(2) silently drops a dangling trailing byte instead of panicking.
        let mut pcm = sine_pcm_1s();
        pcm.push(0xAB);
        let result = pcm16_mono_to_mp3(&pcm, GEMINI_PCM_SAMPLE_RATE);
        assert!(result.is_ok());
    }

    #[test]
    fn test_pcm16_mono_to_mp3_empty_input_ok() {
        let mp3 = pcm16_mono_to_mp3(&[], GEMINI_PCM_SAMPLE_RATE).unwrap();
        // LAME may still emit a trailing (e.g. VBR/Xing) frame on flush even for
        // empty input — the only hard guarantee is that this must not panic.
        let _ = mp3;
    }

    #[test]
    fn test_mp3_encoder_content_type_and_extension() {
        let encoder = Mp3Encoder;
        assert_eq!(encoder.content_type(), "audio/mpeg");
        assert_eq!(encoder.file_extension(), "mp3");
    }

    #[test]
    fn test_mp3_encoder_matches_free_function() {
        let pcm = sine_pcm_1s();
        let via_trait = Mp3Encoder
            .encode_pcm16_mono(&pcm, GEMINI_PCM_SAMPLE_RATE)
            .unwrap();
        assert!(!via_trait.is_empty());
        assert!(find_frame_sync(&via_trait).is_some());
    }
}
