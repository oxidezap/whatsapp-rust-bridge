use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, Ident, ItemUse, LitStr, Path, Type, parse_quote};

pub enum InputCodec {
    Scalar {
        binding: Ident,
        value: Expr,
        ty: Type,
        field: LitStr,
        prefix: Option<LitStr>,
        optional: bool,
    },
    OptionalMillis {
        binding: Ident,
        value: Expr,
        field: LitStr,
    },
    GroupText {
        binding: Ident,
        value: Expr,
        description: bool,
    },
    MessageKey {
        binding: Ident,
        chat: Expr,
        key: Ident,
    },
    MessageReference {
        chat: Expr,
        from_me: Expr,
        sender: Expr,
    },
    ParentKeyGuard {
        key: Ident,
    },
    SendOptions {
        binding: Ident,
        status: bool,
        extra_nodes: Option<Expr>,
        group_freshness: Option<Expr>,
        device_freshness: Option<Expr>,
        message_id: Box<Expr>,
    },
    MessageBytes {
        binding: Ident,
        value: Expr,
    },
    Import(ItemUse),
    Enum {
        binding: Ident,
        value: Expr,
        source: Path,
        target: Path,
        variants: Vec<(Ident, Ident)>,
    },
    JidGuard {
        value: Expr,
        field: LitStr,
        reason: LitStr,
    },
    FullOrBareJid {
        binding: Ident,
        value: Expr,
        family: JidFamily,
        list: bool,
    },
    DefaultOverlay {
        binding: Ident,
        input: Ident,
        ty: Type,
        optional: Vec<(Ident, Ident)>,
        direct: Vec<(Ident, Ident)>,
    },
}
pub enum JidFamily {
    Pn,
    Lid,
}
impl InputCodec {
    pub fn emit(&self) -> TokenStream {
        match self {
            Self::Scalar {
                binding,
                value,
                ty,
                field,
                prefix,
                optional,
            } => {
                let reason = prefix.as_ref().map_or_else(
                    || quote!(error.to_string()),
                    |prefix| {
                        let format = format!("{}{{error}}", prefix.value());
                        quote!(format!(#format))
                    },
                );
                let parse = quote!(value.parse::<#ty>().map_err(|error|crate::errors::invalid_arg(#field,#reason)));
                if *optional {
                    quote!(let #binding=#value.as_deref().map(|value|#parse).transpose()?;)
                } else {
                    quote!(let #binding={let value=#value;#parse}?;)
                }
            }
            Self::OptionalMillis {
                binding,
                value,
                field,
            } => {
                quote!(let #binding=#value.map(|timestamp|parse_timestamp_ms(#field,timestamp)).transpose()?;)
            }
            Self::GroupText {
                binding,
                value,
                description,
            } => {
                if *description {
                    quote!(let #binding=#value.as_deref().map(whatsapp_rust::features::GroupDescription::new).transpose()?;)
                } else {
                    quote!(let #binding=whatsapp_rust::features::GroupSubject::new(#value)?;)
                }
            }
            Self::MessageKey { binding, chat, key } => {
                quote!(let mut #binding=waproto::whatsapp::MessageKey::default();#binding.remote_jid=Some(#chat.to_string());#binding.from_me=Some(#key.from_me);#binding.id=Some(#key.id);#binding.participant=#key.participant;)
            }
            Self::MessageReference {
                chat,
                from_me,
                sender,
            } => {
                quote!(let message_id=whatsapp_rust::MessageId::new(message_id).map_err(|error|crate::errors::invalid_arg("messageId",error.to_string()))?;let target=whatsapp_rust::MessageRef::new(&#chat,message_id,#sender,#from_me).map_err(|error|crate::errors::message_reference_error(&error,"messageId","jid","participant"))?;)
            }
            Self::ParentKeyGuard { key } => {
                quote!(if #key.participant.is_none()&&!#key.from_me {return Err(crate::errors::invalid_arg("parent_key","needs participant (the post author) or fromMe: true"));})
            }
            Self::SendOptions {
                binding,
                status,
                extra_nodes,
                group_freshness,
                device_freshness,
                message_id,
            } => {
                assert!(
                    !(*status && group_freshness.is_some()),
                    "status options do not have group freshness"
                );
                let ty: Path = if *status {
                    parse_quote!(whatsapp_rust::StatusSendOptions)
                } else {
                    parse_quote!(whatsapp_rust::SendOptions)
                };
                let nodes = extra_nodes
                    .as_ref()
                    .map(|value| quote!(.with_extra_stanza_nodes(js_node_array_to_vec(#value)?)));
                let group = group_freshness
                    .as_ref()
                    .map(|value| quote!(.with_group_metadata_freshness(freshness(#value))));
                let devices = device_freshness
                    .as_ref()
                    .map(|value| quote!(.with_device_freshness(freshness(#value))));
                quote!(let mut #binding=#ty::default()#nodes #group #devices;if let Some(message_id)=#message_id {#binding=#binding.with_message_id(whatsapp_rust::MessageId::new(message_id).map_err(|error|crate::errors::invalid_arg("messageId",error.to_string()))?);})
            }
            Self::MessageBytes { binding, value } => {
                quote!(let #binding=waproto::codec::message_decode(#value).map_err(|error|crate::errors::internal(format!("invalid message bytes: {error}")))?;)
            }
            Self::Import(import) => quote!(#import),
            Self::Enum {
                binding,
                value,
                source,
                target,
                variants,
            } => {
                let mut seen = std::collections::BTreeSet::new();
                let arms = variants.iter().map(|(from, to)| {
                    assert!(
                        seen.insert(from.to_string()),
                        "duplicate enum source variant"
                    );
                    quote!(#source::#from=>#target::#to,)
                });
                quote!(let #binding=match #value {#(#arms)*};)
            }
            Self::JidGuard {
                value,
                field,
                reason,
            } => {
                quote!(if !#value.is_group(){return Err(crate::errors::invalid_arg(#field,#reason));})
            }
            Self::FullOrBareJid {
                binding,
                value,
                family,
                list,
            } => {
                let factory: Ident = match family {
                    JidFamily::Pn => parse_quote!(pn),
                    JidFamily::Lid => parse_quote!(lid),
                };
                if *list {
                    quote!(let #binding:Vec<Jid>=#value.iter().map(|jid|{if jid.contains('@'){parse_jid(jid)}else{Ok(Jid::#factory(jid))}}).collect::<Result<Vec<_>,_>>()?;)
                } else {
                    quote!(let #binding=if #value.contains('@'){parse_jid(#value)?}else{Jid::#factory(#value)};)
                }
            }
            Self::DefaultOverlay {
                binding,
                input,
                ty,
                optional,
                direct,
            } => {
                let mut fields = std::collections::BTreeSet::new();
                let optional = optional
                    .iter()
                    .map(|(source, target)| {
                        assert!(
                            fields.insert(target.to_string()),
                            "duplicate default overlay field"
                        );
                        quote!(if let Some(value)=input.#source {#binding.#target=value;})
                    })
                    .collect::<Vec<_>>();
                let direct = direct
                    .iter()
                    .map(|(source, target)| {
                        assert!(
                            fields.insert(target.to_string()),
                            "duplicate default overlay field"
                        );
                        quote!(#binding.#target=input.#source;)
                    })
                    .collect::<Vec<_>>();
                quote!(let input=#input.unwrap_or_default();let mut #binding=#ty::default();#(#optional)*#(#direct)*)
            }
        }
    }
}

pub enum OutputCodec {
    Variant(VariantCodec),
    Serialize {
        serializer: Serializer,
        member: Option<Ident>,
        optional: bool,
        cast: bool,
        error: SerializationError,
    },
    StringMap {
        member: Ident,
        key: Ident,
        value: Ident,
    },
    Object(ObjectShape),
    Value {
        codec: ValueCodec,
        error: Option<ResultError>,
    },
    Converted {
        converter: Converter,
        field: Option<Ident>,
        list: bool,
    },
    CommunityLink {
        groups: Ident,
    },
    GroupRecord {
        source_key: bool,
    },
}
pub enum ValueCodec {
    Identity,
    AllSucceeded,
    CheckedU32(&'static str),
    GroupJid,
    StanzaId,
    MessageId,
    NumberF64,
    Bytes,
    OwnedByteArray,
    True,
    OptionalAddress { field: Ident, server: LitStr },
}
pub enum ResultError {
    From,
    SendKey(LitStr),
    SignalType,
}
pub enum Converter {
    GroupMetadata,
    NewsletterMetadata,
    BotList,
}
pub enum VariantCodec {
    UsernameLookup,
    MediaRetry,
    PushName,
}
pub enum Serializer {
    Serde,
    Camel,
    NodeRef,
    Proto,
}
pub enum SerializationError {
    From,
    Display(&'static str),
    Debug(&'static str),
}
impl OutputCodec {
    pub fn emit(&self, call: TokenStream) -> TokenStream {
        match self {
            Self::Variant(VariantCodec::UsernameLookup) => {
                quote!(to_ts(crate::result_types::project_username_lookup(#call)?))
            }
            Self::Variant(VariantCodec::PushName) => {
                quote!(match #call {
                    whatsapp_rust::features::PushNameOutcome::Synced => Ok(()),
                    whatsapp_rust::features::PushNameOutcome::SyncPending { source } => Err(crate::errors::BridgeError::from(source)),
                    other => Err(crate::errors::internal(format!("unhandled push-name outcome: {other:?}"))),
                })
            }
            Self::Variant(VariantCodec::MediaRetry) => {
                quote!(crate::result_types::project_media_retry(#call))
            }
            Self::StringMap { member, key, value } => {
                quote!({let value=#call;let map:std::collections::HashMap<&str,&str>=value.#member.iter().map(|entry|(entry.#key.as_str(),entry.#value.as_str())).collect();serde_wasm_bindgen::to_value(&map).map_err(crate::errors::BridgeError::from)})
            }
            Self::Serialize {
                serializer,
                member,
                optional,
                cast,
                error,
            } => {
                let encode = match serializer {
                    Serializer::Serde => quote!(serde_wasm_bindgen::to_value(value)),
                    Serializer::Camel => quote!(crate::camel_serializer::to_js_value_camel(value)),
                    Serializer::NodeRef => quote!(node_ref_to_js(value.get())),
                    Serializer::Proto => quote!(crate::proto::to_js_value(value)),
                };
                let error = match error {
                    SerializationError::From => quote!(crate::errors::BridgeError::from),
                    SerializationError::Display(prefix) | SerializationError::Debug(prefix) => {
                        let format = if matches!(error, SerializationError::Debug(_)) {
                            format!("{prefix}{{error:?}}")
                        } else {
                            format!("{prefix}{{error}}")
                        };
                        quote!(|error| crate::errors::internal(format!(#format)))
                    }
                };
                let encoded = if *cast {
                    quote!(Ok(#encode.map_err(#error)?.unchecked_into()))
                } else {
                    quote!(#encode.map_err(#error))
                };
                let source = member
                    .as_ref()
                    .map_or_else(|| quote!(source), |member| quote!(source.#member));
                if *optional {
                    quote!(let source=#call;match &#source {Some(value)=>#encoded,None=>Ok(JsValue::UNDEFINED),})
                } else {
                    quote!(let source=#call;let value=&#source;#encoded)
                }
            }
            Self::Object(shape) => shape.emit(call),
            Self::Value { codec, error } => {
                if matches!(codec, ValueCodec::True) {
                    assert!(matches!(error, Some(ResultError::From)));
                    return quote!(#call?;Ok(true));
                }
                let unwrap = match error {
                    None => quote!(),
                    Some(ResultError::From) => quote!(let value=value?;),
                    Some(ResultError::SendKey(key)) => {
                        quote!(let value=value.map_err(|error|crate::errors::send_error_for_key(error,#key))?;)
                    }
                    Some(ResultError::SignalType) => {
                        quote!(let value=value.map_err(|error|match error {whatsapp_rust::features::SignalError::Unsupported(detail)=>crate::errors::invalid_arg("msgType",detail),other=>other.into(),})?;)
                    }
                };
                let output = match codec {
                    ValueCodec::GroupJid => quote!(value.group_jid().to_string()),
                    ValueCodec::AllSucceeded => {
                        quote!(value.iter().all(|response| response.is_ok()))
                    }
                    ValueCodec::CheckedU32(reason) => {
                        return quote!(let value=#call;#unwrap u32::try_from(value).map_err(|_|crate::errors::internal(#reason)));
                    }
                    ValueCodec::Identity => quote!(value),
                    ValueCodec::StanzaId => quote!(value.into_string()),
                    ValueCodec::MessageId => quote!(value.message_id.into_string()),
                    ValueCodec::NumberF64 => quote!(value as f64),
                    ValueCodec::OwnedByteArray => quote!(crate::wasm_utils::byte_array(&value)),
                    ValueCodec::Bytes => quote!(js_sys::Uint8Array::from(value.as_slice())),
                    ValueCodec::True => quote!({
                        let _ = value;
                        true
                    }),
                    ValueCodec::OptionalAddress { field, server } => {
                        let format = format!("{{}}@{}", server.value());
                        quote!(value.map(|entry| format!(#format,entry.#field)))
                    }
                };
                quote!(let value=#call;#unwrap Ok(#output))
            }
            Self::Converted {
                converter,
                field,
                list,
            } => {
                assert!(
                    !(*list && field.is_some()),
                    "list conversion does not select a scalar field"
                );
                let function: Path = match converter {
                    Converter::GroupMetadata => {
                        parse_quote!(crate::result_types::project_group_metadata)
                    }
                    Converter::NewsletterMetadata => {
                        parse_quote!(crate::result_types::project_newsletter_metadata)
                    }
                    Converter::BotList => parse_quote!(crate::result_types::project_bot_list),
                };
                if *list {
                    quote!(let value=#call;to_ts_vec(value.iter().map(#function).collect()))
                } else {
                    let value = field
                        .as_ref()
                        .map_or_else(|| quote!(&value), |field| quote!(&value.#field));
                    quote!(let value=#call;to_ts(#function(#value)))
                }
            }
            Self::CommunityLink { groups } => {
                let function: Path = match groups.to_string().as_str() {
                    "linked_jids" => parse_quote!(crate::result_types::project_community_link),
                    "unlinked_jids" => parse_quote!(crate::result_types::project_community_unlink),
                    _ => panic!("unregistered community link result source"),
                };
                quote!(let value=#call;to_ts(#function(&value)))
            }
            Self::GroupRecord { source_key } => {
                let key = if *source_key {
                    quote!(&group.id.to_string().into())
                } else {
                    quote!(&JsValue::from_str(&entry.id))
                };
                quote!(let groups=#call;let result=js_sys::Object::new();for group in &groups {let entry=crate::result_types::project_group_overview(group);let value=serde_wasm_bindgen::to_value(&entry)?;js_sys::Reflect::set(&result,#key,&value)?;}Ok(result.into()))
            }
        }
    }
}
#[derive(Clone, Copy)]
pub enum ObjectShape {
    SignalMessage,
    SignalGroup,
    ParticipantNodes,
    UsyncDevices,
}
struct ObjectField {
    name: &'static str,
    source: Ident,
    codec: ObjectFieldCodec,
    condition: Option<Expr>,
}
enum ObjectFieldCodec {
    WireString,
    Bytes,
    OptionalBytesUndefined,
    Nodes,
    Into,
    JidUser,
    JidDevice,
    JidString,
}
impl ObjectShape {
    fn fields(self) -> Vec<ObjectField> {
        let field = |name, source, codec| ObjectField {
            name,
            source,
            codec,
            condition: None,
        };
        match self {
            Self::SignalMessage => vec![
                field("type", parse_quote!(msg_type), ObjectFieldCodec::WireString),
                field(
                    "ciphertext",
                    parse_quote!(ciphertext),
                    ObjectFieldCodec::Bytes,
                ),
            ],
            Self::SignalGroup => vec![
                field(
                    "senderKeyDistributionMessage",
                    parse_quote!(skdm),
                    ObjectFieldCodec::OptionalBytesUndefined,
                ),
                field(
                    "ciphertext",
                    parse_quote!(ciphertext),
                    ObjectFieldCodec::Bytes,
                ),
            ],
            Self::ParticipantNodes => vec![
                field("nodes", parse_quote!(nodes), ObjectFieldCodec::Nodes),
                field(
                    "shouldIncludeDeviceIdentity",
                    parse_quote!(should_include_device_identity),
                    ObjectFieldCodec::Into,
                ),
            ],
            Self::UsyncDevices => vec![
                field("user", parse_quote!(jid), ObjectFieldCodec::JidUser),
                ObjectField {
                    condition: Some(parse_quote!(jid.device != 0)),
                    ..field("device", parse_quote!(jid), ObjectFieldCodec::JidDevice)
                },
                field("jid", parse_quote!(jid), ObjectFieldCodec::JidString),
            ],
        }
    }
    fn emit_fields(self) -> TokenStream {
        let mut names = std::collections::BTreeSet::new();
        let fields=self.fields().into_iter().map(|field| {
            assert!(names.insert(field.name),"duplicate JS object field");
            let name=field.name;let source=field.source;
            let value=match field.codec {
                ObjectFieldCodec::WireString=>quote!(#source.as_wire_str().into()),
                ObjectFieldCodec::Bytes=>quote!(js_sys::Uint8Array::from(#source.as_slice()).into()),
                ObjectFieldCodec::OptionalBytesUndefined=>quote!(match &#source {Some(bytes)=>js_sys::Uint8Array::from(bytes.as_slice()).into(),None=>JsValue::UNDEFINED,}),
                ObjectFieldCodec::Nodes=>quote!(nodes_to_js_array(&#source).map_err(|error|crate::errors::internal(format!("node serialization failed: {error:?}")))?),
                ObjectFieldCodec::Into=>quote!(#source.into()),
                ObjectFieldCodec::JidUser=>quote!(#source.user.as_str().into()),
                ObjectFieldCodec::JidDevice=>quote!((#source.device as f64).into()),
                ObjectFieldCodec::JidString=>quote!(#source.to_string().into()),
            };
            let write=quote!({let value:JsValue=#value;js_sys::Reflect::set(&obj,&#name.into(),&value).map_err(|error|crate::errors::internal(format!("{error:?}")))?;});
            match field.condition {Some(condition)=>quote!(if #condition #write),None=>write}
        });
        quote!(#(#fields)*)
    }
    fn emit(self, call: TokenStream) -> TokenStream {
        let fields = self.emit_fields();
        if matches!(self, Self::UsyncDevices) {
            return quote!(let devices=#call;let arr=js_sys::Array::new_with_length(devices.len() as u32);for (index,jid) in devices.iter().enumerate(){let obj=js_sys::Object::new();#fields arr.set(index as u32,obj.into());}Ok(arr.into()));
        }
        let binding: syn::Pat = match self {
            Self::SignalMessage => parse_quote!((msg_type, ciphertext)),
            Self::SignalGroup => parse_quote!((skdm, ciphertext)),
            Self::ParticipantNodes => parse_quote!((nodes, should_include_device_identity)),
            Self::UsyncDevices => unreachable!(),
        };
        quote!(let #binding=#call?;let obj=js_sys::Object::new();#fields Ok(obj.into()))
    }
}
pub fn wasm_fixture() -> TokenStream {
    let group = ObjectShape::SignalGroup.emit(quote!(Ok::<_, crate::errors::BridgeError>((
        skdm, ciphertext
    ))));
    let devices = ObjectShape::UsyncDevices.emit(quote!(devices));
    quote!(
        #[cfg(test)] mod generated_object_codec_tests {
            use super::*;
            use wasm_bindgen_test::wasm_bindgen_test as test;
            fn group(skdm:Option<Vec<u8>>,ciphertext:Vec<u8>)->Result<JsValue,crate::errors::BridgeError>{#group}
            fn devices(devices:Vec<Jid>)->Result<JsValue,crate::errors::BridgeError>{#devices}
            #[test] fn optional_byte_field_stays_present_with_undefined_or_typed_bytes(){
                let value=group(None,vec![0,128,255]).unwrap();
                assert!(js_sys::Reflect::has(&value,&"senderKeyDistributionMessage".into()).unwrap());
                assert!(js_sys::Reflect::get(&value,&"senderKeyDistributionMessage".into()).unwrap().is_undefined());
                assert_eq!(js_sys::Uint8Array::new(&js_sys::Reflect::get(&value,&"ciphertext".into()).unwrap()).to_vec(),[0,128,255]);
                let value=group(Some(vec![]),vec![]).unwrap();let bytes=js_sys::Reflect::get(&value,&"senderKeyDistributionMessage".into()).unwrap();
                assert!(bytes.is_instance_of::<js_sys::Uint8Array>());assert_eq!(js_sys::Uint8Array::new(&bytes).length(),0);
            }
            #[test] fn device_zero_is_omitted_and_nonzero_stays_numeric(){
                let first=Jid::pn("123");let mut second=Jid::pn("456");second.device=7;
                let value=js_sys::Array::from(&devices(vec![first,second]).unwrap());
                assert!(!js_sys::Reflect::has(&value.get(0),&"device".into()).unwrap());
                assert_eq!(js_sys::Reflect::get(&value.get(1),&"device".into()).unwrap().as_f64(),Some(7.0));
                assert_eq!(js_sys::Reflect::get(&value.get(1),&"user".into()).unwrap().as_string().as_deref(),Some("456"));
            }
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_enum_and_overlay_fields_are_rejected() {
        let codec = InputCodec::Enum {
            binding: parse_quote!(output),
            value: parse_quote!(input),
            source: parse_quote!(Input),
            target: parse_quote!(Output),
            variants: vec![
                (parse_quote!(A), parse_quote!(A)),
                (parse_quote!(A), parse_quote!(B)),
            ],
        };
        assert!(std::panic::catch_unwind(|| codec.emit()).is_err());
        let codec = InputCodec::DefaultOverlay {
            binding: parse_quote!(options),
            input: parse_quote!(input),
            ty: parse_quote!(Options),
            optional: vec![(parse_quote!(a), parse_quote!(a))],
            direct: vec![(parse_quote!(b), parse_quote!(a))],
        };
        assert!(std::panic::catch_unwind(|| codec.emit()).is_err());
    }
}
