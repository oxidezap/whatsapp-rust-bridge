//! Production Noise certificate-chain profile.
//!
//! The published artifact must verify the server Noise cert chain with XEdDSA.
//! The core skips that check under `cfg(test)` and under its
//! `danger-skip-cert-chain-verify` feature, so a test living inside the core
//! cannot prove the profile this crate ships. These tests link the core as a
//! regular dependency (no `cfg(test)` inside it) through the same feature set
//! the npm artifact builds with, so a passing run means the shipped graph
//! rejects a tampered chain.

#[cfg(test)]
mod tests {
    // Alias `#[test]` -> wasm_bindgen_test so these run on wasm32 via
    // `wasm-pack test --node` (the crate has no native test target).
    use wasm_bindgen_test::wasm_bindgen_test as test;
    use whatsapp_rust::buffa::Message;
    use whatsapp_rust::wacore::noise::HandshakeUtils;
    use whatsapp_rust::waproto::whatsapp::{self as wa, cert_chain::noise_certificate};

    // Structurally valid `CertChain` blob with zero-filled signatures and
    // fictitious keys. Mirrors the core's `cert_chain_verify.rs` fixture so no
    // `test-util` feature is needed to build it.
    fn build_zero_signed_chain(server_static_pub: &[u8; 32]) -> Vec<u8> {
        let intermediate_details = noise_certificate::Details {
            serial: Some(1),
            issuer_serial: Some(0),
            key: Some(vec![0xCC; 32]),
            not_before: Some(1_700_000_000),
            not_after: Some(1_900_000_000),
        };
        let intermediate_details_bytes = intermediate_details.encode_to_vec();

        let leaf_details = noise_certificate::Details {
            serial: Some(2),
            issuer_serial: Some(1),
            key: Some(server_static_pub.to_vec()),
            not_before: Some(1_700_000_500),
            not_after: Some(1_899_999_500),
        };
        let leaf_details_bytes = leaf_details.encode_to_vec();

        let chain = wa::CertChain {
            leaf: whatsapp_rust::buffa::MessageField::some(wa::cert_chain::NoiseCertificate {
                details: Some(leaf_details_bytes),
                signature: Some(vec![0u8; 64]),
            }),
            intermediate: whatsapp_rust::buffa::MessageField::some(
                wa::cert_chain::NoiseCertificate {
                    details: Some(intermediate_details_bytes),
                    signature: Some(vec![0u8; 64]),
                },
            ),
        };
        chain.encode_to_vec()
    }

    // The chain is structurally valid (right shape, leaf key matches the
    // server static) but carries an all-zero intermediate signature. The
    // production verify path must reject it at the XEdDSA step; acceptance
    // here means the bypass feature leaked into the shipped graph.
    #[test]
    fn production_profile_rejects_zero_signed_cert_chain() {
        let server_static_pub = [0xAAu8; 32];
        let chain_bytes = build_zero_signed_chain(&server_static_pub);

        let err = HandshakeUtils::verify_server_cert(&chain_bytes, &server_static_pub)
            .expect_err("zero-signed intermediate must fail XEdDSA verify");
        let msg = err.to_string();
        assert!(
            msg.contains("intermediate signature failed XEdDSA verify"),
            "expected an intermediate XEdDSA-verify failure, got: {msg}"
        );
    }

    // Same fixture family, chain built for another static key. This is a
    // rejection control, not a success control: no valid-chain fixture exists
    // anywhere, because signing one needs the issuer private key. The
    // structural leaf check fires before any signature step regardless of
    // profile, so this proves the harness reaches verification instead of
    // failing on framing or decoding.
    #[test]
    fn control_rejects_chain_for_another_static_key() {
        let real_static = [0xAAu8; 32];
        let chain_for_other_static = build_zero_signed_chain(&[0xBBu8; 32]);
        let err = HandshakeUtils::verify_server_cert(&chain_for_other_static, &real_static)
            .expect_err("leaf key != decrypted static must be a CertVerification error");
        assert!(
            err.to_string()
                .contains("Server certificate verification failed")
        );
    }
}
