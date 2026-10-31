//! Compile-time bindings for the concrete client operations. The catalog owns
//! admission and boundary projections; Rust still checks every emitted call.
use crate::operation_codecs::{
    Converter, InputCodec, JidFamily, ObjectShape, OutputCodec, ResultError, SerializationError,
    Serializer, ValueCodec, VariantCodec,
};
use crate::preparations::Preparation;
use crate::projections::{Envelope, Projection};
use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use std::collections::BTreeSet;
use syn::{Attribute, Expr, Ident, Signature, parse::Parser, parse_quote, visit::Visit};

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
    Preparation(Preparation),
    Codec(crate::operation_codecs::InputCodec),
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
}

impl Input {
    fn emit(&self) -> TokenStream {
        match self {
            Self::Preparation(preparation) => preparation.emit(),
            Self::Codec(codec) => codec.emit(),
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
        }
    }
}

#[derive(Clone, Copy)]
enum Boundary {
    AsyncOwned,
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
    Codec(crate::operation_codecs::OutputCodec),
    Dto {
        projection: Projection,
        envelope: Envelope,
    },
    MapError,
    Direct,
    Unit,
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
                    Err(meta.error("operation declarations must be generated, not hidden"))
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
            if signature.asyncness.is_some() != matches!(operation.boundary, Boundary::AsyncOwned)
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
            ) && !matches!(operation.boundary, Boundary::AsyncOwned)
            {
                return Err("online admission requires an owned async boundary".into());
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
        let emitted = self.emit_body();
        if !matches!(self.boundary, Boundary::AsyncOwned) {
            return emitted;
        }
        crate::owned_operations::lower(
            emitted,
            matches!(self.admission, Admission::Adapter(AdmissionAdapter::Alias)),
            match &self.output {
                ReturnAdapter::Dto {
                    projection,
                    envelope: Envelope::Record,
                } => Some(format!("Record<string, {}>", projection.typescript_name())),
                ReturnAdapter::Codec(OutputCodec::GroupRecord { .. }) => Some(format!(
                    "Record<string, {}>",
                    Projection::GroupOverview.typescript_name()
                )),
                _ => None,
            },
        )
    }

    fn emit_body(&self) -> TokenStream {
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
            ReturnAdapter::Codec(codec) => codec.emit(projected_call),
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
    let tests = if domain.name == "signal" {
        crate::operation_codecs::wasm_fixture()
    } else {
        TokenStream::new()
    };
    quote!(#[wasm_bindgen] impl WasmWhatsAppClient { #(#methods)* } #tests)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TemporaryFixture {
        root: std::path::PathBuf,
        crate_name: &'static str,
    }

    impl TemporaryFixture {
        fn new(crate_name: &'static str) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            loop {
                let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let root = std::env::temp_dir().join(format!(
                    "bridge-codegen-{crate_name}-{}-{sequence}",
                    std::process::id()
                ));
                match std::fs::create_dir(&root) {
                    Ok(()) => return Self { root, crate_name },
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create fixture directory: {error}"),
                }
            }
        }
    }

    impl Drop for TemporaryFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn compile_and_run_fixture(fixture: TemporaryFixture, source: &str) {
        let path = fixture.root.join("fixture.rs");
        let binary = fixture.root.join("fixture");
        std::fs::write(&path, source).unwrap();
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let compiled = std::process::Command::new(compiler)
            .args(["--edition=2024", "--crate-name", fixture.crate_name])
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
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn fixture_directory_is_removed_when_compilation_fails() {
        let fixture = TemporaryFixture::new("invalid_fixture");
        let path = fixture.root.clone();
        let failed = std::panic::catch_unwind(|| {
            compile_and_run_fixture(
                fixture,
                "compile_error!(\"intentional fixture failure\"); fn main() {}",
            );
        });
        let failure = failed.expect_err("invalid fixture unexpectedly compiled");
        let diagnostic = failure
            .downcast_ref::<String>()
            .expect("compiler diagnostic");
        assert!(
            diagnostic.contains("intentional fixture failure"),
            "{diagnostic}"
        );
        assert!(
            !path.exists(),
            "compile failure leaked its fixture directory"
        );
    }

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
        domains[0].operations[0].inputs.push(Input::Jid {
            binding: parse_quote!(hidden),
            value: parse_quote!(self.client.online().await?),
        });
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
        owned.boundary = Boundary::AsyncOwned;
        assert!(validate(&domains).is_err());
        let mut domains = catalog();
        let owned = domains[0]
            .operations
            .iter_mut()
            .find(|o| matches!(o.boundary, Boundary::OwnedPromise))
            .unwrap();
        owned.inputs.push(Input::Jid {
            binding: parse_quote!(extra),
            value: parse_quote!(jid),
        });
        assert!(
            validate(&domains)
                .unwrap_err()
                .contains("capture synchronously")
        );
    }

    #[test]
    fn receipt_policy_cannot_bypass_the_template() {
        let mut domains = catalog();
        let receipts = domains
            .iter_mut()
            .flat_map(|d| &mut d.operations)
            .find(|o| matches!(o.admission, Admission::CommittedReceipts))
            .unwrap();
        receipts.evaluation = Evaluation::Immediate;
        assert!(validate(&domains).unwrap_err().contains("receipt template"));
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
            let mut method: syn::ImplItemFn = syn::parse2(operation.emit_body()).unwrap();
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
        compile_and_run_fixture(TemporaryFixture::new("receipt_fixture"), &source);
    }

    #[test]
    fn an_operation_cannot_hide_from_the_published_contract_sweep() {
        let mut domains = catalog();
        domains[0].operations[0]
            .attributes
            .push(parse_quote!(#[wasm_bindgen(skip_typescript)]));
        assert!(
            validate(&domains)
                .unwrap_err()
                .contains("operation declarations must be generated")
        );
    }

    #[test]
    fn every_async_operation_releases_the_wrapper_before_returning() {
        let domains = catalog();
        validate(&domains).unwrap();
        let mut checked = 0;
        for operation in domains.iter().flat_map(|d| &d.operations) {
            if operation.signature.asyncness.is_none() {
                continue;
            }
            checked += 1;
            let method: syn::ImplItemFn = syn::parse2(operation.emit()).unwrap();
            assert!(
                method.sig.asyncness.is_none(),
                "{} retains a wrapper borrow",
                method.sig.ident
            );
            struct Futures;
            impl<'ast> syn::visit::Visit<'ast> for Futures {
                fn visit_expr_async(&mut self, future: &'ast syn::ExprAsync) {
                    let mut access = BoundaryAccess::default();
                    access.visit_block(&future.block);
                    assert!(!access.receiver, "future captures the wrapper");
                    syn::visit::visit_expr_async(self, future);
                }
            }
            Futures.visit_block(&method.block);
        }
        assert!(checked > 0);
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
    #[test]
    fn catalog_codecs_execute_defaults_error_order_and_output_contracts() {
        let domains = catalog();
        let operation = |name: &str| {
            domains
                .iter()
                .flat_map(|domain| &domain.operations)
                .find(|operation| operation.signature.ident == name)
                .unwrap()
        };
        let input = |name: &str, predicate: fn(&InputCodec) -> bool| {
            operation(name)
                .inputs
                .iter()
                .find_map(|input| match input {
                    Input::Codec(codec) if predicate(codec) => Some(codec.emit()),
                    _ => None,
                })
                .unwrap()
                .to_string()
        };
        let mut source = include_str!("codec_fixture.rs.txt").to_owned();
        for (marker, code) in [
            (
                "__PICTURE__",
                input("profile_picture_url", |codec| {
                    matches!(codec, InputCodec::Enum { .. })
                }),
            ),
            (
                "__CATALOG__",
                input("get_catalog", |codec| {
                    matches!(codec, InputCodec::DefaultOverlay { .. })
                }),
            ),
            (
                "__BEFORE__",
                input("newsletter_messages", |codec| {
                    matches!(codec, InputCodec::Scalar { .. })
                }),
            ),
            (
                "__SEND_OPTIONS__",
                input("relay_message_bytes_with_options", |codec| {
                    matches!(codec, InputCodec::SendOptions { .. })
                }),
            ),
            (
                "__PARENT_GUARD__",
                input("send_comment_bytes", |codec| {
                    matches!(codec, InputCodec::ParentKeyGuard { .. })
                }),
            ),
        ] {
            assert!(source.contains(marker));
            source = source.replace(marker, &code);
        }
        let ReturnAdapter::Codec(codec) = &operation("send_reaction").output else {
            panic!("generated send result codec")
        };
        source = source.replace("__MESSAGE_ID__", &codec.emit(quote!(result)).to_string());
        compile_and_run_fixture(TemporaryFixture::new("codec_fixture"), &source);
    }
}
