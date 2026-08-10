//! Raw authenticated payload operations for message addons.

use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore::{event, poll};

use crate::wasm_utils::{byte_array, error_value};

#[wasm_bindgen(js_name = decryptPollVotePayload)]
pub fn decrypt_poll_vote_payload(
    enc_payload: &[u8],
    enc_iv: &[u8],
    message_secret: &[u8],
    stanza_id: &str,
    poll_creator_jid: &str,
    voter_jid: &str,
) -> Result<Uint8Array, JsValue> {
    let plaintext = poll::decrypt_poll_vote_payload_with_secret(
        poll::PollVoteCiphertext {
            enc_payload,
            enc_iv,
        },
        message_secret,
        stanza_id,
        poll_creator_jid,
        voter_jid,
    )
    .map_err(error_value)?;
    Ok(byte_array(&plaintext))
}

#[wasm_bindgen(js_name = decryptEventResponsePayload)]
pub fn decrypt_event_response_payload(
    enc_payload: &[u8],
    enc_iv: &[u8],
    message_secret: &[u8],
    stanza_id: &str,
    event_creator_jid: &str,
    responder_jid: &str,
) -> Result<Uint8Array, JsValue> {
    let plaintext = event::decrypt_event_response_payload_with_secret(
        enc_payload,
        enc_iv,
        message_secret,
        stanza_id,
        event_creator_jid,
        responder_jid,
    )
    .map_err(error_value)?;
    Ok(byte_array(&plaintext))
}
