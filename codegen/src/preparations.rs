//! Ordered boundary preparation. Core constructors own domain validation;
//! the descriptors below own argument names, error precedence and capture.
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, Ident, Pat, Path, Type, parse_quote};

#[derive(Clone, Copy)]
pub enum Preparation {
    BusinessProfile,
    GroupCreate,
    Participants,
    CommunityCreate,
    DirtyBits,
    PairingCode,
    Retransmission,
    EditOptions,
    Revoke,
    StatusMessage,
    NewsletterReference,
    StanzaRejection,
    RetryOptions,
    Usync,
    DecryptType,
    PreKeyBundle,
    LidPnMappings,
    MediaReupload,
    DownloadCapture,
}

/// Only expressions that cannot suspend or enter the client belong here.
/// Control flow is emitted by the typed steps, never supplied as Rust blocks.
enum Step {
    Let {
        binding: Pat,
        value: Expr,
        mutable: bool,
    },
    Construct {
        binding: Pat,
        constructor: Path,
        args: Vec<Expr>,
        error: Error,
    },
    Struct {
        binding: Pat,
        ty: Path,
        fields: Vec<(Ident, Expr)>,
        default: bool,
    },
    Set {
        target: Expr,
        value: Expr,
    },
    Setter {
        binding: Ident,
        method: Ident,
        args: Vec<Expr>,
    },
    Optional {
        binding: Pat,
        value: Expr,
        item: Pat,
        steps: Vec<Step>,
        result: Expr,
    },
    List {
        binding: Pat,
        value: Expr,
        item: Pat,
        steps: Vec<Step>,
        result: Expr,
    },
    BufferedList {
        binding: Ident,
        capacity: Expr,
        value: Expr,
        item: Pat,
        steps: Vec<Step>,
        result: Expr,
    },
    Select {
        binding: Pat,
        value: Expr,
        arms: Vec<Arm>,
    },
    Guard {
        invalid: Expr,
        field: Expr,
        reason: Expr,
    },
    Capture {
        binding: Ident,
        value: Expr,
        kind: Capture,
    },
}
struct Arm {
    pattern: Pat,
    steps: Vec<Step>,
    result: Outcome,
}
enum Outcome {
    Value(Expr),
    Fail { field: Expr, reason: Expr },
}
enum Capture {
    String,
    Bytes,
    Into(Box<Type>),
    U64,
}
enum Error {
    Infallible,
    Bridge,
    Field(&'static str),
    Newsletter,
}
fn ident(s: &str) -> Ident {
    syn::parse_str(s).unwrap()
}
fn pat(s: &str) -> Pat {
    syn::parse::Parser::parse_str(Pat::parse_single, s).unwrap()
}
fn expr(s: &str) -> Expr {
    use syn::visit::Visit;
    struct LeafValidator;
    impl<'ast> Visit<'ast> for LeafValidator {
        fn visit_expr(&mut self, node: &'ast Expr) {
            assert!(
                !matches!(
                    node,
                    Expr::Block(_)
                        | Expr::Return(_)
                        | Expr::If(_)
                        | Expr::Match(_)
                        | Expr::Loop(_)
                        | Expr::ForLoop(_)
                        | Expr::While(_)
                        | Expr::Async(_)
                        | Expr::Await(_)
                        | Expr::Break(_)
                        | Expr::Continue(_)
                ),
                "preparation expressions must not contain control flow"
            );
            syn::visit::visit_expr(self, node);
        }
        fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
            assert!(
                !node.path.is_ident("self") && !node.path.is_ident("client"),
                "preparation cannot enter the client"
            );
            syn::visit::visit_expr_path(self, node);
        }
    }
    let value = syn::parse_str(s).unwrap();
    LeafValidator.visit_expr(&value);
    value
}
fn path(s: &str) -> Path {
    syn::parse_str(s).unwrap()
}
fn bind(name: &str, value: &str) -> Step {
    Step::Let {
        binding: pat(name),
        value: expr(value),
        mutable: false,
    }
}
fn construct(name: &str, function: &str, args: &[&str], error: Error) -> Step {
    Step::Construct {
        binding: pat(name),
        constructor: path(function),
        args: args.iter().map(|s| expr(s)).collect(),
        error,
    }
}
fn structure(name: &str, ty: &str, fields: &[(&str, &str)], default: bool) -> Step {
    Step::Struct {
        binding: pat(name),
        ty: path(ty),
        fields: fields.iter().map(|(k, v)| (ident(k), expr(v))).collect(),
        default,
    }
}
fn guard(condition: &str, field: &str, reason: &str) -> Step {
    Step::Guard {
        invalid: expr(condition),
        field: expr(field),
        reason: expr(reason),
    }
}
fn setter(binding: &str, method: &str, args: &[&str]) -> Step {
    Step::Setter {
        binding: ident(binding),
        method: ident(method),
        args: args.iter().map(|s| expr(s)).collect(),
    }
}
fn arm(pattern: &str, steps: Vec<Step>, result: &str) -> Arm {
    Arm {
        pattern: pat(pattern),
        steps,
        result: Outcome::Value(expr(result)),
    }
}
fn failure_arm(pattern: &str, steps: Vec<Step>, field: &str, reason: &str) -> Arm {
    Arm {
        pattern: pat(pattern),
        steps,
        result: Outcome::Fail {
            field: expr(field),
            reason: expr(reason),
        },
    }
}
fn optional(binding: &str, value: &str, item: &str, steps: Vec<Step>, result: &str) -> Step {
    Step::Optional {
        binding: pat(binding),
        value: expr(value),
        item: pat(item),
        steps,
        result: expr(result),
    }
}
fn steps_emit(steps: &[Step]) -> TokenStream {
    let tokens = steps.iter().map(Step::emit);
    quote!(#(#tokens)*)
}
impl Error {
    fn apply(&self, value: TokenStream) -> TokenStream {
        match self {
            Self::Infallible => value,
            Self::Bridge => quote!(#value?),
            Self::Field(field) => {
                quote!(#value.map_err(|error|crate::errors::invalid_arg(#field,error.to_string()))?)
            }
            Self::Newsletter => {
                quote!(#value.map_err(|error|crate::errors::BridgeError::from(whatsapp_rust::features::NewsletterError::MessageRef(error)))?)
            }
        }
    }
}
impl Step {
    fn emit(&self) -> TokenStream {
        match self {
            Self::Let {
                binding,
                value,
                mutable,
            } => {
                let mutable = mutable.then(|| quote!(mut));
                quote!(let #mutable #binding=#value;)
            }
            Self::Construct {
                binding,
                constructor,
                args,
                error,
            } => {
                let value = error.apply(quote!(#constructor(#(#args),*)));
                quote!(let #binding=#value;)
            }
            Self::Struct {
                binding,
                ty,
                fields,
                default,
            } => {
                let fields = fields.iter().map(|(name, value)| {
                    if matches!(value, Expr::Path(path) if path.path.is_ident(name)) {
                        quote!(#name,)
                    } else {
                        quote!(#name: #value,)
                    }
                });
                let default = default.then(|| quote!(..Default::default()));
                quote!(let #binding=#ty {#(#fields)* #default};)
            }
            Self::Set { target, value } => quote!(#target=#value;),
            Self::Setter {
                binding,
                method,
                args,
            } => quote!(#binding=#binding.#method(#(#args),*);),
            Self::Optional {
                binding,
                value,
                item,
                steps,
                result,
            } => {
                let steps = steps_emit(steps);
                quote!(let #binding=#value.map(|#item|->Result<_,crate::errors::BridgeError>{#steps Ok(#result)}).transpose()?;)
            }
            Self::List {
                binding,
                value,
                item,
                steps,
                result,
            } => {
                let steps = steps_emit(steps);
                quote!(let #binding=#value.map(|#item|->Result<_,crate::errors::BridgeError>{#steps Ok(#result)}).collect::<Result<Vec<_>,_>>()?;)
            }
            Self::BufferedList {
                binding,
                capacity,
                value,
                item,
                steps,
                result,
            } => {
                let steps = steps_emit(steps);
                quote!(let mut #binding=Vec::with_capacity(#capacity);for #item in #value {#steps #binding.push(#result);})
            }
            Self::Select {
                binding,
                value,
                arms,
            } => {
                let arms = arms.iter().map(|a| {
                    let pattern = &a.pattern;
                    let steps = steps_emit(&a.steps);
                    let result = match &a.result {
                        Outcome::Value(value) => quote!(#value),
                        Outcome::Fail { field, reason } => {
                            quote!(return Err(crate::errors::invalid_arg(#field,#reason)))
                        }
                    };
                    quote!(#pattern=>{#steps #result},)
                });
                quote!(let #binding=match #value {#(#arms)*};)
            }
            Self::Guard {
                invalid,
                field,
                reason,
            } => quote!(if #invalid {return Err(crate::errors::invalid_arg(#field,#reason));}),
            Self::Capture {
                binding,
                value,
                kind,
            } => match kind {
                Capture::String => quote!(let #binding=#value.to_string();),
                Capture::Bytes => quote!(let #binding=#value.to_vec();),
                Capture::Into(ty) => quote!(let #binding:#ty=#value.into();),
                Capture::U64 => quote!(let #binding=#value as u64;),
            },
        }
    }
}
fn participants_steps() -> Vec<Step> {
    vec![
        guard(
            "matches!(action,crate::result_types::GroupParticipantAction::Modify)",
            "\"action\"",
            "\"modify represents a received participant identity change and cannot be sent\"",
        ),
        construct("group_jid", "parse_jid", &["jid"], Error::Bridge),
        Step::List {
            binding: pat("participant_jids"),
            value: expr("participants.iter()"),
            item: pat("participant"),
            steps: vec![construct(
                "jid",
                "parse_jid",
                &["participant"],
                Error::Bridge,
            )],
            result: expr("jid"),
        },
    ]
}
fn edit_steps() -> Vec<Step> {
    vec![Step::Select {
        binding: pat("options"),
        value: expr("stanza_id"),
        arms: vec![
            arm("None", vec![], "whatsapp_rust::EditOptions::default()"),
            arm(
                "Some(id)",
                vec![
                    guard(
                        "id.trim().is_empty()",
                        "\"stanzaId\"",
                        "\"must not be empty or whitespace; omit it to have a fresh id minted\"",
                    ),
                    construct(
                        "id",
                        "whatsapp_rust::StanzaId::new",
                        &["id"],
                        Error::Field("stanzaId"),
                    ),
                ],
                "whatsapp_rust::EditOptions::default().with_stanza_id(id)",
            ),
        ],
    }]
}
fn status_steps() -> Vec<Step> {
    vec![
        bind(
            "msg",
            "waproto::codec::message_decode(bytes).map_err(|error|crate::errors::internal(format!(\"invalid message bytes: {error}\")))?",
        ),
        Step::List {
            binding: pat("recipients"),
            value: expr("recipients.iter()"),
            item: pat("recipient"),
            steps: vec![construct("jid", "parse_jid", &["recipient"], Error::Bridge)],
            result: expr("jid"),
        },
    ]
}
fn newsletter_steps() -> Vec<Step> {
    vec![
        construct(
            "target",
            "whatsapp_rust::NewsletterMessageRef::new",
            &["jid", "None", "None"],
            Error::Newsletter,
        ),
        construct(
            "id",
            "whatsapp_rust::MessageId::new",
            &["message_id"],
            Error::Newsletter,
        ),
        construct(
            "reference",
            "whatsapp_rust::NewsletterMessageRef::new",
            &["target.chat()", "Some(id)", "None"],
            Error::Newsletter,
        ),
    ]
}
impl Preparation {
    pub fn emit(self) -> TokenStream {
        steps_emit(&self.steps())
    }
    fn steps(self) -> Vec<Step> {
        match self {
            Self::Participants => vec![bind(
                "(group_jid,participant_jids)",
                "participants_update_input(jid,&participants,action)?",
            )],
            Self::EditOptions => vec![bind("options", "edit_options(stanza_id)?")],
            Self::StatusMessage => vec![bind(
                "(msg,recipients)",
                "status_message_input(bytes,&recipients)?",
            )],
            Self::NewsletterReference => vec![bind(
                "reference",
                "newsletter_content_reference(&target,message_id)?",
            )],
            Self::GroupCreate => vec![
                Step::List {
                    binding: pat("participant_options"),
                    value: expr("participants.iter()"),
                    item: pat("participant"),
                    steps: vec![
                        construct("jid", "parse_jid", &["participant"], Error::Bridge),
                        construct(
                            "options",
                            "whatsapp_rust::features::GroupParticipantOptions::new",
                            &["jid"],
                            Error::Infallible,
                        ),
                    ],
                    result: expr("options"),
                },
                bind(
                    "options",
                    "whatsapp_rust::features::GroupCreateOptions::new(subject).with_participants(participant_options)",
                ),
            ],
            Self::CommunityCreate => {
                let mut steps = vec![
                    construct(
                        "options",
                        "whatsapp_rust::features::CreateCommunityOptions::new",
                        &["name"],
                        Error::Field("name"),
                    ),
                    optional(
                        "description",
                        "description",
                        "value",
                        vec![construct(
                            "description",
                            "whatsapp_rust::features::GroupDescription::new",
                            &["value"],
                            Error::Field("description"),
                        )],
                        "description",
                    ),
                ];
                // A separate mutable binding avoids requiring mutable constructors.
                steps.push(Step::Let {
                    binding: pat("options"),
                    value: expr("options"),
                    mutable: true,
                });
                for (target, value) in [
                    ("options.description", "description"),
                    ("options.closed", "closed"),
                    (
                        "options.allow_non_admin_sub_group_creation",
                        "allow_non_admin_sub_group_creation",
                    ),
                    ("options.create_general_chat", "create_general_chat"),
                ] {
                    steps.push(Step::Set {
                        target: expr(target),
                        value: expr(value),
                    });
                }
                steps
            }
            Self::PairingCode => vec![structure(
                "options",
                "whatsapp_rust::pair_code::PairCodeOptions",
                &[
                    ("phone_number", "phone_number.to_string()"),
                    ("custom_code", "custom_code"),
                ],
                true,
            )],
            Self::DirtyBits => vec![
                guard(
                    "dirty_type.is_empty()",
                    "\"dirtyType\"",
                    "\"must not be empty\"",
                ),
                construct(
                    "kind",
                    "wacore::iq::dirty::DirtyType::from",
                    &["dirty_type"],
                    Error::Infallible,
                ),
                Step::Select {
                    binding: pat("bit"),
                    value: expr("timestamp"),
                    arms: vec![
                        arm(
                            "Some(value)",
                            vec![guard(
                                "!is_representable_millis(value)",
                                "\"timestamp\"",
                                "MILLIS_RANGE",
                            )],
                            "wacore::iq::dirty::DirtyBit::with_timestamp(kind,value as u64)",
                        ),
                        arm("None", vec![], "wacore::iq::dirty::DirtyBit::new(kind)"),
                    ],
                },
            ],
            Self::Retransmission => vec![
                bind(
                    "retry_count",
                    "u8::try_from(input.retry_count).map_err(|_|crate::errors::internal(\"retry count exceeds the u8 range\"))?",
                ),
                Step::Let {
                    binding: pat("request"),
                    value: expr(
                        "whatsapp_rust::MessageRetransmission::new(chat,requester,msg,input.message_id,retry_count)",
                    ),
                    mutable: true,
                },
                setter(
                    "request",
                    "with_group_metadata_freshness",
                    &["freshness(input.refresh_group_metadata)"],
                ),
                optional(
                    "recipient",
                    "input.recipient_jid",
                    "value",
                    vec![construct("jid", "parse_jid", &["&value"], Error::Bridge)],
                    "jid",
                ),
                Step::Select {
                    binding: pat("request"),
                    value: expr("recipient"),
                    arms: vec![
                        arm(
                            "Some(recipient)",
                            vec![],
                            "request.with_recipient(recipient)",
                        ),
                        arm("None", vec![], "request"),
                    ],
                },
            ],
            Self::Revoke => vec![Step::Select {
                binding: pat("revoke_type"),
                value: expr("participant"),
                arms: vec![
                    arm(
                        "Some(value)",
                        vec![construct("sender", "parse_jid", &["&value"], Error::Bridge)],
                        "whatsapp_rust::RevokeType::Admin {original_sender:sender}",
                    ),
                    arm("None", vec![], "whatsapp_rust::RevokeType::Sender"),
                ],
            }],
            Self::StanzaRejection => vec![
                construct(
                    "reason",
                    "whatsapp_rust::NackReason::from",
                    &["error_code"],
                    Error::Infallible,
                ),
                Step::Select {
                    binding: pat("rejection"),
                    value: expr("reason"),
                    arms: vec![
                        arm(
                            "whatsapp_rust::NackReason::InvalidProtobuf",
                            vec![],
                            "whatsapp_rust::StanzaRejection::invalid_protobuf(failure_reason)",
                        ),
                        arm(
                            "reason",
                            vec![],
                            "whatsapp_rust::StanzaRejection::new(reason)",
                        ),
                    ],
                },
            ],
            Self::RetryOptions => vec![bind(
                "options",
                "whatsapp_rust::RetryRequestOptions::new().with_force_include_keys(force_include_keys.unwrap_or(false))",
            )],
            Self::Usync => vec![bind(
                "query",
                "crate::proto::from_js_value::<whatsapp_rust::usync::UsyncQuery>(query.into(),\"query\")?",
            )],
            Self::DecryptType => vec![bind(
                "enc_type",
                "wacore::message_processing::EncType::from_wire(msg_type).ok_or_else(||crate::errors::invalid_arg(\"msgType\",format!(\"unknown: {msg_type}\")))?",
            )],
            Self::MediaReupload => vec![
                optional(
                    "participant_jid",
                    "participant.as_deref()",
                    "value",
                    vec![construct("jid", "parse_jid", &["value"], Error::Bridge)],
                    "jid",
                ),
                structure(
                    "req",
                    "whatsapp_rust::MediaReuploadRequest",
                    &[
                        ("msg_id", "msg_id"),
                        ("chat_jid", "&chat"),
                        ("media_key", "media_key"),
                        ("is_from_me", "is_from_me"),
                        ("participant", "participant_jid.as_ref()"),
                    ],
                    false,
                ),
            ],
            Self::DownloadCapture => vec![
                Step::Capture {
                    binding: ident("mt"),
                    value: expr("media_type"),
                    kind: Capture::Into(Box::new(parse_quote!(wacore::download::MediaType))),
                },
                Step::Capture {
                    binding: ident("direct_path"),
                    value: expr("direct_path"),
                    kind: Capture::String,
                },
                Step::Capture {
                    binding: ident("media_key"),
                    value: expr("media_key"),
                    kind: Capture::Bytes,
                },
                Step::Capture {
                    binding: ident("file_sha256"),
                    value: expr("file_sha256"),
                    kind: Capture::Bytes,
                },
                Step::Capture {
                    binding: ident("file_enc_sha256"),
                    value: expr("file_enc_sha256"),
                    kind: Capture::Bytes,
                },
                Step::Capture {
                    binding: ident("file_length"),
                    value: expr("file_length"),
                    kind: Capture::U64,
                },
            ],
            Self::PreKeyBundle => prekey_steps(),
            Self::LidPnMappings => mapping_steps(),
            Self::BusinessProfile => business_steps(),
        }
    }
}
fn prekey_steps() -> Vec<Step> {
    vec![
        construct(
            "public_identity",
            "wacore::libsignal::protocol::PublicKey::from_djb_public_key_bytes",
            &["&input.identity_key"],
            Error::Field("identityKey"),
        ),
        construct(
            "identity_key",
            "wacore::libsignal::protocol::IdentityKey::new",
            &["public_identity"],
            Error::Infallible,
        ),
        construct(
            "signed_pre_key",
            "wacore::libsignal::protocol::PublicKey::from_djb_public_key_bytes",
            &["&input.signed_pre_key.public_key"],
            Error::Field("signedPreKey.publicKey"),
        ),
        optional(
            "pre_key",
            "input.pre_key",
            "pre_key",
            vec![construct(
                "public_key",
                "wacore::libsignal::protocol::PublicKey::from_djb_public_key_bytes",
                &["&pre_key.public_key"],
                Error::Field("preKey.publicKey"),
            )],
            "(pre_key.key_id.into(),public_key)",
        ),
        construct(
            "bundle",
            "wacore::libsignal::protocol::PreKeyBundle::new",
            &[
                "input.registration_id",
                "u32::from(parsed.device).into()",
                "pre_key",
                "input.signed_pre_key.key_id.into()",
                "signed_pre_key",
                "input.signed_pre_key.signature",
                "identity_key",
            ],
            Error::Field("bundle"),
        ),
    ]
}
fn mapping_steps() -> Vec<Step> {
    vec![
        bind(
            "mappings",
            "from_js_input::<Vec<crate::result_types::LidPnMappingInput>>(\"mappings\",mappings)?",
        ),
        Step::BufferedList {
            binding: ident("pairs"),
            capacity: expr("mappings.len()"),
            value: expr("mappings.into_iter().enumerate()"),
            item: pat("(index,mapping)"),
            steps: vec![
                construct("lid", "parse_jid", &["&mapping.lid"], Error::Bridge),
                guard(
                    "!lid.server.is_lid_family()",
                    "format!(\"mappings[{index}].lid\")",
                    "\"must use a linked-identifier namespace\"",
                ),
                construct("pn", "parse_jid", &["&mapping.pn"], Error::Bridge),
                guard(
                    "!pn.server.is_pn_family()",
                    "format!(\"mappings[{index}].pn\")",
                    "\"must use a phone-number namespace\"",
                ),
            ],
            result: expr("(lid.user.to_string(),pn.user.to_string())"),
        },
    ]
}
fn business_steps() -> Vec<Step> {
    let config = Step::List {
        binding: pat("config"),
        value: expr("hours.config.into_iter()"),
        item: pat("entry"),
        steps: vec![
            bind("day", "entry.day_of_week.as_str().into()"),
            bind("mode", "entry.mode.as_str().into()"),
            Step::Select {
                binding: pat("config"),
                value: expr("(entry.open_time,entry.close_time)"),
                arms: vec![
                    arm(
                        "(Some(open),Some(close))",
                        vec![],
                        "wacore::iq::business::BusinessHoursConfig::with_hours(day,mode,open,close)",
                    ),
                    arm(
                        "(None,None)",
                        vec![],
                        "wacore::iq::business::BusinessHoursConfig::new(day,mode)",
                    ),
                    failure_arm(
                        "(open,_)",
                        vec![Step::Select {
                            binding: pat("missing"),
                            value: expr("open"),
                            arms: vec![
                                arm("Some(_)", vec![], "\"closeTime\""),
                                arm("None", vec![], "\"openTime\""),
                            ],
                        }],
                        "\"businessHours.config\"",
                        "format!(\"{} is missing {missing}: an opening range needs both ends\",entry.day_of_week)",
                    ),
                ],
            },
        ],
        result: expr("config"),
    };
    vec![
        optional(
            "business_hours",
            "update.business_hours",
            "hours",
            vec![
                config,
                structure(
                    "hours",
                    "wacore::iq::business::BusinessHoursUpdate",
                    &[
                        ("timezone", "hours.timezone"),
                        ("note", "hours.note"),
                        ("config", "config"),
                    ],
                    false,
                ),
            ],
            "hours",
        ),
        structure(
            "delta",
            "wacore::iq::business::BusinessProfileUpdate",
            &[
                ("address", "update.address"),
                ("latitude", "update.latitude"),
                ("longitude", "update.longitude"),
                ("description", "update.description"),
                ("email", "update.email"),
                ("websites", "update.websites"),
                ("categories", "update.categories"),
                ("business_hours", "business_hours"),
            ],
            false,
        ),
    ]
}

/// These names are retained so existing focused runtime tests still exercise
/// the emitted validation order. Include the output in wasm_client's scope.
pub fn emit_helpers() -> TokenStream {
    let participants = steps_emit(&participants_steps());
    let edit = steps_emit(&edit_steps());
    let status = steps_emit(&status_steps());
    let newsletter = steps_emit(&newsletter_steps());
    quote!(
        #[cfg(feature="client-groups")]
        fn participants_update_input(jid:&str,participants:&[String],action:crate::result_types::GroupParticipantAction)->Result<(Jid,Vec<Jid>),crate::errors::BridgeError>{#participants Ok((group_jid,participant_jids))}

        fn edit_options(stanza_id:Option<String>)->Result<whatsapp_rust::EditOptions,crate::errors::BridgeError>{#edit Ok(options)}

        fn status_message_input(bytes:&[u8],recipients:&[String])->Result<(waproto::whatsapp::Message,Vec<Jid>),crate::errors::BridgeError>{#status Ok((msg,recipients))}
        #[cfg(feature="client-newsletter")]
        fn newsletter_content_reference<'a>(jid:&'a Jid,message_id:&str)->Result<whatsapp_rust::NewsletterMessageRef<'a>,crate::errors::BridgeError>{#newsletter Ok(reference)}
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_preparation_and_helpers_parse_as_rust() {
        for preparation in [
            Preparation::BusinessProfile,
            Preparation::GroupCreate,
            Preparation::Participants,
            Preparation::CommunityCreate,
            Preparation::DirtyBits,
            Preparation::PairingCode,
            Preparation::Retransmission,
            Preparation::EditOptions,
            Preparation::Revoke,
            Preparation::StatusMessage,
            Preparation::NewsletterReference,
            Preparation::StanzaRejection,
            Preparation::RetryOptions,
            Preparation::Usync,
            Preparation::DecryptType,
            Preparation::PreKeyBundle,
            Preparation::LidPnMappings,
            Preparation::MediaReupload,
            Preparation::DownloadCapture,
        ] {
            let tokens = preparation.emit();
            syn::parse2::<syn::Block>(quote!({#tokens})).unwrap();
        }
        syn::parse2::<syn::File>(emit_helpers()).unwrap();
    }
    fn ordered(tokens: TokenStream, markers: &[&str]) {
        let rendered = tokens.to_string();
        let mut cursor = 0;
        for marker in markers {
            let offset = rendered[cursor..]
                .find(marker)
                .unwrap_or_else(|| panic!("missing or reordered {marker}: {rendered}"));
            cursor += offset + marker.len();
        }
    }

    #[test]
    fn validation_precedence_is_preserved_before_construction() {
        ordered(
            steps_emit(&participants_steps()),
            &["matches !", "parse_jid (jid)", "participants . iter"],
        );
        ordered(
            steps_emit(&newsletter_steps()),
            &[
                "NewsletterMessageRef :: new (jid , None , None)",
                "MessageId :: new",
                "NewsletterMessageRef :: new (target . chat",
            ],
        );
        ordered(
            steps_emit(&status_steps()),
            &["message_decode", "recipients . iter"],
        );
        ordered(
            steps_emit(&edit_steps()),
            &["trim", "stanzaId", "StanzaId :: new", "with_stanza_id"],
        );
        ordered(
            Preparation::PreKeyBundle.emit(),
            &[
                "identity_key",
                "signed_pre_key . public_key",
                "input . pre_key",
                "PreKeyBundle :: new",
            ],
        );
    }

    #[test]
    fn mapping_allocation_and_first_error_policy_are_explicit() {
        let steps = mapping_steps();
        let Step::BufferedList {
            capacity,
            value,
            steps: per_mapping,
            ..
        } = &steps[1]
        else {
            panic!("mapping must retain its reserved loop");
        };
        assert_eq!(
            quote!(#capacity).to_string(),
            quote!(mappings.len()).to_string()
        );
        assert_eq!(
            quote!(#value).to_string(),
            quote!(mappings.into_iter().enumerate()).to_string()
        );
        assert!(matches!(
            &per_mapping[..],
            [
                Step::Construct { .. },
                Step::Guard { .. },
                Step::Construct { .. },
                Step::Guard { .. }
            ]
        ));
        ordered(
            Preparation::LidPnMappings.emit(),
            &[
                "with_capacity",
                "for (index , mapping)",
                "parse_jid (& mapping . lid)",
                "is_lid_family",
                "parse_jid (& mapping . pn)",
                "is_pn_family",
                "pairs . push",
            ],
        );
    }

    #[test]
    fn capture_owns_stream_inputs_without_reordering() {
        let captures = Preparation::DownloadCapture.steps();
        assert!(matches!(
            &captures[..],
            [
                Step::Capture {
                    kind: Capture::Into(_),
                    ..
                },
                Step::Capture {
                    kind: Capture::String,
                    ..
                },
                Step::Capture {
                    kind: Capture::Bytes,
                    ..
                },
                Step::Capture {
                    kind: Capture::Bytes,
                    ..
                },
                Step::Capture {
                    kind: Capture::Bytes,
                    ..
                },
                Step::Capture {
                    kind: Capture::U64,
                    ..
                }
            ]
        ));
        ordered(
            Preparation::DownloadCapture.emit(),
            &[
                "media_type . into",
                "direct_path . to_string",
                "media_key . to_vec",
                "file_sha256 . to_vec",
                "file_enc_sha256 . to_vec",
                "file_length as u64",
            ],
        );
    }

    #[test]
    fn business_incomplete_range_is_a_typed_failure() {
        let steps = business_steps();
        let Step::Optional { steps: hours, .. } = &steps[0] else {
            panic!("hours stay optional");
        };
        let Step::List { steps: config, .. } = &hours[0] else {
            panic!("config stays ordered");
        };
        let Step::Select { arms, .. } = &config[2] else {
            panic!("range selection must be explicit");
        };
        assert!(matches!(arms[2].result, Outcome::Fail { .. }));
        ordered(
            Preparation::BusinessProfile.emit(),
            &[
                "open_time",
                "close_time",
                "businessHours.config",
                "BusinessProfileUpdate",
            ],
        );
    }

    #[test]
    fn raw_control_flow_and_client_access_cannot_be_leaf_expressions() {
        for source in [
            "return Err(error)",
            "if enabled { value } else { other }",
            "{let hidden=1;hidden}",
            "client.is_logged_in()",
            "self.client",
            "pending.await",
        ] {
            assert!(
                std::panic::catch_unwind(|| expr(source)).is_err(),
                "accepted {source}"
            );
        }
    }
}
