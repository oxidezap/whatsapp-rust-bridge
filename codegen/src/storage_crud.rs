use proc_macro2::TokenStream;
use quote::quote;
use std::collections::BTreeSet;
use syn::{Expr, Ident, Signature, parse_quote};
struct Trait {
    name: Ident,
    macro_name: Ident,
    operations: Vec<Crud>,
}
struct Crud {
    signature: Signature,
    namespace: Ident,
    key: Option<Key>,
    codec: Codec,
}
enum Key {
    Argument(Ident),
    Decimal(Ident),
    Hex(Ident),
    Literal(Ident),
    Field {
        argument: Ident,
        field: Ident,
    },
    Prefix {
        prefix: &'static str,
        argument: Ident,
    },
    Compound(Vec<Ident>),
}
enum Codec {
    GetRaw,
    GetBytes,
    GetJson,
    PutRaw(Expr),
    PutJson(Expr),
    Delete,
    PutMany(Ident),
    Compare(Ident),
}
fn catalog() -> Vec<Trait> {
    vec![
        Trait {
            name: parse_quote!(SignalStore),
            macro_name: parse_quote!(generated_signal_store),
            operations: vec![
                Crud {
                    signature: parse_quote!(async fn get_session(&self, address: &str) -> Result<Option<Bytes>>),
                    namespace: parse_quote!(STORE_SESSION),
                    key: Some(Key::Argument(parse_quote!(address))),
                    codec: Codec::GetBytes,
                },
                Crud {
                    signature: parse_quote!(async fn put_session(&self, address: &str, session: &[u8]) -> Result<()>),
                    namespace: parse_quote!(STORE_SESSION),
                    key: Some(Key::Argument(parse_quote!(address))),
                    codec: Codec::PutRaw(parse_quote!(session)),
                },
                Crud {
                    signature: parse_quote!(async fn delete_session(&self, address: &str) -> Result<()>),
                    namespace: parse_quote!(STORE_SESSION),
                    key: Some(Key::Argument(parse_quote!(address))),
                    codec: Codec::Delete,
                },
                Crud {
                    signature: parse_quote!(async fn put_sessions_batch(&self, sessions: &[(Arc<str>, Bytes)]) -> Result<()>),
                    namespace: parse_quote!(STORE_SESSION),
                    key: None,
                    codec: Codec::PutMany(parse_quote!(sessions)),
                },
                Crud {
                    signature: parse_quote!(async fn put_sender_key(&self, address: &str, record: &[u8]) -> Result<()>),
                    namespace: parse_quote!(STORE_SENDER_KEY),
                    key: Some(Key::Argument(parse_quote!(address))),
                    codec: Codec::PutRaw(parse_quote!(record)),
                },
                Crud {
                    signature: parse_quote!(async fn get_sender_key(&self, address: &str) -> Result<Option<Vec<u8>>>),
                    namespace: parse_quote!(STORE_SENDER_KEY),
                    key: Some(Key::Argument(parse_quote!(address))),
                    codec: Codec::GetRaw,
                },
                Crud {
                    signature: parse_quote!(async fn delete_sender_key(&self, address: &str) -> Result<()>),
                    namespace: parse_quote!(STORE_SENDER_KEY),
                    key: Some(Key::Argument(parse_quote!(address))),
                    codec: Codec::Delete,
                },
                Crud {
                    signature: parse_quote!(async fn put_sender_keys_batch(&self, sender_keys: &[(Arc<str>, Bytes)]) -> Result<()>),
                    namespace: parse_quote!(STORE_SENDER_KEY),
                    key: None,
                    codec: Codec::PutMany(parse_quote!(sender_keys)),
                },
                Crud {
                    signature: parse_quote!(async fn put_identity(&self, address: &str, key: [u8; 32]) -> Result<()>),
                    namespace: parse_quote!(STORE_IDENTITY),
                    key: Some(Key::Argument(parse_quote!(address))),
                    codec: Codec::PutRaw(parse_quote!(&key)),
                },
                Crud {
                    signature: parse_quote!(async fn delete_identity(&self, address: &str) -> Result<()>),
                    namespace: parse_quote!(STORE_IDENTITY),
                    key: Some(Key::Argument(parse_quote!(address))),
                    codec: Codec::Delete,
                },
                Crud {
                    signature: parse_quote!(async fn put_identities_batch(&self, identities: &[(Arc<str>, [u8; 32])]) -> Result<()>),
                    namespace: parse_quote!(STORE_IDENTITY),
                    key: None,
                    codec: Codec::PutMany(parse_quote!(identities)),
                },
                Crud {
                    signature: parse_quote!(async fn store_prekey(&self, id: u32, record: &[u8], _uploaded: bool) -> Result<()>),
                    namespace: parse_quote!(STORE_PREKEY),
                    key: Some(Key::Decimal(parse_quote!(id))),
                    codec: Codec::PutRaw(parse_quote!(record)),
                },
                Crud {
                    signature: parse_quote!(async fn load_prekey(&self, id: u32) -> Result<Option<Bytes>>),
                    namespace: parse_quote!(STORE_PREKEY),
                    key: Some(Key::Decimal(parse_quote!(id))),
                    codec: Codec::GetBytes,
                },
                Crud {
                    signature: parse_quote!(async fn remove_prekey(&self, id: u32) -> Result<()>),
                    namespace: parse_quote!(STORE_PREKEY),
                    key: Some(Key::Decimal(parse_quote!(id))),
                    codec: Codec::Delete,
                },
                Crud {
                    signature: parse_quote!(async fn load_signed_prekey(&self, id: u32) -> Result<Option<Vec<u8>>>),
                    namespace: parse_quote!(STORE_SIGNED_PREKEY),
                    key: Some(Key::Decimal(parse_quote!(id))),
                    codec: Codec::GetRaw,
                },
            ],
        },
        Trait {
            name: parse_quote!(AppSyncStore),
            macro_name: parse_quote!(generated_app_sync_store),
            operations: vec![
                Crud {
                    signature: parse_quote!(async fn get_version(&self, name: &str) -> Result<Option<HashState>>),
                    namespace: parse_quote!(STORE_SYNC_VERSION),
                    key: Some(Key::Argument(parse_quote!(name))),
                    codec: Codec::GetJson,
                },
                Crud {
                    signature: parse_quote!(async fn set_version(&self, name: &str, state: HashState) -> Result<()>),
                    namespace: parse_quote!(STORE_SYNC_VERSION),
                    key: Some(Key::Argument(parse_quote!(name))),
                    codec: Codec::PutJson(parse_quote!(&state)),
                },
                Crud {
                    signature: parse_quote!(async fn delete_version(&self, name: &str) -> Result<()>),
                    namespace: parse_quote!(STORE_SYNC_VERSION),
                    key: Some(Key::Argument(parse_quote!(name))),
                    codec: Codec::Delete,
                },
                Crud {
                    signature: parse_quote!(async fn get_sync_key(&self, key_id: &[u8]) -> Result<Option<AppStateSyncKey>>),
                    namespace: parse_quote!(STORE_SYNC_KEY),
                    key: Some(Key::Hex(parse_quote!(key_id))),
                    codec: Codec::GetJson,
                },
                Crud {
                    signature: parse_quote!(async fn get_latest_sync_key_id(&self) -> Result<Option<Vec<u8>>>),
                    namespace: parse_quote!(STORE_META),
                    key: Some(Key::Literal(parse_quote!(META_LATEST_SYNC_KEY_ID))),
                    codec: Codec::GetRaw,
                },
            ],
        },
        Trait {
            name: parse_quote!(ProtocolStore),
            macro_name: parse_quote!(generated_protocol_store),
            operations: vec![
                Crud {
                    signature: parse_quote!(async fn get_devices(&self, user: &str) -> Result<Option<DeviceListRecord>>),
                    namespace: parse_quote!(STORE_DEVICE_LIST),
                    key: Some(Key::Argument(parse_quote!(user))),
                    codec: Codec::GetJson,
                },
                Crud {
                    signature: parse_quote!(async fn update_device_list(&self, record: DeviceListRecord) -> Result<()>),
                    namespace: parse_quote!(STORE_DEVICE_LIST),
                    key: Some(Key::Field {
                        argument: parse_quote!(record),
                        field: parse_quote!(user),
                    }),
                    codec: Codec::PutJson(parse_quote!(&record)),
                },
                Crud {
                    signature: parse_quote!(async fn delete_devices(&self, user: &str) -> Result<()>),
                    namespace: parse_quote!(STORE_DEVICE_LIST),
                    key: Some(Key::Argument(parse_quote!(user))),
                    codec: Codec::Delete,
                },
                Crud {
                    signature: parse_quote!(async fn get_tc_token(&self, jid: &str) -> Result<Option<TcTokenEntry>>),
                    namespace: parse_quote!(STORE_TC_TOKEN),
                    key: Some(Key::Argument(parse_quote!(jid))),
                    codec: Codec::GetJson,
                },
                Crud {
                    signature: parse_quote!(async fn get_lid_mapping(&self, lid: &str) -> Result<Option<LidPnMappingEntry>>),
                    namespace: parse_quote!(STORE_LID_MAPPING),
                    key: Some(Key::Prefix {
                        prefix: "lid:",
                        argument: parse_quote!(lid),
                    }),
                    codec: Codec::GetJson,
                },
                Crud {
                    signature: parse_quote!(async fn save_base_key(&self, address: &str, message_id: &str, base_key: &[u8]) -> Result<()>),
                    namespace: parse_quote!(STORE_BASE_KEY),
                    key: Some(Key::Compound(vec![
                        parse_quote!(address),
                        parse_quote!(message_id),
                    ])),
                    codec: Codec::PutRaw(parse_quote!(base_key)),
                },
                Crud {
                    signature: parse_quote!(async fn delete_base_key(&self, address: &str, message_id: &str) -> Result<()>),
                    namespace: parse_quote!(STORE_BASE_KEY),
                    key: Some(Key::Compound(vec![
                        parse_quote!(address),
                        parse_quote!(message_id),
                    ])),
                    codec: Codec::Delete,
                },
                Crud {
                    signature: parse_quote!(async fn has_same_base_key(
        &self,
        address: &str,
        message_id: &str,
        current_base_key: &[u8],
    ) -> Result<bool>),
                    namespace: parse_quote!(STORE_BASE_KEY),
                    key: Some(Key::Compound(vec![
                        parse_quote!(address),
                        parse_quote!(message_id),
                    ])),
                    codec: Codec::Compare(parse_quote!(current_base_key)),
                },
            ],
        },
    ]
}
impl Crud {
    fn emit(&self) -> TokenStream {
        let signature = &self.signature;
        let namespace = &self.namespace;
        assert!(
            namespace.to_string().starts_with("STORE_"),
            "CRUD namespace must be explicit"
        );
        assert_eq!(
            self.key.is_none(),
            matches!(self.codec, Codec::PutMany(_)),
            "only batch writes omit a single key"
        );
        let (prepare, key) = match &self.key {
            None => (quote!(), quote!()),
            Some(Key::Argument(key) | Key::Literal(key)) => (quote!(), quote!(#key)),
            Some(Key::Field { argument, field }) => (quote!(), quote!(&#argument.#field)),
            Some(Key::Decimal(key)) => (quote!(), quote!(&#key.to_string())),
            Some(Key::Hex(key)) => (quote!(let key=to_hex(#key);), quote!(&key)),
            Some(Key::Prefix { prefix, argument }) => {
                let format = format!("{prefix}{{{argument}}}");
                (quote!(), quote!(&format!(#format)))
            }
            Some(Key::Compound(parts)) => {
                assert!(parts.len() > 1, "compound key needs components");
                let format = parts
                    .iter()
                    .map(|p| format!("{{{p}}}"))
                    .collect::<Vec<_>>()
                    .join(":");
                (quote!(let key=format!(#format);), quote!(&key))
            }
        };
        let body = match &self.codec {
            Codec::GetRaw => quote!(self.js_get(#namespace,#key).await),
            Codec::GetBytes => quote!(Ok(self.js_get(#namespace,#key).await?.map(Bytes::from))),
            Codec::GetJson => quote!(self.js_get_json(#namespace,#key).await),
            Codec::PutRaw(value) => quote!(self.js_set(#namespace,#key,#value).await),
            Codec::PutJson(value) => quote!(self.js_set_json(#namespace,#key,#value).await),
            Codec::Delete => quote!(self.js_delete(#namespace,#key).await),
            Codec::PutMany(values) => quote!(self.js_put_many(#namespace,pair_refs(#values)).await),
            Codec::Compare(value) => {
                quote!(match self.js_get(#namespace,#key).await? {Some(stored)=>Ok(stored==#value),None=>Ok(false)})
            }
        };
        quote!(#signature {#prepare #body})
    }
}
fn methods(declaration: &Trait) -> Vec<TokenStream> {
    let mut names = BTreeSet::new();
    declaration
        .operations
        .iter()
        .map(|operation| {
            assert!(
                names.insert(operation.signature.ident.to_string()),
                "duplicate storage operation"
            );
            operation.emit()
        })
        .collect()
}
#[cfg(test)]
pub fn emit() -> TokenStream {
    let mut output = TokenStream::new();
    for declaration in catalog() {
        let methods = methods(&declaration);
        let name = declaration.name;
        let macro_name = declaration.macro_name;
        output.extend(quote!(macro_rules! #macro_name { ($($manual:item)*) => {
            #[cfg_attr(target_arch="wasm32",async_trait(?Send))]
            #[cfg_attr(not(target_arch="wasm32"),async_trait)]
            impl #name for JsBackend { #(#methods)* $($manual)* }
        };}));
    }
    output
}
// rustfmt treats a macro body as opaque. Format the complete impl before inserting
// the item repetition, so generated methods remain reviewable like ordinary Rust.
pub fn readable(format: impl Fn(TokenStream) -> String) -> String {
    let mut output = String::new();
    for declaration in catalog() {
        let methods = methods(&declaration);
        let name = declaration.name;
        let inner = format(quote!(
            #[cfg_attr(target_arch="wasm32",async_trait(?Send))]
            #[cfg_attr(not(target_arch="wasm32"),async_trait)]
            impl #name for JsBackend { #(#methods)* fn __generated_manual_methods_marker() {} }
        ));
        let marker = "    fn __generated_manual_methods_marker() {}\n";
        assert_eq!(
            inner.matches(marker).count(),
            1,
            "formatter changed manual item marker"
        );
        let inner = inner.replace(marker, "    $($manual)*\n");
        output.push_str(&format!(
            "macro_rules! {} {{\n    ($($manual:item)*) => {{\n",
            declaration.macro_name
        ));
        for line in inner.lines() {
            output.push_str("        ");
            output.push_str(line);
            output.push('\n');
        }
        output.push_str("    };\n}\n");
    }
    output
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emits_complete_trait_macros_and_explicit_key_codecs() {
        syn::parse2::<syn::File>(emit()).unwrap();
    }
    #[test]
    fn unsupported_key_codec_combinations_are_rejected() {
        let mut traits = catalog();
        traits[0].operations[0].key = None;
        assert!(std::panic::catch_unwind(|| traits[0].operations[0].emit()).is_err());
    }
}
