//! Raw authenticated payload operations for message addons.

use whatsapp_rust::wacore::{event, poll};

use crate::{CoreError, CoreResult};

pub fn decrypt_poll_vote_payload(
    enc_payload: &[u8],
    enc_iv: &[u8],
    message_secret: &[u8],
    stanza_id: &str,
    poll_creator_jid: &str,
    voter_jid: &str,
) -> CoreResult<Vec<u8>> {
    poll::decrypt_poll_vote_payload_with_secret(
        poll::PollVoteCiphertext {
            enc_payload,
            enc_iv,
        },
        message_secret,
        stanza_id,
        poll_creator_jid,
        voter_jid,
    )
    .map_err(CoreError::from_display)
}

pub fn decrypt_event_response_payload(
    enc_payload: &[u8],
    enc_iv: &[u8],
    message_secret: &[u8],
    stanza_id: &str,
    event_creator_jid: &str,
    responder_jid: &str,
) -> CoreResult<Vec<u8>> {
    event::decrypt_event_response_payload_with_secret(
        enc_payload,
        enc_iv,
        message_secret,
        stanza_id,
        event_creator_jid,
        responder_jid,
    )
    .map_err(CoreError::from_display)
}
