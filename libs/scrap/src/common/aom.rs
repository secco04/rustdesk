// M1 (plans/soft-frolicking-thimble.md): drop-in replacement for the real aom.rs (AV1 via
// libaom), which needs the `aom` vcpkg port — its CMake configure fails to find an ASM compiler
// when cross-compiling to Android from a Windows host (a vcpkg/NDK toolchain-file wiring issue,
// unrelated to our own code). VP8/VP9 (via libvpx, already working) remain fully functional as
// RustDesk's primary codecs; AV1 was always an optional enhancement, out of scope for M1-M4 in the
// plan. This stub keeps codec.rs's CodecFormat::AV1 / PreferCodec::AV1 branches compiling
// unchanged — AomEncoder::new and AomDecoder::new both simply return an error, so AV1 is never
// selected or used at runtime. Restore the real aom vcpkg dependency + this file's original body
// (see plans/soft-frolicking-thimble.md) once AV1 support is actually needed.

use crate::codec::{EncoderApi, EncoderCfg};
use crate::{common::GoogleImage, EncodeInput, EncodeYuvFormat, Pixfmt};
use hbb_common::{
    anyhow::anyhow,
    message_proto::{Chroma, VideoFrame},
    ResultType,
};

#[derive(Clone, Copy, Debug)]
pub struct AomEncoderConfig {
    pub width: u32,
    pub height: u32,
    pub quality: f32,
    pub keyframe_interval: Option<usize>,
}

pub struct AomEncoder;

impl EncoderApi for AomEncoder {
    fn new(_cfg: EncoderCfg, _i444: bool) -> ResultType<Self>
    where
        Self: Sized,
    {
        Err(anyhow!("AV1 encoding deferred (see libs/scrap/src/common/aom.rs)"))
    }

    fn encode_to_message(&mut self, _frame: EncodeInput, _ms: i64) -> ResultType<VideoFrame> {
        Err(anyhow!("AV1 encoding deferred (see libs/scrap/src/common/aom.rs)"))
    }

    fn yuvfmt(&self) -> EncodeYuvFormat {
        EncodeYuvFormat {
            pixfmt: Pixfmt::I420,
            w: 0,
            h: 0,
            stride: vec![],
            u: 0,
            v: 0,
        }
    }

    fn set_quality(&mut self, _ratio: f32) -> ResultType<()> {
        Ok(())
    }

    fn bitrate(&self) -> u32 {
        0
    }

    fn support_changing_quality(&self) -> bool {
        false
    }

    fn latency_free(&self) -> bool {
        false
    }

    fn is_hardware(&self) -> bool {
        false
    }

    fn disable(&self) {}
}

impl AomEncoder {
    // Inherent method used by codec.rs's test_av1() self-test, separate from the EncoderApi trait.
    pub fn encode(&mut self, _pts: i64, _data: &[u8], _stride_align: usize) -> ResultType<()> {
        Err(anyhow!("AV1 encoding deferred (see libs/scrap/src/common/aom.rs)"))
    }
}

pub struct AomDecoder;

pub struct Image;

impl Image {
    pub fn new() -> Self {
        Image
    }

    pub fn is_null(&self) -> bool {
        true
    }

    pub fn to(&self, _rgb: &mut crate::common::ImageRgb) {}
}

impl GoogleImage for Image {
    fn width(&self) -> usize {
        0
    }

    fn height(&self) -> usize {
        0
    }

    fn stride(&self) -> Vec<i32> {
        vec![]
    }

    fn planes(&self) -> Vec<*mut u8> {
        vec![]
    }

    fn chroma(&self) -> Chroma {
        Chroma::I420
    }
}

impl AomDecoder {
    pub fn new() -> ResultType<Self> {
        Err(anyhow!("AV1 decoding deferred (see libs/scrap/src/common/aom.rs)"))
    }

    pub fn decode(&mut self, _data: &[u8]) -> ResultType<Vec<Image>> {
        Ok(vec![])
    }

    pub fn flush(&mut self) -> ResultType<Vec<Image>> {
        Ok(vec![])
    }
}
