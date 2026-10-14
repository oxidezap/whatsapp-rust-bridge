//! Compile-time bindings for the concrete client operations. The catalog owns
//! admission and boundary projections; Rust still checks every emitted call.
use crate::projections::{Envelope, Projection};
use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use std::collections::BTreeSet;
use syn::{Attribute, Block, Expr, Ident, Signature, parse::Parser, parse_quote, visit::Visit};

mod business;
mod chat_actions;
mod connection;
mod contacts;
mod groups;
mod media;
mod messaging;
mod newsletter;
mod signal;

pub struct Domain {
    pub name: &'static str,
    pub operations: Vec<Operation>,
}

pub struct Operation {
    attributes: Vec<Attribute>,
    signature: Signature,
    inputs: Vec<Input>,
    boundary: Boundary,
    evaluation: Evaluation,
    admission: Admission,
    call: Expr,
    output: ReturnAdapter,
}

enum Input {
    Jids {
        binding: Ident,
        value: Expr,
    },
    Into {
        binding: Ident,
        ty: syn::Type,
        value: Expr,
    },
    Node {
        binding: Ident,
        input: Ident,
    },
    Timeout {
        binding: Ident,
        field: syn::LitStr,
        value: Expr,
        optional: bool,
    },
    Jid {
        binding: Ident,
        value: Expr,
    },
    Decode {
        binding: Ident,
        ty: syn::Type,
        field: syn::LitStr,
        value: Expr,
    },
    Class {
        binding: Ident,
        ty: syn::Type,
        field: syn::LitStr,
        class: syn::LitStr,
        method: syn::LitStr,
        value: Expr,
    },
    Message {
        binding: syn::Pat,
        jid: Expr,
        bytes: Expr,
    },
    // Domain builders stay explicit; validation rejects client access or await.
    Convert(Block),
}

impl Input {
    fn emit(&self) -> TokenStream {
        match self {
            Self::Jids { binding, value } => {
                quote!(let #binding:Vec<wacore_binary::jid::Jid>=#value.iter().map(|jid|parse_jid(jid)).collect::<Result<_,_>>()?;)
            }
            Self::Into { binding, ty, value } => quote!(let #binding:#ty=#value.into();),
            Self::Node { binding, input } => {
                quote!(let #input:JsValue=#input.into();let #binding=js_to_node(&#input)?;)
            }
            Self::Timeout {
                binding,
                field,
                value,
                optional,
            } => {
                let helper = if *optional {
                    parse_quote!(parse_optional_timeout_ms)
                } else {
                    parse_quote!(parse_timeout_ms)
                };
                let helper: Ident = helper;
                quote!(let #binding=#helper(#field,#value)?;)
            }
            Self::Jid { binding, value } => quote!(let #binding = parse_jid(#value)?;),
            Self::Decode {
                binding,
                ty,
                field,
                value,
            } => quote!(let #binding = from_js_input::<#ty>(#field, #value)?;),
            Self::Class {
                binding,
                ty,
                field,
                class,
                method,
                value,
            } => quote!(let #binding = from_js_class::<#ty>(#field, #class, #method, #value)?;),
            Self::Message {
                binding,
                jid,
                bytes,
            } => quote!(let #binding = parse_jid_and_msg_bytes(#jid, #bytes)?;),
            Self::Convert(block) => {
                let statements = &block.stmts;
                quote!(#(#statements)*)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Boundary {
    AsyncBorrowed,
    SyncBorrowed,
    SyncMutable,
    OwnedPromise,
}

#[derive(Clone, Copy)]
enum Evaluation {
    AwaitFallible,
    Await,
    Immediate,
}

enum Admission {
    CommittedReceipts,
    Unwaited(Unwaited),
    Adapter(AdmissionAdapter),
    Online,
    OnlineIf {
        condition: Expr,
        otherwise: Unwaited,
    },
}

enum AdmissionAdapter {
    Lifecycle,
    Reachability,
    Withdrawal,
    Diagnostics,
    Alias,
    StreamEncryption,
    StreamingRetry,
    RawForwarding,
    LocalConversion,
}

enum Unwaited {
    Local,
    ConnectionBound,
    ThisSocket,
    Redriven,
    Opaque,
    Deferred,
    Cached,
}

enum ReturnAdapter {
    Dto {
        projection: Projection,
        envelope: Envelope,
    },
    MapError,
    Direct,
    Unit,
    ProjectBlock {
        binding: syn::Pat,
        body: Block,
    },
    Project {
        binding: Ident,
        expression: Expr,
    },
}

fn attributes(tokens: TokenStream) -> Vec<Attribute> {
    Attribute::parse_outer
        .parse2(tokens)
        .expect("catalog attributes")
}

pub fn catalog() -> Vec<Domain> {
    vec![
        Domain {
            name: "connection",
            operations: connection::catalog(),
        },
        Domain {
            name: "messaging",
            operations: messaging::catalog(),
        },
        Domain {
            name: "contacts",
            operations: contacts::catalog(),
        },
        Domain {
            name: "chat_actions",
            operations: chat_actions::catalog(),
        },
        Domain {
            name: "newsletter",
            operations: newsletter::catalog(),
        },
        Domain {
            name: "signal",
            operations: signal::catalog(),
        },
        Domain {
            name: "groups",
            operations: groups::catalog(),
        },
        Domain {
            name: "business",
            operations: business::catalog(),
        },
        Domain {
            name: "media",
            operations: media::catalog(),
        },
    ]
}

fn js_name(attributes: &[Attribute]) -> Result<String, String> {
    let mut name = None;
    for attribute in attributes
        .iter()
        .filter(|a| a.path().is_ident("wasm_bindgen"))
    {
        attribute
            .parse_nested_meta(|meta| {
                if meta.path.is_ident("js_name") {
                    let value = meta.value()?;
                    name = Some(if value.peek(syn::LitStr) {
                        value.parse::<syn::LitStr>()?.value()
                    } else {
                        value.parse::<Ident>()?.to_string()
                    });
                    Ok(())
                } else if meta.path.is_ident("unchecked_return_type") {
                    let _: syn::LitStr = meta.value()?.parse()?;
                    Ok(())
                } else if meta.path.is_ident("skip_typescript") {
                    Ok(())
                } else {
                    Err(meta.error("declare other boundary attributes on the explicit adapter"))
                }
            })
            .map_err(|e| e.to_string())?;
    }
    name.ok_or_else(|| "operation requires a public js_name".to_string())
}

pub fn validate(domains: &[Domain]) -> Result<(), String> {
    let mut domain_names = BTreeSet::new();
    let mut rust_names = BTreeSet::new();
    let mut js_names = BTreeSet::new();
    for domain in domains {
        if !domain_names.insert(domain.name)
            || domain.name == "storage"
            || domain.name.is_empty()
            || !domain
                .name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c == b'_')
        {
            return Err(format!("invalid or duplicate domain {}", domain.name));
        }
        for operation in &domain.operations {
            let signature = &operation.signature;
            if signature.asyncness.is_some()
                != matches!(operation.boundary, Boundary::AsyncBorrowed)
                || !signature.generics.params.is_empty()
            {
                return Err(format!(
                    "{} must be a concrete async operation",
                    signature.ident
                ));
            }
            if !matches!(signature.inputs.first(), Some(syn::FnArg::Receiver(r)) if
            match operation.boundary {
                Boundary::SyncMutable => matches!(r.kind, syn::ReceiverKind::Reference(_, None, Some(_))),
                _ => matches!(r.kind, syn::ReceiverKind::Reference(_, None, None)),
            }) {
                return Err(format!("{} must borrow &self", signature.ident));
            }
            if !rust_names.insert(signature.ident.to_string()) {
                return Err(format!("duplicate Rust operation {}", signature.ident));
            }
            if let syn::ReturnType::Type(_, result) = &signature.output
                && let syn::Type::Path(path) = result.as_ref()
                && let Some(segment) = path.path.segments.last()
                && segment.ident == "Result"
            {
                let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                    return Err("missing Result arguments".into());
                };
                if arguments.args.len() != 2
                    || arguments.args.last().unwrap().to_token_stream().to_string()
                        != quote!(crate::errors::BridgeError).to_string()
                {
                    return Err(format!("{} must preserve BridgeError", signature.ident));
                }
            }
            for input in &operation.inputs {
                let mut boundary = BoundaryAccess::default();
                let tokens = input.emit();
                boundary.visit_block(&syn::parse2(quote!({#tokens})).unwrap());
                if boundary.awaited || boundary.receiver || boundary.admitted_client {
                    return Err(format!(
                        "{} input conversion must precede admission",
                        signature.ident
                    ));
                }
            }
            if matches!(
                operation.admission,
                Admission::Online | Admission::OnlineIf { .. } | Admission::CommittedReceipts
            ) && !matches!(operation.boundary, Boundary::AsyncBorrowed)
            {
                return Err("online admission requires a borrowed async boundary".into());
            }
            if matches!(operation.admission, Admission::CommittedReceipts) {
                let unit = signature.output.to_token_stream().to_string()
                    == quote!(-> Result<(), crate::errors::BridgeError>).to_string();
                let keys = signature.inputs.len() == 2
                    && matches!(signature.inputs.last(), Some(syn::FnArg::Typed(arg)) if arg.pat.to_token_stream().to_string() == "keys" && arg.ty.to_token_stream().to_string() == "JsValue");
                let invocation = operation.call.to_token_stream().to_string();
                let call = invocation
                    == quote!(client.mark_as_read(&chat, participant.as_ref(), &id_refs))
                        .to_string()
                    || invocation
                        == quote!(client.mark_as_played(&chat, participant.as_ref(), &id_refs))
                            .to_string();
                if !unit
                    || !keys
                    || !call
                    || !operation.inputs.is_empty()
                    || !matches!(operation.evaluation, Evaluation::AwaitFallible)
                    || !matches!(operation.output, ReturnAdapter::Unit)
                {
                    return Err("committed receipt template requires keys, fallible unit settlement and a receipt call".into());
                }
            }
            if matches!(operation.boundary, Boundary::OwnedPromise)
                && (!operation.inputs.is_empty()
                    || !matches!(
                        operation.admission,
                        Admission::Adapter(AdmissionAdapter::Lifecycle)
                    )
                    || !matches!(operation.evaluation, Evaluation::Immediate)
                    || !matches!(operation.output, ReturnAdapter::Direct)
                    || signature.output.to_token_stream().to_string()
                        != quote!(-> js_sys::Promise).to_string())
            {
                return Err(
                    "owned Promise must capture synchronously through a lifecycle adapter".into(),
                );
            }
            if let Admission::Adapter(adapter) = &operation.admission {
                adapter.validate(operation)?;
            } else {
                let mut boundary = BoundaryAccess::default();
                boundary.visit_expr(&operation.call);
                if boundary.receiver {
                    return Err("call must use its declared admission, not self".into());
                }
            }
            let projection = match &operation.output {
                ReturnAdapter::ProjectBlock { body, .. } => Some(body.to_token_stream()),
                ReturnAdapter::Project { expression, .. } => Some(quote!({#expression})),
                _ => None,
            };
            if let Some(tokens) = projection {
                let mut boundary = BoundaryAccess::default();
                boundary.visit_block(&syn::parse2(tokens).unwrap());
                if boundary.awaited || boundary.receiver || boundary.admitted_client {
                    return Err("result projection cannot suspend or reach the client".into());
                }
            }
            let name = js_name(&operation.attributes)?;
            if !js_names.insert(name.clone()) {
                return Err(format!("duplicate JS export {name}"));
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct BoundaryAccess {
    receiver: bool,
    admitted_client: bool,
    awaited: bool,
}
impl<'ast> Visit<'ast> for BoundaryAccess {
    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        self.receiver |= node.path.is_ident("self");
        self.admitted_client |= node.path.is_ident("client");
        syn::visit::visit_expr_path(self, node);
    }
    fn visit_expr_await(&mut self, node: &'ast syn::ExprAwait) {
        self.awaited = true;
        syn::visit::visit_expr_await(self, node);
    }
}

impl AdmissionAdapter {
    fn validate(&self, operation: &Operation) -> Result<(), String> {
        let name = operation.signature.ident.to_string();
        let expected: Option<Expr> = match self {
            Self::Lifecycle => Some(match name.as_str() {
                "run" => parse_quote!(self.execute_run()),
                "connect" => parse_quote!(self.prepare_connect()),
                "disconnect" => parse_quote!(self.prepare_disconnect()),
                "logout" => parse_quote!(self.prepare_logout()),
                "wait_for_run_completion" => parse_quote!(self.prepare_wait_for_run_completion()),
                _ => return Err("unknown lifecycle adapter".into()),
            }),
            Self::StreamEncryption => Some(parse_quote!(encrypt_stream(input, output, mt))),
            Self::StreamingRetry => Some(parse_quote!(upload_encrypted_stream(
                &self.client,
                get_body,
                media_key,
                file_sha256,
                file_enc_sha256,
                file_length,
                mt
            ))),
            Self::RawForwarding => {
                Some(parse_quote!(self.execute_set_raw_node_forwarding(enabled)))
            }
            _ => None,
        };
        if let Some(expected) = expected
            && expected.to_token_stream().to_string()
                != operation.call.to_token_stream().to_string()
        {
            return Err("registered adapter invocation differs from its declared contract".into());
        }

        let allowed: &[&str] = match self {
            Self::Lifecycle => &[
                "run",
                "connect",
                "disconnect",
                "logout",
                "wait_for_run_completion",
            ],
            Self::Reachability => &["reachability", "wait_until_reachable"],
            Self::Withdrawal => &["withdraw_parked_calls"],
            Self::Diagnostics => &["get_core_allocation_snapshot"],
            Self::Alias => &["newsletter_mute"],
            Self::StreamEncryption => &["encrypt_media_stream"],
            Self::StreamingRetry => &["upload_encrypted_media_stream"],
            Self::RawForwarding => &["set_raw_node_forwarding"],
            Self::LocalConversion => &["jid_to_signal_protocol_address"],
        };
        if matches!(self, Self::Lifecycle)
            && !matches!(
                (name.as_str(), operation.boundary),
                ("run", Boundary::SyncMutable)
                    | (
                        "connect" | "disconnect" | "logout" | "wait_for_run_completion",
                        Boundary::OwnedPromise
                    )
            )
        {
            return Err("lifecycle ownership must be explicit".into());
        }
        if !allowed.contains(&name.as_str()) {
            return Err(format!("{name} needs a registered admission adapter"));
        }
        Ok(())
    }
}

impl Unwaited {
    fn name(&self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::ConnectionBound => "ConnectionBound",
            Self::ThisSocket => "ThisSocket",
            Self::Redriven => "Redriven",
            Self::Opaque => "Opaque",
            Self::Deferred => "Deferred",
            Self::Cached => "Cached",
        }
    }
}

impl Operation {
    fn emit(&self) -> TokenStream {
        let attributes = &self.attributes;
        let signature = &self.signature;
        let preparation: Vec<_> = self.inputs.iter().map(Input::emit).collect();
        if matches!(self.admission, Admission::CommittedReceipts) {
            let call = &self.call;
            return quote!(#(#attributes)* pub #signature {
                #(#preparation)*
                let mut sent = false;
                for (chat, participant, ids) in group_receipt_keys("keys", keys)? {
                    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                    let client = if sent {
                        self.client.online_committed().await
                    } else {
                        self.client.online().await?
                    };
                    #call.await?;
                    sent = true;
                }
                Ok(())
            });
        }
        let client = match &self.admission {
            Admission::CommittedReceipts => unreachable!("emitted above"),
            Admission::Adapter(_) => TokenStream::new(),
            Admission::Unwaited(reason) => {
                let reason = Ident::new(reason.name(), proc_macro2::Span::call_site());
                quote!(self.client.unwaited(Unwaited::#reason))
            }
            Admission::Online => quote!(self.client.online().await?),
            Admission::OnlineIf {
                condition,
                otherwise,
            } => {
                let reason = Ident::new(otherwise.name(), proc_macro2::Span::call_site());
                quote!(
                    if #condition { self.client.online().await? }
                    else { self.client.unwaited(Unwaited::#reason) }
                )
            }
        };
        let expression = &self.call;
        let call = match self.evaluation {
            Evaluation::AwaitFallible | Evaluation::Await => quote!(#expression.await),
            Evaluation::Immediate => quote!(#expression),
        };
        let projected_call = match self.evaluation {
            Evaluation::AwaitFallible => quote!(#call?),
            _ => call.clone(),
        };
        let binding = if matches!(self.admission, Admission::Adapter(_)) {
            TokenStream::new()
        } else {
            quote!(let client = #client;)
        };
        if matches!(self.boundary, Boundary::OwnedPromise) {
            return quote!(#(#attributes)* pub #signature {
                wasm_bindgen_futures::future_to_promise(#expression)
            });
        }
        let output = match &self.output {
            ReturnAdapter::Dto {
                projection,
                envelope,
            } => {
                let output = projection.envelope(*envelope, quote!(value));
                quote!(let value = #projected_call; #output)
            }
            ReturnAdapter::MapError => {
                quote!(#call.map_err(crate::errors::BridgeError::from))
            }
            ReturnAdapter::Direct => match &self.call {
                Expr::Block(block) if matches!(self.evaluation, Evaluation::Immediate) => {
                    let statements = &block.block.stmts;
                    quote!(#(#statements)*)
                }
                _ => quote!(#call),
            },
            ReturnAdapter::Unit => quote!(#projected_call; Ok(())),
            ReturnAdapter::ProjectBlock { binding, body } => {
                let statements = &body.stmts;
                quote!(let #binding = #projected_call; #(#statements)*)
            }
            ReturnAdapter::Project {
                binding,
                expression,
            } => quote!(
                let #binding = #projected_call;
                #expression
            ),
        };
        quote!(#(#attributes)* pub #signature {
            #(#preparation)*
            #binding
            #output
        })
    }
}

pub fn emit(domain: &Domain) -> TokenStream {
    let methods = domain.operations.iter().map(Operation::emit);
    quote!(#[wasm_bindgen] impl WasmWhatsAppClient { #(#methods)* })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_exports_cannot_silently_replace_an_operation() {
        let mut domains = catalog();
        let duplicate = groups::catalog().remove(0);
        domains[1].operations.push(duplicate);
        assert!(validate(&domains).unwrap_err().contains("duplicate Rust"));
        let mut domains = catalog();
        let mut duplicate = groups::catalog().remove(0);
        duplicate.signature.ident = parse_quote!(different_rust_name);
        domains[1].operations.push(duplicate);
        assert!(validate(&domains).unwrap_err().contains("duplicate JS"));
    }

    #[test]
    fn emitted_catalog_is_concrete_and_has_no_duplicate_exports() {
        let domains = catalog();
        validate(&domains).unwrap();
        for domain in &domains {
            syn::parse2::<syn::File>(emit(domain)).unwrap();
        }
    }

    #[test]
    fn hidden_admission_and_owned_borrows_fail_generation() {
        let mut domains = catalog();
        domains[0].operations[0]
            .inputs
            .push(Input::Convert(parse_quote!({
                let _ = self.client.online().await?;
            })));
        assert!(
            validate(&domains)
                .unwrap_err()
                .contains("precede admission")
        );
        let mut domains = catalog();
        domains[0].operations[0].call = parse_quote!(self.client.online().await?.bots().list());
        assert!(
            validate(&domains)
                .unwrap_err()
                .contains("declared admission")
        );
        let mut domains = catalog();
        let owned = domains[0]
            .operations
            .iter_mut()
            .find(|o| matches!(o.boundary, Boundary::OwnedPromise))
            .unwrap();
        owned.boundary = Boundary::AsyncBorrowed;
        assert!(validate(&domains).is_err());
        let mut domains = catalog();
        let owned = domains[0]
            .operations
            .iter_mut()
            .find(|o| matches!(o.boundary, Boundary::OwnedPromise))
            .unwrap();
        owned.inputs.push(Input::Convert(parse_quote!({
            let x = 1;
        })));
        assert!(
            validate(&domains)
                .unwrap_err()
                .contains("capture synchronously")
        );
    }

    #[test]
    fn receipt_policy_and_result_projection_cannot_bypass_the_template() {
        let mut domains = catalog();
        let receipts = domains
            .iter_mut()
            .flat_map(|d| &mut d.operations)
            .find(|o| matches!(o.admission, Admission::CommittedReceipts))
            .unwrap();
        receipts.evaluation = Evaluation::Immediate;
        assert!(validate(&domains).unwrap_err().contains("receipt template"));
        let mut domains = catalog();
        domains[0].operations[0].output = ReturnAdapter::ProjectBlock {
            binding: parse_quote!(value),
            body: parse_quote!({
                client.is_logged_in();
                Ok(value)
            }),
        };
        assert!(
            validate(&domains)
                .unwrap_err()
                .contains("projection cannot")
        );
    }

    #[test]
    fn emitted_receipts_withdraw_only_before_effect_and_stop_on_failure() {
        let domain = catalog()
            .into_iter()
            .find(|d| d.name == "messaging")
            .unwrap();
        let mut methods = Vec::new();
        for operation in domain
            .operations
            .iter()
            .filter(|o| matches!(o.admission, Admission::CommittedReceipts))
        {
            let mut method: syn::ImplItemFn = syn::parse2(operation.emit()).unwrap();
            method.attrs.clear();
            for arg in &mut method.sig.inputs {
                if let syn::FnArg::Typed(arg) = arg {
                    arg.attrs.clear();
                }
            }
            methods.push(method);
        }
        let fixture = include_str!("receipt_fixture.rs.txt");
        let source = format!(
            "{}\nimpl WasmWhatsAppClient {{ {} }}",
            fixture,
            quote!(#(#methods)*)
        );
        let root = std::env::temp_dir().join(format!("bridge-receipts-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("receipt.rs");
        let binary = root.join("receipt");
        std::fs::write(&path, source).unwrap();
        let compiler = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        let compiled = std::process::Command::new(compiler)
            .args(["--edition=2024", "--crate-name=receipt_fixture"])
            .arg(&path)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let result = std::process::Command::new(&binary).output().unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn unsupported_receiver_generics_and_error_channels_fail_generation() {
        for signature in [
            parse_quote!(async fn wrong<T>(&self) -> Result<(), crate::errors::BridgeError>),
            parse_quote!(async fn wrong(&mut self) -> Result<(), crate::errors::BridgeError>),
            parse_quote!(async fn wrong(&self) -> Result<(), JsValue>),
        ] {
            let mut domains = catalog();
            domains[0].operations[0].signature = signature;
            assert!(validate(&domains).is_err());
        }
    }
}
