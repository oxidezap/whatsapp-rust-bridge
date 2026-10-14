//! Finite enum boundary policies. The compiler checks source variants; fallback
//! behavior and published spellings are explicit bridge policy, not Debug inference.
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Expr, Ident, Pat, Path, parse_quote};
struct StringEnum {
    source: Path,
    function: Ident,
    feature: &'static str,
    variants: Vec<(&'static str, &'static str)>,
    unknown_code: bool,
}
fn strings() -> Vec<StringEnum> {
    vec![
        StringEnum {
            source: parse_quote!(whatsapp_rust::ConnectStage),
            function: parse_quote!(connect_stage_str),
            feature: "",
            variants: vec![
                ("VersionFetch", "version-fetch"),
                ("Transport", "transport"),
                ("Socket", "socket"),
                ("Ready", "ready"),
            ],
            unknown_code: false,
        },
        StringEnum {
            source: parse_quote!(whatsapp_rust::ConflictKind),
            function: parse_quote!(conflict_kind_str),
            feature: "",
            variants: vec![
                ("Replaced", "replaced"),
                ("DeviceRemoved", "device_removed"),
                ("Unknown", "unknown"),
            ],
            unknown_code: false,
        },
        StringEnum {
            source: parse_quote!(whatsapp_rust::features::NewsletterVerification),
            function: parse_quote!(newsletter_verification_str),
            feature: "client-newsletter",
            variants: vec![("Verified", "Verified"), ("Unverified", "Unverified")],
            unknown_code: false,
        },
        StringEnum {
            source: parse_quote!(whatsapp_rust::features::NewsletterState),
            function: parse_quote!(newsletter_state_str),
            feature: "client-newsletter",
            variants: vec![
                ("Active", "Active"),
                ("Suspended", "Suspended"),
                ("Geosuspended", "Geosuspended"),
            ],
            unknown_code: false,
        },
        StringEnum {
            source: parse_quote!(whatsapp_rust::features::NewsletterRole),
            function: parse_quote!(newsletter_role_str),
            feature: "client-newsletter",
            variants: vec![
                ("Owner", "Owner"),
                ("Admin", "Admin"),
                ("Subscriber", "Subscriber"),
                ("Guest", "Guest"),
            ],
            unknown_code: false,
        },
        StringEnum {
            source: parse_quote!(whatsapp_rust::features::SubgroupKind),
            function: parse_quote!(subgroup_kind_str),
            feature: "client-groups",
            variants: vec![
                ("Regular", "regular"),
                ("Announcement", "announcement"),
                ("General", "general"),
            ],
            unknown_code: false,
        },
        StringEnum {
            source: parse_quote!(whatsapp_rust::wacore::types::events::ConnectFailureReason),
            function: parse_quote!(connect_failure_reason_str),
            feature: "",
            variants: [
                "Generic",
                "LoggedOut",
                "TempBanned",
                "AccountLocked",
                "UnknownLogout",
                "ClientOutdated",
                "BadUserAgent",
                "CatExpired",
                "CatInvalid",
                "NotFound",
                "ClientUnknown",
                "InternalServerError",
                "Experimental",
                "ServiceUnavailable",
            ]
            .into_iter()
            .map(|name| (name, name))
            .collect(),
            unknown_code: true,
        },
    ]
}
pub fn root() -> TokenStream {
    let strings=strings().into_iter().map(|spec|{
        let source=spec.source;let function=spec.function;let feature=spec.feature;
        let cfg=(!feature.is_empty()).then(||quote!(#[cfg(any(feature=#feature,test))]));
        let arms=spec.variants.into_iter().map(|(variant,text)|{let variant=format_ident!("{variant}");quote!(#source::#variant=>#text.into(),)});
        let fallback=if spec.unknown_code {quote!(#source::Unknown(code)=>format!("Unknown({code})"),)}else{quote!(other=>format!("{other:?}"),)};
        quote!(#cfg pub(crate) fn #function(value:&#source)->String {match value {#(#arms)*#fallback}})
    });
    quote!(#(#strings)*)
}
// Variant fields are leaves with explicit codecs. No statement blocks or runtime
// evaluator are accepted by the schema.
enum FieldCodec {
    String,
    OptionalString,
    OptionalStringRef,
    Identity,
    FlattenOwned,
    Adapter(Path),
}
struct Field {
    name: Ident,
    value: Expr,
    codec: FieldCodec,
}
enum Outcome {
    Unit(Path),
    Struct { target: Path, fields: Vec<Field> },
    SuccessField(Expr),
    Internal(&'static str),
}
struct Arm {
    pattern: Pat,
    outcome: Outcome,
}
struct Variant {
    source: Path,
    function: Ident,
    target: Path,
    borrowed: bool,
    fallible: bool,
    feature: &'static str,
    arms: Vec<Arm>,
    fallback: Option<Outcome>,
}
fn field(name: &str, value: Expr, codec: FieldCodec) -> Field {
    Field {
        name: format_ident!("{name}"),
        value,
        codec,
    }
}
fn specs() -> Vec<Variant> {
    vec![
        Variant {
            source: parse_quote!(whatsapp_rust::features::GroupHierarchy),
            function: parse_quote!(project_group_hierarchy),
            target: parse_quote!(GroupHierarchyResult),
            borrowed: true,
            fallible: false,
            feature: "client-groups",
            arms: vec![
                Arm {
                    pattern: parse_quote!(whatsapp_rust::features::GroupHierarchy::Standalone),
                    outcome: Outcome::Unit(parse_quote!(GroupHierarchyResult::Standalone)),
                },
                Arm {
                    pattern: parse_quote!(whatsapp_rust::features::GroupHierarchy::Community),
                    outcome: Outcome::Unit(parse_quote!(GroupHierarchyResult::Community)),
                },
                Arm {
                    pattern: parse_quote!(whatsapp_rust::features::GroupHierarchy::Subgroup {
                        parent,
                        kind
                    }),
                    outcome: Outcome::Struct {
                        target: parse_quote!(GroupHierarchyResult::Subgroup),
                        fields: vec![
                            field("parent", parse_quote!(parent), FieldCodec::String),
                            field(
                                "kind",
                                parse_quote!(kind),
                                FieldCodec::Adapter(parse_quote!(
                                    crate::wasm_client::subgroup_kind_str
                                )),
                            ),
                        ],
                    },
                },
            ],
            fallback: Some(Outcome::Struct {
                target: parse_quote!(GroupHierarchyResult::Unknown),
                fields: vec![field(
                    "detail",
                    parse_quote!(format!("{other:?}")),
                    FieldCodec::Identity,
                )],
            }),
        },
        Variant {
            source: parse_quote!(whatsapp_rust::features::UsernameLookup),
            function: parse_quote!(project_username_lookup),
            target: parse_quote!(UsernameLookupResult),
            borrowed: false,
            fallible: true,
            feature: "client-contacts",
            arms: vec![
                Arm {
                    pattern: parse_quote!(whatsapp_rust::features::UsernameLookup::NotFound),
                    outcome: Outcome::Unit(parse_quote!(UsernameLookupResult::NotFound)),
                },
                Arm {
                    pattern: parse_quote!(whatsapp_rust::features::UsernameLookup::KeyRequired {
                        username
                    }),
                    outcome: Outcome::Struct {
                        target: parse_quote!(UsernameLookupResult::KeyRequired),
                        fields: vec![field(
                            "username",
                            parse_quote!(username),
                            FieldCodec::OptionalString,
                        )],
                    },
                },
                Arm {
                    pattern: parse_quote!(whatsapp_rust::features::UsernameLookup::Found(user)),
                    outcome: Outcome::Struct {
                        target: parse_quote!(UsernameLookupResult::Found),
                        fields: vec![
                            field("jid", parse_quote!(user.jid), FieldCodec::String),
                            field(
                                "pn_jid",
                                parse_quote!(user.pn_jid),
                                FieldCodec::OptionalStringRef,
                            ),
                            field(
                                "username",
                                parse_quote!(user.username),
                                FieldCodec::OptionalString,
                            ),
                            field(
                                "is_business",
                                parse_quote!(user.is_business),
                                FieldCodec::Identity,
                            ),
                            field(
                                "verified_name",
                                parse_quote!(user.verified_name),
                                FieldCodec::FlattenOwned,
                            ),
                        ],
                    },
                },
            ],
            fallback: Some(Outcome::Internal("unhandled username lookup answer: ")),
        },
        Variant {
            source: parse_quote!(whatsapp_rust::MediaRetryResult),
            function: parse_quote!(project_media_retry),
            target: parse_quote!(String),
            borrowed: false,
            fallible: true,
            feature: "client-media",
            arms: vec![
                Arm {
                    pattern: parse_quote!(whatsapp_rust::MediaRetryResult::Success { direct_path }),
                    outcome: Outcome::SuccessField(parse_quote!(direct_path)),
                },
                Arm {
                    pattern: parse_quote!(whatsapp_rust::MediaRetryResult::NotFound),
                    outcome: Outcome::Internal("Media not found on server"),
                },
                Arm {
                    pattern: parse_quote!(whatsapp_rust::MediaRetryResult::DecryptionError),
                    outcome: Outcome::Internal("Media decryption error"),
                },
                Arm {
                    pattern: parse_quote!(whatsapp_rust::MediaRetryResult::GeneralError),
                    outcome: Outcome::Internal("Media reupload failed"),
                },
            ],
            fallback: None,
        },
    ]
}
fn outcome(outcome: Outcome, fallible: bool, fallback: bool) -> TokenStream {
    let value = match outcome {
        Outcome::Internal(message) => {
            let message = if fallback {
                let format = format!("{message}{{other:?}}");
                quote!(format!(#format))
            } else {
                quote!(#message)
            };
            return quote!(Err(crate::errors::internal(#message)));
        }
        Outcome::Unit(path) => quote!(#path),
        Outcome::SuccessField(value) => quote!(#value),
        Outcome::Struct { target, fields } => {
            let fields = fields.into_iter().map(|field| {
                let name = field.name;
                let value = field.value;
                let converted = match field.codec {
                    FieldCodec::String => quote!(#value.to_string()),
                    FieldCodec::OptionalString => quote!(#value.map(|value|value.to_string())),
                    FieldCodec::OptionalStringRef => {
                        quote!(#value.as_ref().map(|value|value.to_string()))
                    }
                    FieldCodec::Identity => quote!(#value),
                    FieldCodec::FlattenOwned => quote!(#value.and_then(|value|value.name)),
                    FieldCodec::Adapter(function) => quote!(#function(#value)),
                };
                quote!(#name:#converted,)
            });
            quote!(#target{#(#fields)*})
        }
    };
    if fallible { quote!(Ok(#value)) } else { value }
}
pub fn projections() -> TokenStream {
    let specs = specs().into_iter().map(|spec| {
        let declaration = variant_declaration(&spec);
        let source = spec.source;
        let function = spec.function;
        let target = spec.target;
        let feature = spec.feature;
        let source = if spec.borrowed {
            quote!(&#source)
        } else {
            quote!(#source)
        };
        let output = if spec.fallible {
            quote!(Result<#target, crate::errors::BridgeError>)
        } else {
            quote!(#target)
        };
        let arms = spec.arms.into_iter().map(|arm| {
            let pattern = arm.pattern;
            let value = outcome(arm.outcome, spec.fallible, false);
            quote!(#pattern => #value,)
        });
        let fallback = spec.fallback.map(|fallback| {
            let value = outcome(fallback, spec.fallible, true);
            quote!(other => #value,)
        });
        quote!(
            #declaration
            #[cfg(feature = #feature)]
            pub(crate) fn #function(value: #source) -> #output {
                match value { #(#arms)* #fallback }
            }
        )
    });
    let lifecycle = lifecycle_projections();
    quote!(#(#specs)* #lifecycle)
}

// Lifecycle enum declarations and projections share typed field policies.
struct LifecycleField {
    name: Ident,
    ty: syn::Type,
    attributes: TokenStream,
    source: Expr,
    codec: LifecycleCodec,
}
enum LifecycleCodec {
    NumberF64Value,
    OptionalAdapter(Path),
    Clone,
    String,
    Debug,
    NumberF64,
    OptionalF64,
    Millis,
    Adapter(Path),
}
struct LifecycleArm {
    pattern: Pat,
    name: Ident,
    attributes: TokenStream,
    fields: Vec<LifecycleField>,
}
struct LifecycleVariant {
    arguments: Vec<syn::FnArg>,
    source: Path,
    target: Ident,
    function: Ident,
    attributes: TokenStream,
    arms: Vec<LifecycleArm>,
}
fn lifecycle_specs() -> Vec<LifecycleVariant> {
    vec![
        LifecycleVariant {
            arguments: vec![parse_quote!(generation:u64)],
            source: parse_quote!(whatsapp_rust::RunCompletionReason),
            target: parse_quote!(RunCompletionResult),
            function: parse_quote!(run_completion_to_result),
            attributes: quote!(/// Why the supervised run loop started by `run()` ended.
            ///
            /// The core's `RunCompletionReason`, carried across unflattened: the
            /// discriminant says which branch ended the run, and only the
            /// `auto-reconnect-disabled` branch carries causes. Every cause is typed at
            /// the boundary (no `Debug` rendering), and every absence is an absent key.
            /// `generation` keys the result to the `run()` call that produced it; a stale
            /// task can never overwrite a newer run's result.
            ///
            /// `unknown` is not one of the core's: the enum is `#[non_exhaustive]`, and a
            /// reason added upstream has no shape here yet.
            #[derive(Debug, Clone, Serialize, Tsify)]
            #[serde(tag = "reason")]
            ),
            arms: vec![
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::RunCompletionReason::ShutdownRequested),
                    name: parse_quote!(ShutdownRequested),
                    attributes: quote!(/// A terminal shutdown was requested through `disconnect()`, `logout()`
    /// or client teardown.
    #[serde(rename = "shutdown-requested")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(generation),
                        ty: parse_quote!(f64),
                        attributes: quote!(),
                        source: parse_quote!(generation),
                        codec: LifecycleCodec::NumberF64Value,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(
                        whatsapp_rust::RunCompletionReason::AutoReconnectDisabled {
                            connection,
                            connect_error,
                            protocol_error
                        }
                    ),
                    name: parse_quote!(AutoReconnectDisabled),
                    attributes: quote!(/// The connection ended while automatic reconnection was disabled.
    /// `connection` is the final reader outcome when a connection was
    /// established; `connectError` is the final connect failure when none
    /// was; `protocolError` is the terminal stream or connect-failure cause
    /// the reader captured, when it captured one.
    #[serde(rename = "auto-reconnect-disabled", rename_all = "camelCase")]),
                    fields: vec![
                        LifecycleField {
                            name: parse_quote!(generation),
                            ty: parse_quote!(f64),
                            attributes: quote!(),
                            source: parse_quote!(generation),
                            codec: LifecycleCodec::NumberF64Value,
                        },
                        LifecycleField {
                            name: parse_quote!(connection),
                            ty: parse_quote!(Option<DisconnectReasonResult>),
                            attributes: quote!(#[serde(skip_serializing_if="Option::is_none")]),
                            source: parse_quote!(connection),
                            codec: LifecycleCodec::OptionalAdapter(parse_quote!(
                                disconnect_reason_to_result
                            )),
                        },
                        LifecycleField {
                            name: parse_quote!(connect_error),
                            ty: parse_quote!(Option<ConnectErrorResult>),
                            attributes: quote!(#[serde(skip_serializing_if="Option::is_none")]),
                            source: parse_quote!(connect_error),
                            codec: LifecycleCodec::OptionalAdapter(parse_quote!(
                                connect_error_to_result
                            )),
                        },
                        LifecycleField {
                            name: parse_quote!(protocol_error),
                            ty: parse_quote!(Option<ProtocolTerminalReasonResult>),
                            attributes: quote!(#[serde(skip_serializing_if="Option::is_none")]),
                            source: parse_quote!(protocol_error),
                            codec: LifecycleCodec::OptionalAdapter(parse_quote!(
                                protocol_terminal_to_result
                            )),
                        },
                    ],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::RunCompletionReason::Stopped),
                    name: parse_quote!(Stopped),
                    attributes: quote!(/// The supervision flag was observed cleared without a classified
    /// terminal verdict. Carries no claim about the protocol cause.
    #[serde(rename = "stopped")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(generation),
                        ty: parse_quote!(f64),
                        attributes: quote!(),
                        source: parse_quote!(generation),
                        codec: LifecycleCodec::NumberF64Value,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::RunCompletionReason::AlreadyRunning),
                    name: parse_quote!(AlreadyRunning),
                    attributes: quote!(/// Another task already owns the client's read loop.
    #[serde(rename = "already-running")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(generation),
                        ty: parse_quote!(f64),
                        attributes: quote!(),
                        source: parse_quote!(generation),
                        codec: LifecycleCodec::NumberF64Value,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(other),
                    name: parse_quote!(Unknown),
                    attributes: quote!(/// A reason this version of the bridge has no shape for. `detail`
    /// carries the core's own rendering so the host still learns what ended
    /// the run instead of receiving a neighbour's meaning.
    #[serde(rename = "unknown", rename_all = "camelCase")]),
                    fields: vec![
                        LifecycleField {
                            name: parse_quote!(generation),
                            ty: parse_quote!(f64),
                            attributes: quote!(),
                            source: parse_quote!(generation),
                            codec: LifecycleCodec::NumberF64Value,
                        },
                        LifecycleField {
                            name: parse_quote!(detail),
                            ty: parse_quote!(String),
                            attributes: quote!(),
                            source: parse_quote!(other),
                            codec: LifecycleCodec::Debug,
                        },
                    ],
                },
            ],
        },
        LifecycleVariant {
            arguments: vec![],
            source: parse_quote!(whatsapp_rust::wacore::net::DisconnectReason),
            target: parse_quote!(DisconnectReasonResult),
            function: parse_quote!(disconnect_reason_to_result),
            attributes: quote!(/// Why the transport connection ended, as the final reader observed it.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "kind")]),
            arms: vec![
                LifecycleArm {
                    pattern: parse_quote!(
                        whatsapp_rust::wacore::net::DisconnectReason::ServerClose { code, reason }
                    ),
                    name: parse_quote!(ServerClose),
                    attributes: quote!(/// The peer sent a WebSocket Close frame. `code` is the RFC 6455 close
    /// code; absent when the frame carried none.
    #[serde(rename = "server-close", rename_all = "camelCase")]),
                    fields: vec![
                        LifecycleField {
                            name: parse_quote!(code),
                            ty: parse_quote!(Option<f64>),
                            attributes: quote!(#[serde(skip_serializing_if = "Option::is_none")]),
                            source: parse_quote!(code),
                            codec: LifecycleCodec::OptionalF64,
                        },
                        LifecycleField {
                            name: parse_quote!(reason),
                            ty: parse_quote!(String),
                            attributes: quote!(),
                            source: parse_quote!(reason),
                            codec: LifecycleCodec::Clone,
                        },
                    ],
                },
                LifecycleArm {
                    pattern: parse_quote!(
                        whatsapp_rust::wacore::net::DisconnectReason::StreamEnded
                    ),
                    name: parse_quote!(StreamEnded),
                    attributes: quote!(/// The stream ended (EOF) without a Close frame.
    #[serde(rename = "stream-ended")]),
                    fields: vec![],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::wacore::net::DisconnectReason::ReadError(
                        message
                    )),
                    name: parse_quote!(ReadError),
                    attributes: quote!(/// A transport-level read/IO error ended the connection.
    #[serde(rename = "read-error", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(message),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(message),
                        codec: LifecycleCodec::Clone,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::wacore::net::DisconnectReason::Unknown),
                    name: parse_quote!(Unknown),
                    attributes: quote!(/// The reason was not reported by the transport.
    #[serde(rename = "unknown")]),
                    fields: vec![],
                },
            ],
        },
        LifecycleVariant {
            arguments: vec![],
            source: parse_quote!(whatsapp_rust::ConnectError),
            target: parse_quote!(ConnectErrorResult),
            function: parse_quote!(connect_error_to_result),
            attributes: quote!(/// Why a connection attempt failed, as the final attempt reported it.
///
/// `anyhow` causes cross as their rendered message: the detail is diagnostic
/// text, not a boundary contract.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "kind")]),
            arms: vec![
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ConnectError::AlreadyConnected),
                    name: parse_quote!(AlreadyConnected),
                    attributes: quote!(/// A connection is already up, or another attempt is in flight.
    #[serde(rename = "already-connected")]),
                    fields: vec![],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ConnectError::NotActivated),
                    name: parse_quote!(NotActivated),
                    attributes: quote!(/// Construction never completed, so the attempt was rejected before any I/O.
    #[serde(rename = "not-activated")]),
                    fields: vec![],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ConnectError::Shutdown),
                    name: parse_quote!(Shutdown),
                    attributes: quote!(/// The client was shut down; build a new client rather than reconnecting.
    #[serde(rename = "shutdown")]),
                    fields: vec![],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ConnectError::Paused),
                    name: parse_quote!(Paused),
                    attributes: quote!(/// A pause is in effect; resume lifts it.
    #[serde(rename = "paused")]),
                    fields: vec![],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ConnectError::Timeout { stage, timeout }),
                    name: parse_quote!(Timeout),
                    attributes: quote!(/// A step of the connect flow ran out of time.
    #[serde(rename = "timeout", rename_all = "camelCase")]),
                    fields: vec![
                        LifecycleField {
                            name: parse_quote!(stage),
                            ty: parse_quote!(String),
                            attributes: quote!(),
                            source: parse_quote!(stage),
                            codec: LifecycleCodec::Adapter(parse_quote!(
                                crate::wasm_client::connect_stage_str
                            )),
                        },
                        LifecycleField {
                            name: parse_quote!(timeout_ms),
                            ty: parse_quote!(f64),
                            attributes: quote!(),
                            source: parse_quote!(timeout),
                            codec: LifecycleCodec::Millis,
                        },
                    ],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ConnectError::Version(message)),
                    name: parse_quote!(Version),
                    attributes: quote!(/// The app version could not be resolved.
    #[serde(rename = "version", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(message),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(message),
                        codec: LifecycleCodec::String,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ConnectError::Transport(message)),
                    name: parse_quote!(Transport),
                    attributes: quote!(/// The transport factory could not open a connection.
    #[serde(rename = "transport", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(message),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(message),
                        codec: LifecycleCodec::String,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ConnectError::Handshake(failure)),
                    name: parse_quote!(Handshake),
                    attributes: quote!(/// The noise handshake failed after the transport was up.
    #[serde(rename = "handshake", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(reason),
                        ty: parse_quote!(HandshakeFailureResult),
                        attributes: quote!(),
                        source: parse_quote!(failure),
                        codec: LifecycleCodec::Adapter(parse_quote!(handshake_failure_to_result)),
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(other),
                    name: parse_quote!(Unknown),
                    attributes: quote!(/// A failure this version of the bridge has no shape for. `detail`
    /// carries the core's own rendering rather than a guessed kind.
    #[serde(rename = "unknown", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(detail),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(other),
                        codec: LifecycleCodec::Debug,
                    }],
                },
            ],
        },
        LifecycleVariant {
            arguments: vec![],
            source: parse_quote!(whatsapp_rust::handshake::HandshakeError),
            target: parse_quote!(HandshakeFailureResult),
            function: parse_quote!(handshake_failure_to_result),
            attributes: quote!(/// Which handshake step failed, for a `handshake` connect error.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "kind")]),
            arms: vec![
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::handshake::HandshakeError::Transport(
                        message
                    )),
                    name: parse_quote!(Transport),
                    attributes: quote!(/// The transport failed under the handshake.
    #[serde(rename = "transport", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(message),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(message),
                        codec: LifecycleCodec::String,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::handshake::HandshakeError::Core(failure)),
                    name: parse_quote!(Core),
                    attributes: quote!(/// The core Noise handshake step failed, typed per cause below.
    #[serde(rename = "core", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(reason),
                        ty: parse_quote!(NoiseHandshakeFailureResult),
                        attributes: quote!(),
                        source: parse_quote!(failure),
                        codec: LifecycleCodec::Adapter(parse_quote!(
                            noise_handshake_failure_to_result
                        )),
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::handshake::HandshakeError::Timeout),
                    name: parse_quote!(Timeout),
                    attributes: quote!(/// The handshake ran out of time, as opposed to being torn down.
    #[serde(rename = "timeout")]),
                    fields: vec![],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::handshake::HandshakeError::StreamClosed),
                    name: parse_quote!(StreamClosed),
                    attributes: quote!(/// The transport event stream closed before the handshake completed.
    #[serde(rename = "stream-closed")]),
                    fields: vec![],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::handshake::HandshakeError::Disconnected),
                    name: parse_quote!(Disconnected),
                    attributes: quote!(/// The client disconnected during the handshake.
    #[serde(rename = "disconnected")]),
                    fields: vec![],
                },
                LifecycleArm {
                    pattern: parse_quote!(
                        whatsapp_rust::handshake::HandshakeError::UnexpectedEvent(detail)
                    ),
                    name: parse_quote!(UnexpectedEvent),
                    attributes: quote!(/// The peer sent something the handshake did not expect.
    #[serde(rename = "unexpected-event", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(detail),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(detail),
                        codec: LifecycleCodec::Clone,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(other),
                    name: parse_quote!(Unknown),
                    attributes: quote!(/// A failure this version of the bridge has no shape for. `detail`
    /// carries the core's own rendering rather than a guessed kind.
    #[serde(rename = "unknown", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(detail),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(other),
                        codec: LifecycleCodec::Debug,
                    }],
                },
            ],
        },
        LifecycleVariant {
            arguments: vec![],
            source: parse_quote!(whatsapp_rust::wacore::handshake::HandshakeError),
            target: parse_quote!(NoiseHandshakeFailureResult),
            function: parse_quote!(noise_handshake_failure_to_result),
            attributes: quote!(/// Which core Noise handshake step failed, for a `core` handshake failure.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "kind")]),
            arms: vec![
                LifecycleArm {
                    pattern: parse_quote!(
                        whatsapp_rust::wacore::handshake::HandshakeError::ProtoDecode(message)
                    ),
                    name: parse_quote!(ProtoDecode),
                    attributes: quote!(/// The server hello did not decode.
    #[serde(rename = "proto-decode", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(message),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(message),
                        codec: LifecycleCodec::String,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(
                        whatsapp_rust::wacore::handshake::HandshakeError::IncompleteResponse
                    ),
                    name: parse_quote!(IncompleteResponse),
                    attributes: quote!(/// The handshake response is missing required parts.
    #[serde(rename = "incomplete-response")]),
                    fields: vec![],
                },
                LifecycleArm {
                    pattern: parse_quote!(
                        whatsapp_rust::wacore::handshake::HandshakeError::Crypto(detail)
                    ),
                    name: parse_quote!(Crypto),
                    attributes: quote!(/// A Noise crypto operation failed.
    #[serde(rename = "crypto", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(detail),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(detail),
                        codec: LifecycleCodec::Clone,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(
                        whatsapp_rust::wacore::handshake::HandshakeError::CertVerification(detail)
                    ),
                    name: parse_quote!(CertVerification),
                    attributes: quote!(/// The server certificate chain did not verify.
    #[serde(rename = "cert-verification", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(detail),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(detail),
                        codec: LifecycleCodec::Clone,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(
                        whatsapp_rust::wacore::handshake::HandshakeError::InvalidLength {
                            name,
                            expected,
                            got
                        }
                    ),
                    name: parse_quote!(InvalidLength),
                    attributes: quote!(/// A handshake field arrived at an unexpected length.
    #[serde(rename = "invalid-length", rename_all = "camelCase")]),
                    fields: vec![
                        LifecycleField {
                            name: parse_quote!(name),
                            ty: parse_quote!(String),
                            attributes: quote!(),
                            source: parse_quote!(name),
                            codec: LifecycleCodec::Clone,
                        },
                        LifecycleField {
                            name: parse_quote!(expected),
                            ty: parse_quote!(f64),
                            attributes: quote!(),
                            source: parse_quote!(expected),
                            codec: LifecycleCodec::NumberF64,
                        },
                        LifecycleField {
                            name: parse_quote!(got),
                            ty: parse_quote!(f64),
                            attributes: quote!(),
                            source: parse_quote!(got),
                            codec: LifecycleCodec::NumberF64,
                        },
                    ],
                },
                LifecycleArm {
                    pattern: parse_quote!(
                        whatsapp_rust::wacore::handshake::HandshakeError::InvalidKeyLength
                    ),
                    name: parse_quote!(InvalidKeyLength),
                    attributes: quote!(/// A handshake key arrived at an invalid length.
    #[serde(rename = "invalid-key-length")]),
                    fields: vec![],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::wacore::handshake::HandshakeError::Noise(
                        message
                    )),
                    name: parse_quote!(Noise),
                    attributes: quote!(/// The Noise protocol state machine failed.
    #[serde(rename = "noise", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(message),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(message),
                        codec: LifecycleCodec::String,
                    }],
                },
            ],
        },
        LifecycleVariant {
            arguments: vec![],
            source: parse_quote!(whatsapp_rust::ProtocolTerminalReason),
            target: parse_quote!(ProtocolTerminalReasonResult),
            function: parse_quote!(protocol_terminal_to_result),
            attributes: quote!(/// Terminal protocol cause the reader captured before its expected-disconnect
/// flag suppressed the transport outcome.
#[derive(Debug, Clone, Serialize, Tsify)]
#[serde(tag = "kind")]),
            arms: vec![
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ProtocolTerminalReason::StreamErrorCode(
                        code
                    )),
                    name: parse_quote!(StreamError),
                    attributes: quote!(/// A `<stream:error>` carrying a numeric code.
    #[serde(rename = "stream-error", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(code),
                        ty: parse_quote!(f64),
                        attributes: quote!(),
                        source: parse_quote!(code),
                        codec: LifecycleCodec::NumberF64,
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ProtocolTerminalReason::ConnectFailure(
                        reason
                    )),
                    name: parse_quote!(ConnectFailure),
                    attributes: quote!(/// A `<failure>` stanza; `reason` uses the same spelling the
    /// `connect_failure` event carries.
    #[serde(rename = "connect-failure", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(reason),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(reason),
                        codec: LifecycleCodec::Adapter(parse_quote!(
                            crate::wasm_client::connect_failure_reason_str
                        )),
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(whatsapp_rust::ProtocolTerminalReason::Conflict(cause)),
                    name: parse_quote!(Conflict),
                    attributes: quote!(/// Terminal stream conflict, without conflating removal with replacement.
    #[serde(rename = "conflict")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(cause),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(cause),
                        codec: LifecycleCodec::Adapter(parse_quote!(
                            crate::wasm_client::conflict_kind_str
                        )),
                    }],
                },
                LifecycleArm {
                    pattern: parse_quote!(other),
                    name: parse_quote!(Unknown),
                    attributes: quote!(/// A cause this version of the bridge has no shape for. `detail` carries
    /// the core's own rendering rather than a guessed kind.
    #[serde(rename = "unknown", rename_all = "camelCase")]),
                    fields: vec![LifecycleField {
                        name: parse_quote!(detail),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                        source: parse_quote!(other),
                        codec: LifecycleCodec::Debug,
                    }],
                },
            ],
        },
    ]
}
fn lifecycle_projections() -> TokenStream {
    let specs = lifecycle_specs().into_iter().map(|spec| {
        let source = spec.source;
        let target = spec.target;
        let function = spec.function;
        let arguments = spec.arguments;
        let attributes = spec.attributes;
        let mut declarations = Vec::new();
        let mut arms = Vec::new();
        for arm in spec.arms {
            let name = arm.name;
            let pattern = arm.pattern;
            let attributes = arm.attributes;
            if arm.fields.is_empty() {
                declarations.push(quote!(#attributes #name,));
                arms.push(quote!(#pattern=>#target::#name,));
            } else {
                let mut fields = Vec::new();
                let mut values = Vec::new();
                for field in arm.fields {
                    let name = field.name;
                    let ty = field.ty;
                    let attributes = field.attributes;
                    let value = field.source;
                    fields.push(quote!(#attributes #name:#ty,));
                    let value = match field.codec {
                        LifecycleCodec::NumberF64Value => quote!(#value as f64),
                        LifecycleCodec::OptionalAdapter(function) => quote!(#value.as_ref().map(#function)),
                        LifecycleCodec::Clone => quote!(#value.clone()),
                        LifecycleCodec::String => quote!(#value.to_string()),
                        LifecycleCodec::Debug => quote!(format!("{:?}",#value)),
                        LifecycleCodec::NumberF64 => quote!(*#value as f64),
                        LifecycleCodec::OptionalF64 => quote!(#value.map(|value|value as f64)),
                        LifecycleCodec::Millis => quote!(#value.as_millis() as f64),
                        LifecycleCodec::Adapter(function) => quote!(#function(#value)),
                    };
                    values.push(quote!(#name:#value,));
                }
                declarations.push(quote!(#attributes #name {#(#fields)*},));
                arms.push(quote!(#pattern=>#target::#name {#(#values)*},));
            }
        }
        let declaration=enum_tokens(attributes,&target,declarations);
        quote!(#declaration pub(crate) fn #function(#(#arguments,)*value:&#source)->#target {match value {#(#arms)*}})
    });
    quote!(#(#specs)*)
}

struct EnumDeclaration {
    target: Ident,
    attributes: TokenStream,
    branches: Vec<EnumBranch>,
}
struct EnumBranch {
    name: Ident,
    attributes: TokenStream,
    fields: Vec<EnumField>,
}
struct EnumField {
    name: Ident,
    ty: syn::Type,
    attributes: TokenStream,
}
fn enum_tokens(attributes: TokenStream, target: &Ident, branches: Vec<TokenStream>) -> TokenStream {
    quote!(#attributes pub enum #target {#(#branches)*})
}
fn variant_declaration(spec: &Variant) -> Option<TokenStream> {
    let schema = enum_declarations()
        .into_iter()
        .find(|schema| spec.target.is_ident(&schema.target))?;
    let mut outcomes = spec
        .arms
        .iter()
        .map(|arm| &arm.outcome)
        .chain(spec.fallback.iter())
        .filter(|outcome| !matches!(outcome, Outcome::Internal(_)));
    let mut branches = Vec::new();
    for branch in schema.branches {
        let outcome = outcomes
            .next()
            .expect("enum schema branch has a source arm");
        let (path, names) = match outcome {
            Outcome::Unit(path) => (path, vec![]),
            Outcome::Struct { target, fields } => (
                target,
                fields
                    .iter()
                    .map(|field| field.name.clone())
                    .collect::<Vec<_>>(),
            ),
            _ => panic!("enum branch needs a concrete target"),
        };
        assert_eq!(
            path.segments.last().unwrap().ident,
            branch.name,
            "enum schema order must follow source arms"
        );
        assert_eq!(
            names,
            branch
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect::<Vec<_>>(),
            "enum declaration and projection fields must agree"
        );
        let name = branch.name;
        let attributes = branch.attributes;
        if branch.fields.is_empty() {
            branches.push(quote!(#attributes #name,));
        } else {
            let fields = branch.fields.into_iter().map(|field| {
                let name = field.name;
                let ty = field.ty;
                let attrs = field.attributes;
                quote!(#attrs #name:#ty,)
            });
            branches.push(quote!(#attributes #name{#(#fields)*},));
        }
    }
    assert!(
        outcomes.next().is_none(),
        "every source arm has an enum declaration"
    );
    Some(enum_tokens(schema.attributes, &schema.target, branches))
}
fn enum_declarations() -> Vec<EnumDeclaration> {
    vec![
        EnumDeclaration {
            target: parse_quote!(GroupHierarchyResult),
            attributes: quote!(#[derive(Serialize, Tsify)]
            #[serde(tag = "type", rename_all = "snake_case")]
            ),
            branches: vec![
                EnumBranch {
                    name: parse_quote!(Standalone),
                    attributes: quote!(),
                    fields: vec![],
                },
                EnumBranch {
                    name: parse_quote!(Community),
                    attributes: quote!(),
                    fields: vec![],
                },
                EnumBranch {
                    name: parse_quote!(Subgroup),
                    attributes: quote!(),
                    fields: vec![
                        EnumField {
                            name: parse_quote!(parent),
                            ty: parse_quote!(String),
                            attributes: quote!(),
                        },
                        EnumField {
                            name: parse_quote!(kind),
                            ty: parse_quote!(String),
                            attributes: quote!(),
                        },
                    ],
                },
                EnumBranch {
                    name: parse_quote!(Unknown),
                    attributes: quote!(),
                    fields: vec![EnumField {
                        name: parse_quote!(detail),
                        ty: parse_quote!(String),
                        attributes: quote!(),
                    }],
                },
            ],
        },
        EnumDeclaration {
            target: parse_quote!(UsernameLookupResult),
            attributes: quote!(/// What `findByUsername` learned about a Meta username.
            ///
            /// Mirrors the core `UsernameLookup`, which is a three-way answer rather than
            /// an optional user: a username the server confirms but will not resolve
            /// without the account's username key is neither a hit nor a miss.
            #[derive(Serialize, Tsify)]
            #[serde(tag = "status")]
            ),
            branches: vec![
                EnumBranch {
                    name: parse_quote!(NotFound),
                    attributes: quote!(/// No account answers to this username, or it is not reachable from here.
    #[serde(rename = "notFound")]),
                    fields: vec![],
                },
                EnumBranch {
                    name: parse_quote!(KeyRequired),
                    attributes: quote!(/// The username exists and the server withheld the identity behind it.
    /// Repeat the lookup with the account's username key.
    #[serde(rename = "keyRequired", rename_all = "camelCase")]),
                    fields: vec![EnumField {
                        name: parse_quote!(username),
                        ty: parse_quote!(Option<String>),
                        attributes: quote!(#[serde(skip_serializing_if = "Option::is_none")]),
                    }],
                },
                EnumBranch {
                    name: parse_quote!(Found),
                    attributes: quote!(/// The username resolved to an account.
    #[serde(rename = "found", rename_all = "camelCase")]),
                    fields: vec![
                        EnumField {
                            name: parse_quote!(jid),
                            ty: parse_quote!(String),
                            attributes: quote!(),
                        },
                        EnumField {
                            name: parse_quote!(pn_jid),
                            ty: parse_quote!(Option<String>),
                            attributes: quote!(#[serde(skip_serializing_if = "Option::is_none")]),
                        },
                        EnumField {
                            name: parse_quote!(username),
                            ty: parse_quote!(Option<String>),
                            attributes: quote!(#[serde(skip_serializing_if = "Option::is_none")]),
                        },
                        EnumField {
                            name: parse_quote!(is_business),
                            ty: parse_quote!(bool),
                            attributes: quote!(),
                        },
                        EnumField {
                            name: parse_quote!(verified_name),
                            ty: parse_quote!(Option<String>),
                            attributes: quote!(#[serde(skip_serializing_if = "Option::is_none")]),
                        },
                    ],
                },
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_tables_emit_valid_concrete_rust() {
        syn::parse2::<syn::File>(root()).unwrap();
        syn::parse2::<syn::File>(projections()).unwrap();
        let specs = specs();
        assert_eq!(specs.len(), 3);
        let names: std::collections::BTreeSet<_> =
            specs.iter().map(|spec| spec.function.to_string()).collect();
        assert_eq!(names.len(), specs.len());
    }
}
