//! Sending, editing and acknowledging messages.
//!
//! One of the per-domain `impl` blocks for [`WasmWhatsAppClient`];
//! see `wasm_client.rs` for the type, its construction and the shared
//! conversion helpers.

use super::*;

/// One receipt's worth of keys: the chat it addresses, the participant it
/// names when the chat has one, and the message ids it covers.
type ReceiptBatch = (Jid, Option<Jid>, Vec<String>);

/// Message keys grouped into the batches a receipt is sent per: one per
/// chat-and-participant pair, since that pair is what addresses the stanza.
///
/// Shared by the read and played receipts, which differ only in the call they
/// end with — grouping the same keys two different ways is a bug waiting for
/// one copy to be edited.
///
/// Every JID is parsed before any receipt is sent, so one unparseable key
/// costs the whole call. That is a change: parsing used to happen inside the
/// send loop, so a bad key left whatever batches `HashMap` iteration had
/// already reached delivered. Which ones those were was not something a caller
/// could predict, let alone repeat.
fn group_receipt_keys(
    field: &'static str,
    keys: JsValue,
) -> Result<Vec<ReceiptBatch>, crate::errors::BridgeError> {
    let keys = from_js_input::<Vec<crate::result_types::ReadMessageKey>>(field, keys)?;

    let mut grouped: HashMap<(String, Option<String>), Vec<String>> = HashMap::new();
    for key in keys {
        grouped
            .entry((key.remote_jid, key.participant))
            .or_default()
            .push(key.id);
    }

    grouped
        .into_iter()
        .map(|((chat, participant), ids)| {
            Ok((
                parse_jid(&chat)?,
                participant.as_deref().map(parse_jid).transpose()?,
                ids,
            ))
        })
        .collect()
}

include!("generated/messaging.rs");
