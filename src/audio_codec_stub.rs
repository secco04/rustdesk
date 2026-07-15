// M1 (plans/soft-frolicking-thimble.md): drop-in replacement for the `magnum_opus` crate, which
// pins bindgen "0.59" — incompatible with scrap's 0.65 / kcp-sys's 0.71.1 needing NDK cross-compile
// clang args in the same build. Audio was already out of scope for M1-M4 in the plan, so this stub
// keeps client.rs/audio_service.rs compiling unchanged (same API shape) without decoding/encoding
// anything. Restore the real `magnum-opus` dependency + revert both call sites once audio is
// actually being implemented.

use std::fmt;

#[derive(Debug)]
pub struct OpusError;

impl fmt::Display for OpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "audio codec deferred (see src/audio_codec_stub.rs)")
    }
}

impl std::error::Error for OpusError {}

pub enum Channels {
    Mono,
    Stereo,
}

pub enum Application {
    Voip,
    Audio,
    LowDelay,
}

pub struct Decoder;

impl Decoder {
    pub fn new(_sample_rate: u32, _channels: Channels) -> Result<Self, OpusError> {
        Err(OpusError)
    }

    pub fn decode_float(
        &mut self,
        _input: &[u8],
        _output: &mut [f32],
        _fec: bool,
    ) -> Result<usize, OpusError> {
        Ok(0)
    }
}

pub struct Encoder;

impl Encoder {
    pub fn new(_sample_rate: u32, _channels: Channels, _application: Application) -> Result<Self, OpusError> {
        Err(OpusError)
    }

    pub fn encode_vec_float(&mut self, _input: &[f32], _max_size: usize) -> Result<Vec<u8>, OpusError> {
        Ok(Vec::new())
    }
}
