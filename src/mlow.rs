//! Pure-Rust stateful MLOW audio decoder exposed to WebAssembly.
//!
//! Exposes the core's stateful [`MlowDecoder`] so a JavaScript host can decode
//! inbound WhatsApp MLOW audio packets to 16 kHz mono Float32 PCM without
//! native dependencies.

use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore::voip::MlowDecoder;
pub use whatsapp_rust::wacore::voip::rtp::RTP_PAYLOAD_TYPE_MLOW_RED;

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
    ///
    /// Accepts the optional RTP `payload_type` (e.g. 120 for bare MLOW, 121 for MLOW RED).
    /// When payload type 121 is passed, the decoder activates RED depacketization for that packet.
    /// Redundancy state does not stay sticky: omitting `payload_type` or passing 120 decodes bare frames.
    #[wasm_bindgen(js_name = decode)]
    pub fn decode(
        &mut self,
        packet: &[u8],
        #[wasm_bindgen(unchecked_optional_param_type = "number | null")] payload_type: JsValue,
    ) -> Result<Box<[f32]>, JsValue> {
        let pt: Option<u8> = if payload_type.is_undefined() || payload_type.is_null() {
            None
        } else if let Some(n) = payload_type.as_f64() {
            if n.is_finite() && n.fract() == 0.0 && (0.0..=255.0).contains(&n) {
                Some(n as u8)
            } else {
                return Err(crate::errors::to_js_error(&crate::errors::invalid_arg(
                    "payloadType",
                    format!("expected RTP payload type integer between 0 and 255, got {n}"),
                )));
            }
        } else {
            return Err(crate::errors::to_js_error(&crate::errors::invalid_arg(
                "payloadType",
                "expected RTP payload type number",
            )));
        };
        self.inner
            .set_redundancy(i32::from(pt == Some(RTP_PAYLOAD_TYPE_MLOW_RED)));
        Ok(self.inner.decode(packet).into_boxed_slice())
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
    use whatsapp_rust::wacore::voip::MlowEncoder;

    fn test_tone(freq_hz: f32, count: usize, phase: f32) -> Vec<f32> {
        (0..count)
            .map(|i| {
                let t = i as f32 / 16000.0;
                (2.0 * core::f32::consts::PI * freq_hz * t + phase).sin() * 0.5
            })
            .collect()
    }

    fn pt(value: u8) -> JsValue {
        JsValue::from_f64(f64::from(value))
    }

    #[test]
    fn empty_payload_conceals_to_60ms_silence() {
        let mut decoder = MlowAudioDecoder::new();
        let samples = decoder
            .decode(&[], JsValue::UNDEFINED)
            .expect("decode silence");
        assert_eq!(samples.len(), 960);
        assert!(samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn twenty_ms_payload_decodes_to_320_samples() {
        let mut decoder = MlowAudioDecoder::new();
        let samples = decoder
            .decode(&[0x48, 0xaa, 0xbb, 0xcc], JsValue::UNDEFINED)
            .expect("decode payload");
        assert_eq!(samples.len(), 320);
        for &s in samples.iter() {
            assert!((-1.0..=1.0).contains(&s));
        }
    }

    #[test]
    fn real_voice_roundtrip_decodes_non_silent_audio() {
        let mut encoder = MlowEncoder::new();
        let pcm = test_tone(440.0, 960, 0.0);
        let packet = encoder.encode(&pcm).expect("encode frame");

        let mut decoder = MlowAudioDecoder::new();
        let decoded = decoder.decode(&packet, pt(120)).expect("decode voice");
        assert_eq!(decoded.len(), 960);
        assert!(decoded.iter().all(|s| s.is_finite()));
        // Proves real voice reconstruction rather than zero-silence concealment
        assert!(
            decoded.iter().any(|&s| s.abs() > 0.05),
            "decoded voice should reconstruct non-silent audio"
        );
    }

    #[test]
    fn stream_maintains_state_across_frames() {
        let mut encoder = MlowEncoder::new();
        let pcm_a = test_tone(440.0, 960, 0.0);
        let pcm_b = test_tone(880.0, 960, 0.5);
        let packet_a = encoder.encode(&pcm_a).expect("encode A");
        let packet_b = encoder.encode(&pcm_b).expect("encode B");

        // Stateful decode: frame A then frame B
        let mut dec_stream = MlowAudioDecoder::new();
        let _ = dec_stream.decode(&packet_a, pt(120)).expect("decode A");
        let stateful_b = dec_stream.decode(&packet_b, pt(120)).expect("decode B");

        // Cold decode: frame B directly without preceding frame A
        let mut dec_cold = MlowAudioDecoder::new();
        let cold_b = dec_cold.decode(&packet_b, pt(120)).expect("cold decode");

        assert_eq!(stateful_b.len(), 960);
        assert_eq!(cold_b.len(), 960);
        // Inter-frame predictor and filter history differentiate stateful from cold
        assert_ne!(
            stateful_b.as_ref(),
            cold_b.as_ref(),
            "stateful decode should differ from cold start"
        );

        // Reset restores state back to clean initial condition
        dec_stream.reset();
        let after_reset_b = dec_stream
            .decode(&packet_b, pt(120))
            .expect("decode after reset");
        assert_eq!(
            after_reset_b.as_ref(),
            cold_b.as_ref(),
            "after reset, decode should match cold start"
        );
    }

    #[test]
    fn red_payload_type_121_unwraps_split_red_and_clears_sticky_redundancy() {
        let mut encoder = MlowEncoder::new();
        let pcm_a = test_tone(500.0, 960, 0.0);
        let pcm_b = test_tone(1000.0, 960, 0.0);
        let bare_a = encoder.encode(&pcm_a).expect("encode A");
        let bare_b = encoder.encode(&pcm_b).expect("encode B");

        // Wrap bare_a in SplitRed (N=1 redundancy):
        // [0x80 | time_code, size, 0x00 (main marker), redundant_payload, main_payload]
        let red_payload = [0xAAu8, 0xBB];
        let mut env = vec![0x80u8, red_payload.len() as u8, 0x00];
        env.extend_from_slice(&red_payload);
        env.extend_from_slice(&bare_a);

        let mut dec_bare = MlowAudioDecoder::new();
        let bare_pcm = dec_bare.decode(&bare_a, pt(120)).expect("decode bare");

        let mut dec_red = MlowAudioDecoder::new();
        // PT 121 activates RED depacketizer
        let red_pcm = dec_red.decode(&env, pt(121)).expect("decode RED");
        assert_eq!(
            red_pcm.as_ref(),
            bare_pcm.as_ref(),
            "RED unwrapped frame should match bare decode"
        );

        // Subsequent bare frame with PT 120 or None must NOT keep sticky redundancy
        dec_red.reset();
        let bare_next = dec_red.decode(&bare_b, pt(120)).expect("decode next");
        let mut dec_clean = MlowAudioDecoder::new();
        let expected_next = dec_clean.decode(&bare_b, pt(120)).expect("decode expected");
        assert_eq!(
            bare_next.as_ref(),
            expected_next.as_ref(),
            "redundancy state must not remain sticky on PT 120"
        );
    }
}
