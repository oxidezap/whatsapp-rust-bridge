//! Pure-Rust stateful MLOW audio decoder exposed to WebAssembly.
//!
//! Exposes the core's stateful [`MlowDecoder`] so a JavaScript host can decode
//! inbound WhatsApp MLOW audio packets to 16 kHz mono Float32 PCM without
//! native dependencies.

use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore::voip::MlowDecoder;

/// Stateful pure-Rust decoder for WhatsApp's proprietary MLOW audio streams.
///
/// Decodes inbound MLOW audio packets into mono 16 kHz Float32 PCM samples in [-1.0, 1.0].
/// One decoder instance represents one continuous audio stream. It should be reset
/// or recreated across stream discontinuities, such as a new call.
#[wasm_bindgen]
pub struct MlowAudioDecoder {
    inner: MlowDecoder,
}

impl Default for MlowAudioDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl MlowAudioDecoder {
    /// Construct a new stateful MLOW audio decoder.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: MlowDecoder::new(),
        }
    }

    /// Decode one inbound MLOW packet into mono 16 kHz Float32 PCM samples.
    pub fn decode(&mut self, packet: &[u8]) -> Box<[f32]> {
        self.inner.decode(packet).into_boxed_slice()
    }

    /// Reset internal filter, predictor, and synthesis state.
    ///
    /// Call this when the audio stream discontinuously changes, such as when
    /// transitioning between calls.
    pub fn reset(&mut self) {
        self.inner.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn empty_payload_conceals_to_60ms_silence() {
        let mut decoder = MlowAudioDecoder::new();
        let samples = decoder.decode(&[]);
        assert_eq!(samples.len(), 960);
        assert!(samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn twenty_ms_payload_decodes_to_320_samples() {
        let mut decoder = MlowAudioDecoder::new();
        let samples = decoder.decode(&[0x48, 0xaa, 0xbb, 0xcc]);
        assert_eq!(samples.len(), 320);
        for &s in samples.iter() {
            assert!((-1.0..=1.0).contains(&s));
        }
    }

    #[test]
    fn one_hundred_twenty_ms_payload_decodes_to_1920_samples() {
        let mut decoder = MlowAudioDecoder::new();
        let samples = decoder.decode(&[0x58, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x11, 0x22]);
        assert_eq!(samples.len(), 1920);
        for &s in samples.iter() {
            assert!((-1.0..=1.0).contains(&s));
        }
    }

    #[test]
    fn reset_clears_state() {
        let mut dec1 = MlowAudioDecoder::new();
        let mut dec2 = MlowAudioDecoder::new();
        let packet = [0x48, 0xaa, 0xbb, 0xcc];
        let _ = dec1.decode(&packet);
        dec1.reset();

        let out1 = dec1.decode(&packet);
        let out2 = dec2.decode(&packet);
        assert_eq!(out1.as_ref(), out2.as_ref());
    }
}
