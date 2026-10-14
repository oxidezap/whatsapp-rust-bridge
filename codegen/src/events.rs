use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Type, parse_quote};

struct Callback {
    field: Ident,
    constant: Ident,
    method: &'static str,
    missing_warning: Option<&'static str>,
    doc: &'static str,
    parameters: &'static str,
    result: &'static str,
    required_in_types: bool,
}
struct PackedChannel {
    field: Ident,
    copying: Callback,
    borrowing: Callback,
    batch: Type,
    segment: Ident,
}
fn callback(
    field: Ident,
    constant: Ident,
    method: &'static str,
    doc: &'static str,
    parameters: &'static str,
    result: &'static str,
    required_in_types: bool,
) -> Callback {
    Callback {
        field,
        constant,
        method,
        missing_warning: None,
        doc,
        parameters,
        result,
        required_in_types,
    }
}
fn required() -> Callback {
    callback(
        parse_quote!(on_event),
        parse_quote!(EVENT_CALLBACK_METHOD),
        "onEvent",
        "",
        "event: WhatsAppEvent",
        "void",
        true,
    )
}
fn ordinary() -> Vec<Callback> {
    vec![
        Callback {
            missing_warning: Some(
                "event callbacks lack onMessageBatch; message events will be dropped",
            ),
            ..callback(
                parse_quote!(on_message_batch),
                parse_quote!(MESSAGE_BATCH_CALLBACK_METHOD),
                "onMessageBatch",
                "/**\n   * Protobuf-wire message path. The bridge packs a bounded ordered group of\n   * messages — payloads and metadata alike — into one flat buffer. Decode it\n   * with `decodeMessageWireBatch`.\n   *\n   * Decode every batch, exactly once, in the order it arrives, before calling\n   * back into the client: addresses and push names repeat, so a batch defines\n   * them once and later batches reference the table the decoder is holding.\n   * A batch skipped or decoded out of order leaves that table describing a\n   * history the records do not have.\n   *\n   * The return type is `void` and TypeScript lets an `async` method satisfy\n   * it, so this is checked rather than trusted: a callback that hands back\n   * anything promise-like has not decoded inside its call, and the bridge\n   * gives up the cross-batch table for the rest of the session — every batch\n   * then carries every value it names, which decodes the same in any order.\n   * Keeping the buffer is fine; decoding it later is what costs the table.\n   *\n   * The check runs after the batch has been handed over, so the batch that\n   * revealed the violation is the one it cannot save: decoded after a later\n   * one, it reads that one's table. Every batch after it is safe whenever it\n   * is decoded.\n   */",
                "batch: MessageWireBatch",
                "void",
                true,
            )
        },
        Callback {
            missing_warning: Some(
                "event callbacks lack onHistorySyncBatch; history-sync events will be dropped",
            ),
            ..callback(
                parse_quote!(on_history_sync_batch),
                parse_quote!(HISTORY_SYNC_BATCH_CALLBACK_METHOD),
                "onHistorySyncBatch",
                "/**\n   * Protobuf-wire history-sync path. Conversation entries cross as wire bytes\n   * and the non-conversation remainder (pushnames, mappings, settings, ...)\n   * crosses as one encoded `proto.HistorySync` payload in `remainderData`.\n   * Return the number of malformed entries skipped by the host, if any.\n   */",
                "batch: HistorySyncWireBatch",
                "number | void",
                true,
            )
        },
        callback(
            parse_quote!(on_event_batch),
            parse_quote!(EVENT_BATCH_CALLBACK_METHOD),
            "onEventBatch",
            "/**\n   * Optional coalescing path. A live message produces up to three batches (its\n   * own, its receipt, its ack); with this method the ones the dispatch loop\n   * already holds cross together as one envelope of tagged segments instead of\n   * one crossing each. Split it with `decodeEventWireEnvelope` and hand each\n   * segment to the codec its kind names, in order. Nothing is ever held back\n   * waiting for a companion event, so a batch with no company still arrives\n   * through its own callback above, in the buffer that callback negotiated.\n   *\n   * There is no borrowing form, for the reason `onMessageBatch` has none: an\n   * envelope may carry a message segment, whose decode returns views over the\n   * buffer.\n   */",
            "batch: EventWireEnvelope",
            "void",
            false,
        ),
    ]
}
fn packed() -> Vec<PackedChannel> {
    vec![
        PackedChannel {
            field: parse_quote!(receipt_channel),
            copying: callback(
                parse_quote!(copying),
                parse_quote!(RECEIPT_BATCH_CALLBACK_METHOD),
                "onReceiptBatch",
                "/**\n   * Optional packed receipt path: adjacent `receipt` events coalesce into one\n   * flat buffer (decode with `decodeReceiptWireBatch`) instead of one\n   * reflected object per event. Without it, receipts use `onEvent`.\n   * Decode the batch fully before calling back into the client: the reader\n   * walks the buffer in order and shares cached JID objects across events.\n   */",
                "batch: ReceiptWireBatch",
                "void",
                false,
            ),
            borrowing: callback(
                parse_quote!(borrowing),
                parse_quote!(RECEIPT_BATCH_BORROWED_CALLBACK_METHOD),
                "onReceiptBatchBorrowed",
                "/**\n   * Borrowing form of `onReceiptBatch`, and the whole of the opt-in: declaring\n   * it replaces `onReceiptBatch` as the receipt sink and lets the bridge hand\n   * every batch out of one buffer it reuses.\n   *\n   * The batch is therefore a window on shared memory, valid ONLY for the\n   * synchronous duration of the call: implementations MUST decode or copy it\n   * before returning, and MUST NOT retain it, pass it to an async consumer, or\n   * return a Promise. `decodeReceiptWireBatch` satisfies that on its own, since\n   * it materializes every field and keeps no view over the buffer, so the usual\n   * body is a decode plus whatever the host does with the result. A callback\n   * caught handing back something promise-like, or letting an exception escape,\n   * drops every borrowing callback of every packed kind back to a buffer per\n   * batch for the rest of the session, before the buffer is reused, so the\n   * window it kept stays intact.\n   *\n   * `onReceiptBatch` remains the copying path for every host that does not opt\n   * in, and a batch too large for the shared buffer gets its own regardless.\n   * There is no borrowing form of `onMessageBatch`: `decodeMessageWireBatch`\n   * returns views over the batch, so reuse there would alias the decoded result\n   * and not just the buffer.\n   */",
                "batch: ReceiptWireBatch",
                "void",
                false,
            ),
            batch: parse_quote!(ReceiptWireBatch),
            segment: parse_quote!(EVENT_SEGMENT_KIND_RECEIPT),
        },
        PackedChannel {
            field: parse_quote!(server_ack_channel),
            copying: callback(
                parse_quote!(copying),
                parse_quote!(SERVER_ACK_BATCH_CALLBACK_METHOD),
                "onServerAckBatch",
                "/**\n   * Optional packed server-ack path, analogous to `onReceiptBatch`; decode\n   * with `decodeServerAckWireBatch`. Without it, acks use `onEvent`.\n   */",
                "batch: ServerAckWireBatch",
                "void",
                false,
            ),
            borrowing: callback(
                parse_quote!(borrowing),
                parse_quote!(SERVER_ACK_BATCH_BORROWED_CALLBACK_METHOD),
                "onServerAckBatchBorrowed",
                "/** Borrowing form of `onServerAckBatch`, under the contract above. */",
                "batch: ServerAckWireBatch",
                "void",
                false,
            ),
            batch: parse_quote!(ServerAckWireBatch),
            segment: parse_quote!(EVENT_SEGMENT_KIND_SERVER_ACK),
        },
    ]
}
fn typescript(required: &Callback, ordinary: &[Callback], packed: &[PackedChannel]) -> String {
    let mut output = String::from(
        "/**\n * Typed event sink. Message and history-sync events cross the boundary as\n * protobuf wire bytes only: the host decodes them with its own codec, so the\n * bridge never materializes an intermediate reflected JS tree (and never\n * compiles the Rust->JS serializers for those proto graphs). A handler that\n * omits `onMessageBatch`/`onHistorySyncBatch` has those events dropped with an\n * error log; every other event kind continues through `onEvent`.\n */\nexport interface WhatsAppEventCallbacks {\n",
    );
    let mut append = |callback: &Callback| {
        if !callback.doc.is_empty() {
            output.push_str(callback.doc);
            output.push('\n');
        }
        output.push_str(&format!(
            "  {}{}({}): {};\n",
            callback.method,
            if callback.required_in_types { "" } else { "?" },
            callback.parameters,
            callback.result
        ));
    };
    append(required);
    append(&ordinary[0]);
    append(&ordinary[1]);
    append(&packed[0].copying);
    append(&packed[1].copying);
    append(&packed[0].borrowing);
    append(&packed[1].borrowing);
    append(&ordinary[2]);
    output.push_str("/**\n   * Optional host-interest filter for conversation records. When present, the\n   * bridge still walks every history payload and emits its final metadata, but\n   * materializes conversation wire bytes only for the listed numeric sync\n   * types. Omit it for the backward-compatible \"all types\" behavior.\n   */\n  historySyncConversationTypes?: readonly number[];\n}\n");
    output
}
pub fn emit() -> TokenStream {
    let required = required();
    let ordinary = ordinary();
    let packed = packed();
    let mut names = std::collections::BTreeSet::new();
    let mut constants = std::collections::BTreeSet::new();
    for callback in std::iter::once(&required).chain(ordinary.iter()).chain(
        packed
            .iter()
            .flat_map(|channel| [&channel.copying, &channel.borrowing]),
    ) {
        assert!(
            names.insert(callback.method),
            "duplicate event callback method"
        );
        assert!(
            constants.insert(callback.constant.to_string()),
            "duplicate event callback constant"
        );
    }
    let typescript = typescript(&required, &ordinary, &packed);
    let required_constant = &required.constant;
    let required_method = required.method;
    let constants = ordinary
        .iter()
        .chain(
            packed
                .iter()
                .flat_map(|channel| [&channel.copying, &channel.borrowing]),
        )
        .map(|callback| {
            let name = &callback.constant;
            let method = callback.method;
            quote!(const #name:&str=#method;)
        });
    let fields = ordinary
        .iter()
        .map(|callback| &callback.field)
        .collect::<Vec<_>>();
    let channels = packed
        .iter()
        .map(|channel| &channel.field)
        .collect::<Vec<_>>();
    let registration = |callback: &Callback| {
        let field = &callback.field;
        let constant = &callback.constant;
        quote!(let #field=Self::optional_method(&value,#constant)?;)
    };
    // Preserve getter order: message, history, each packed pair, event envelope,
    // then the history filter. A throwing getter is an observable registration error.
    let first = ordinary[..2].iter().map(registration);
    let last = registration(&ordinary[2]);
    let channel_registration=packed.iter().map(|channel| {let field=&channel.field;let copying=&channel.copying.constant;let borrowing=&channel.borrowing.constant;quote!(let #field=PackedBatchChannelState {copying:Self::optional_method(&value,#copying)?,borrowing:Self::optional_method(&value,#borrowing)?};)});
    let warnings = ordinary.iter().filter_map(|callback| {
        callback.missing_warning.map(|warning| {
            let field = &callback.field;
            quote!(if #field.is_none(){log::warn!(#warning);})
        })
    });
    let associations=packed.iter().map(|channel| {let field=&channel.field;let batch=&channel.batch;quote!(impl PackedBatchChannel for #batch {fn channel(callbacks:&JsEventCallbacks)->&PackedBatchChannelState{&callbacks.#field}})});
    let cases = packed.iter().map(|channel| {
        let field = &channel.field;
        let batch = &channel.batch;
        let segment = &channel.segment;
        quote!(#segment=>Some((#batch::KIND,&self.#field)),)
    });
    quote!(
        const #required_constant:&str=#required_method;
        #[wasm_bindgen(typescript_custom_section)]
        const GENERATED_EVENT_CALLBACKS_TS:&str=#typescript;
        #(#constants)*
        /// Callback handles resolved once; dispatch keeps the explicit receiver.
        struct JsEventCallbacks {
            receiver:JsValue,
            on_event:js_sys::Function,
            #(#fields:Option<js_sys::Function>,)*
            #(#channels:PackedBatchChannelState,)*
            history_sync_conversation_types:Option<u128>,
        }
        impl JsEventCallbacks {
            fn from_js(value:JsValue)->Result<Self,crate::errors::BridgeError> {
                if value.is_function(){return Ok(Self {receiver:JsValue::NULL,on_event:value.unchecked_into(),#(#fields:None,)*#(#channels:PackedBatchChannelState::default(),)*history_sync_conversation_types:None});}
                if !value.is_object()||value.is_null(){return Err(crate::errors::invalid_arg("on_event","must be a function or an object with an onEvent method"));}
                let on_event=js_sys::Reflect::get(&value,&EVENT_CALLBACK_METHOD.into()).map_err(|_|crate::errors::invalid_arg("on_event","could not read the onEvent method"))?.dyn_into::<js_sys::Function>().map_err(|_|crate::errors::invalid_arg("on_event.onEvent","must be a function"))?;
                #(#first)* #(#channel_registration)* #last
                let history_sync_conversation_types=Self::optional_history_sync_conversation_types(&value)?;
                #(#warnings)*
                Ok(Self {receiver:value,on_event,#(#fields,)*#(#channels,)*history_sync_conversation_types})
            }
            fn optional_method(receiver:&JsValue,method:&'static str)->Result<Option<js_sys::Function>,crate::errors::BridgeError>{
                let value=js_sys::Reflect::get(receiver,&method.into()).map_err(|_|crate::errors::invalid_arg("on_event",format!("could not read the optional {method} method")))?;
                if value.is_null()||value.is_undefined(){return Ok(None);}
                value.dyn_into::<js_sys::Function>().map(Some).map_err(|_|crate::errors::invalid_arg(format!("on_event.{method}"),"must be a function"))
            }
            fn channel_of_kind(&self,kind:u32)->Option<(&'static str,&PackedBatchChannelState)> {match kind {#(#cases)*_=>None}}
        }
        #(#associations)*
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_catalog_has_distinct_callbacks_and_valid_rust() {
        let required = required();
        let ordinary = ordinary();
        let packed = packed();
        let mut names = std::collections::BTreeSet::new();
        for callback in std::iter::once(&required).chain(ordinary.iter()).chain(
            packed
                .iter()
                .flat_map(|channel| [&channel.copying, &channel.borrowing]),
        ) {
            assert!(names.insert(callback.method));
        }
        assert_eq!(names.len(), 8);
        syn::parse2::<syn::File>(emit()).unwrap();
    }
}
