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
        // First fix (120ms, the Opus spec's documented max single-frame duration) stopped the
        // panic ("range end index 96000 out of range for slice of length 11520" — 11520 = 5760*2
        // channels = 120ms at 48kHz, confirming opus-rs bounds its work to whatever frame_size it
        // gets) but didn't fix playback: decode then reported n=5760 samples/channel for EVERY
        // packet regardless of actual size (confirmed via the "first successful decode" log), even
        // though `server/audio_service.rs`'s own encoder (this same repo, the RustDesk protocol's
        // authoritative sender) fixes `frame_size = sample_rate / 100` — exactly 10ms, ALWAYS,
        // never 120ms. opus-rs apparently echoes back whatever frame_size it was asked for rather
        // than the packet's true decoded length when given a generously large one — every call was
        // silently padding ~11x more samples than were real, mostly stale/uninitialized leftover
        // buffer content, explaining "no audible sound despite a technically-working pipeline"
        // exactly. Matching the encoder's own fixed 10ms exactly removes the ambiguity: real and
        // requested frame size are now identical, so whatever convention opus-rs follows, the
        // result is correct either way.
        let max_frame_samples = sample_rate as usize / 100;
        Ok(Self { inner, channels, max_frame_samples })
    }

    /// Matches magnum_opus's real signature exactly: `output`'s length bounds how many
    /// interleaved samples (across all channels) CAN be written, but the `frame_size` opus-rs
    /// actually wants is the REAL per-packet duration RustDesk's own encoder always uses (10ms —
    /// see `new`'s doc for why a generously large guess broke playback instead of just being safe).
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
