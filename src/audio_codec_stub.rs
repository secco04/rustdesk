// M1 (plans/soft-frolicking-thimble.md): originally a drop-in no-op replacement for the
// `magnum_opus` crate, which pins bindgen "0.59" — incompatible with scrap's 0.65 / kcp-sys's
// 0.71.1 needing NDK cross-compile clang args in the same build. Audio (plans/
// soft-frolicking-thimble.md, "Audio" round): the Decoder half is now a real wrapper around
// `opus-rs`, a pure-Rust Opus decoder (RFC 6716, ported from libopus 1.6) that needs no C library
// or bindgen at all — it sidesteps the exact NDK-cross-compile problem magnum-opus hit, so there
// was no need to fight that version pin after all. client.rs/audio_service.rs need NO changes:
// this file keeps the exact same `Decoder`/`Channels` API shape they already call.
//
// The Encoder half stays a stub — this app only ever RECEIVES audio from a peer (playing the
// host's sound), it never captures/sends its own, so nothing in this codebase calls Encoder::new.

use std::fmt;

#[derive(Debug)]
pub struct OpusError(String);

impl fmt::Display for OpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "opus decode error: {}", self.0)
    }
}

impl std::error::Error for OpusError {}

impl From<&str> for OpusError {
    fn from(s: &str) -> Self {
        OpusError(s.to_string())
    }
}

#[derive(Clone, Copy)]
pub enum Channels {
    Mono,
    Stereo,
}

impl Channels {
    fn count(self) -> usize {
        match self {
            Channels::Mono => 1,
            Channels::Stereo => 2,
        }
    }
}

pub enum Application {
    Voip,
    Audio,
    LowDelay,
}

pub struct Decoder {
    inner: opus_rs::OpusDecoder,
    channels: usize,
    max_frame_samples: usize,
}

impl Decoder {
    pub fn new(sample_rate: u32, channels: Channels) -> Result<Self, OpusError> {
        let channels = channels.count();
        let inner = opus_rs::OpusDecoder::new(sample_rate as i32, channels)
            .map_err(|e| OpusError(e.to_string()))?;
        // The Opus spec caps any single frame at 120ms — libopus itself documents 120ms as the
        // right size to request when the caller doesn't know the packet's real duration up
        // front (our case exactly: `output` is a whole-second scratch buffer, not sized to one
        // frame). opus-rs's own `decode()` was found (via a real panic, not guessed) to
        // internally bound its work to a max-single-frame-sized buffer derived from whatever
        // `frame_size` it's given — requesting far more (our old `output.len() / channels`,
        // effectively a full second) overflowed that internal buffer:
        // "range end index 96000 out of range for slice of length 11520" — 11520 = 5760 * 2
        // channels, i.e. exactly 120ms at 48kHz, confirming this cap is what opus-rs expects.
        let max_frame_samples = (sample_rate as usize * 120) / 1000;
        Ok(Self { inner, channels, max_frame_samples })
    }

    /// Matches magnum_opus's real signature exactly: `output`'s length bounds how many
    /// interleaved samples (across all channels) CAN be written, but the `frame_size` opus-rs
    /// actually wants is the Opus spec's own max single-frame duration (120ms) — see `new`'s doc
    /// for why passing the full output capacity panics instead of erroring cleanly.
    pub fn decode_float(
        &mut self,
        input: &[u8],
        output: &mut [f32],
        _fec: bool,
    ) -> Result<usize, OpusError> {
        let frame_size = (output.len() / self.channels).min(self.max_frame_samples);
        self.inner
            .decode(input, frame_size, output)
            .map_err(|e| OpusError(e.to_string()))
    }
}

pub struct Encoder;

impl Encoder {
    pub fn new(_sample_rate: u32, _channels: Channels, _application: Application) -> Result<Self, OpusError> {
        Err(OpusError("encoding not implemented — this app never sends audio".to_string()))
    }

    pub fn encode_vec_float(&mut self, _input: &[f32], _max_size: usize) -> Result<Vec<u8>, OpusError> {
        Ok(Vec::new())
    }
}
