use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
enum Support {
    Required,
    RequiredAtomic,
    Optional,
    DeclaredOnly,
}
#[derive(Clone, Copy)]
enum Capability {
    Batch,
    Enumerate,
    PrefixDelete,
}
struct Callback {
    name: &'static str,
    field: &'static str,
    support: Support,
    capability: Option<Capability>,
    parameters: &'static str,
    result: &'static str,
    doc: &'static str,
}
fn catalog() -> Vec<Callback> {
    vec![
        Callback {
            name: "get",
            field: "get_fn",
            support: Support::Required,
            capability: None,
            parameters: "store: string, key: string",
            result: "Uint8Array | null",
            doc: r###"/** Read one value by (store, key). Null/undefined if absent. MANDATORY. */"###,
        },
        Callback {
            name: "set",
            field: "set_fn",
            support: Support::Required,
            capability: None,
            parameters: "store: string, key: string, value: Uint8Array",
            result: "void",
            doc: r###"/** Write one value by (store, key). MANDATORY. */"###,
        },
        Callback {
            name: "delete",
            field: "delete_fn",
            support: Support::Required,
            capability: None,
            parameters: "store: string, key: string",
            result: "void",
            doc: r###"/** Delete one key. No-op if absent. MANDATORY. */"###,
        },
        Callback {
            name: "commitPatch",
            field: "commit_patch_fn",
            support: Support::RequiredAtomic,
            capability: None,
            parameters: "name: string, state: Uint8Array, removedKeys: string[], added: [key: string, value: Uint8Array][], maintainIndex: boolean",
            result: "void",
            doc: r###"/** Atomically commit `sync_version[name]` and `mutation_mac` removals/additions.
             * `state` is the exact JSON byte format used by set/get. Keys already include
             * the collection prefix. Remove before adding when a key occurs in both.
             * With maintainIndex, update `meta[mutation_mac_keys]` in the same transaction.
             * Failure/cancellation leaves the old state or the complete new state, never
             * a partial patch. Resolution confirms persistence; retain post-commit errors.
             * All writers in the device scope must share the transaction/locking mechanism.
             * There is no get/set or setMany fallback. */"###,
        },
        Callback {
            name: "touchTcTokenSenderTimestamp",
            field: "touch_tc_token_fn",
            support: Support::RequiredAtomic,
            capability: None,
            parameters: "jid: string, senderTimestamp: string",
            result: "void",
            doc: r###"/** Atomically max-merge the sender timestamp in `tc_token[jid]`.
             * Timestamp is a signed i64 decimal string. Preserve existing token bytes
             * and token_timestamp; an absent row becomes an empty-token placeholder with
             * token_timestamp=senderTimestamp. Add jid to `meta[tc_token_jids]` atomically.
             * Serialize against ALL row writers, including set/delete and expiry sweeps,
             * across every adapter/process in this device scope. No local-lock fallback. */"###,
        },
        Callback {
            name: "storeReceivedTcToken",
            field: "store_received_tc_token_fn",
            support: Support::RequiredAtomic,
            capability: None,
            parameters: "jid: string, token: Uint8Array, tokenTimestamp: string",
            result: "void",
            doc: r###"/** Atomically merge received token bytes and the signed i64 decimal timestamp.
             * Replace only a missing/empty-token row or one whose token_timestamp is no
             * newer. Equal timestamps accept incoming bytes. Preserve sender_timestamp,
             * using null on insertion. Add jid to `meta[tc_token_jids]` atomically.
             * Share serialization with every writer, including touch, set and delete.
             * Values retain the existing JSON byte layout; preserve exact i64 integers.
             * Resolution confirms persistence; thrown/rejected causes remain storage errors.
             * There is no non-atomic fallback. */"###,
        },
        Callback {
            name: "setMany",
            field: "set_many_fn",
            support: Support::Optional,
            capability: Some(Capability::Batch),
            parameters: "store: string, entries: [key: string, value: Uint8Array][]",
            result: "void",
            doc: r###"/**
   * Write many [key, value] pairs into ONE store in a single call. Entries are
   * tuples (so keys may contain any character). Best-effort: if the medium has
   * no cross-key atomicity (file-per-key) it MUST still apply every entry and
   * fail-fast on error so the core can retry (writes are idempotent by key).
   * Empty array is a valid no-op.
   */"###,
        },
        Callback {
            name: "deleteMany",
            field: "delete_many_fn",
            support: Support::Optional,
            capability: Some(Capability::Batch),
            parameters: "store: string, keys: string[]",
            result: "void",
            doc: r###"/** Delete many keys from ONE store in a single call. Missing keys ignored. */"###,
        },
        Callback {
            name: "getMany",
            field: "get_many_fn",
            support: Support::Optional,
            capability: Some(Capability::Batch),
            parameters: "store: string, keys: string[]",
            result: "[key: string, value: Uint8Array][]",
            doc: r###"/** Read many keys from ONE store; one entry per FOUND key, any order. */"###,
        },
        Callback {
            name: "listKeys",
            field: "list_keys_fn",
            support: Support::Optional,
            capability: Some(Capability::Enumerate),
            parameters: "store: string, prefix?: string",
            result: "string[]",
            doc: r###"/** Enumerate live keys in `store` (optionally prefix-filtered). Unordered. */"###,
        },
        Callback {
            name: "deletePrefix",
            field: "delete_prefix_fn",
            support: Support::Optional,
            capability: Some(Capability::PrefixDelete),
            parameters: "store: string, prefix: string",
            result: "number",
            doc: r###"/** Delete every key in `store` starting with `prefix`. Returns count removed. */"###,
        },
        Callback {
            name: "listEntries",
            field: "listEntries_fn",
            support: Support::DeclaredOnly,
            capability: None,
            parameters: "store: string, prefix?: string",
            result: "[key: string, value: Uint8Array][]",
            doc: r###"/**
   * Like listKeys but returns [key, value] pairs, so the core can inspect the
   * embedded timestamp prefix for delete-expired sweeps without N follow-up
   * gets. If absent but listKeys exists, the core falls back to listKeys+getMany.
   */"###,
        },
        Callback {
            name: "flush",
            field: "flush_fn",
            support: Support::DeclaredOnly,
            capability: None,
            parameters: "",
            result: "void",
            doc: r###"/** Optional durability barrier (flush pending writes). */"###,
        },
    ]
}

fn typescript(callbacks: &[Callback]) -> String {
    let mut result = String::from(INTERFACE_DOC);
    result.push_str("\nexport interface JsStoreCallbacks {\n");
    for callback in callbacks {
        result.push_str(callback.doc);
        result.push('\n');
        result.push_str(&format!(
            "  {}{}({}): Promise<{}>;\n",
            callback.name,
            if matches!(
                callback.support,
                Support::Required | Support::RequiredAtomic
            ) {
                ""
            } else {
                "?"
            },
            callback.parameters,
            callback.result
        ));
    }
    result.push_str(
        r#"  /** Static capability declaration, read once at init. Omitted => all false. */
  capabilities?: {
    /** setMany/getMany/deleteMany are implemented. */
    batch?: boolean;
    /** listKeys/listEntries reliably enumerate a namespace. */
    enumerate?: boolean;
    /** deletePrefix is implemented. */
    prefixDelete?: boolean;
  };
}
"#,
    );
    result
}

pub fn emit() -> TokenStream {
    let callbacks = catalog();
    let mut names = BTreeSet::new();
    let mut fields = BTreeSet::new();
    let mut declarations = Vec::new();
    let mut extraction = Vec::new();
    let mut initialization = Vec::new();
    for callback in &callbacks {
        assert!(names.insert(callback.name), "duplicate storage callback");
        if matches!(callback.support, Support::DeclaredOnly) {
            continue;
        }
        assert!(fields.insert(callback.field), "duplicate storage handle");
        let field = format_ident!("{}", callback.field);
        let name = callback.name;
        if matches!(
            callback.support,
            Support::Required | Support::RequiredAtomic
        ) {
            let atomic = matches!(callback.support, Support::RequiredAtomic);
            declarations.push(if atomic {
                quote!(pub #field: Option<js_sys::Function>,)
            } else {
                quote!(pub #field: js_sys::Function,)
            });
            let required = format!("store.{name} is required");
            let wrong_type = format!("store.{name} must be a function");
            extraction.push(quote!(
                let #field = js_sys::Reflect::get(store_val, &#name.into())
                    .map_err(|_| crate::errors::invalid_arg("store", #required))?
                    .dyn_into::<js_sys::Function>()
                    .map_err(|_| crate::errors::invalid_arg("store", #wrong_type))?;
            ));
            if atomic {
                extraction.push(quote!(let #field = Some(#field);));
            }
            initialization.push(quote!(#field,));
        } else {
            declarations.push(quote!(pub #field: Option<js_sys::Function>,));
            extraction.push(quote!(let #field = opt_fn(#name);));
            initialization.push(if matches!(callback.capability, Some(Capability::Batch)) {
                quote!(#field: if cap_batch { #field } else { None },)
            } else {
                quote!(#field,)
            });
        }
    }
    let ts = typescript(&callbacks);
    // Storage deliberately tolerates throwing/non-function optional getters.
    // Event callbacks have a different error policy and use their own parser.
    quote!(
        pub(crate) struct JsBackendHandles {
            #(#declarations)*
            pub cap_enumerate: bool,
            pub cap_prefix_delete: bool,
        }
        impl JsBackendHandles {
            pub(crate) fn from_js(store_val: &JsValue) -> std::result::Result<Self, crate::errors::BridgeError> {
                let opt_fn = |name: &str| {
                    js_sys::Reflect::get(store_val, &name.into()).ok()
                        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
                };
                #(#extraction)*
                let cap = |field: &str| {
                    js_sys::Reflect::get(store_val, &"capabilities".into()).ok()
                        .and_then(|c| js_sys::Reflect::get(&c, &field.into()).ok())
                        .and_then(|v| v.as_bool()).unwrap_or(false)
                };
                let cap_batch = cap("batch");
                let cap_enumerate = cap("enumerate");
                let cap_prefix_delete = cap("prefixDelete");
                log::info!("Using JS-backed persistent storage (batch={}, enumerate={}, prefixDelete={})",
                    cap_batch, cap_enumerate, cap_prefix_delete);
                Ok(Self { #(#initialization)* cap_enumerate, cap_prefix_delete })
            }
        }
        #[wasm_bindgen(typescript_custom_section)]
        const STORAGE_CALLBACK_TYPES: &'static str = #ts;
    )
}

const INTERFACE_DOC: &str = r###"/**
 * JS storage callbacks for the persistent backend.
 *
 * The boundary is a two-level namespaced key/value store: `store` is one of the
 * fixed STORE_* namespaces (e.g. "session", "msg_secret", "lid_mapping") and
 * `key` is an opaque, namespace-scoped id; values are raw bytes.
 *
 * All six CRUD and atomic callbacks are mandatory at construction.
 * Missing or non-function callbacks reject before backend initialization.
 * `get`/`set`/`delete` provide ordinary CRUD. The atomic callbacks are required
 * to commit app-state patches and merge trusted-contact tokens: a three-method
 * store cannot implement those operations safely. They never fall back to
 * separate CRUD calls or non-transactional batches.
 *
 * The remaining methods are optional performance/structural primitives:
 *   - `setMany`/`deleteMany` collapse N per-key FFI crossings into one (this is
 *     what turns a ~20k-secret history-sync write from 20k awaits into a single
 *     batched call).
 *   - `listKeys`/`listEntries` let the core enumerate a namespace directly,
 *     which lets it DROP its hand-maintained meta-index lists (msg_secret_keys,
 *     tc_token_jids, …). A host that cannot enumerate a category (e.g. an
 *     id-addressed external key store) simply omits them; the core then keeps its
 *     self-maintained index for that backend.
 *   - `deletePrefix` accelerates unconditional bulk clears.
 *
 * `capabilities` is read ONCE at init. Omit it (or a field) and the core treats
 * the corresponding primitive as absent. A capability declared `true` MUST have
 * its method(s) present and working.
 */"###;
