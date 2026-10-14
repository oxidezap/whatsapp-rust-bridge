//! Compile-time bindings for selected, concrete operations. The catalog owns
//! admission and boundary projections; Rust still checks every emitted call.
use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use std::collections::BTreeSet;
use syn::{Attribute, Block, Expr, Ident, Signature, parse::Parser, parse_quote};

mod business;
mod groups;
mod media;

pub struct Domain {
    pub name: &'static str,
    pub operations: Vec<Operation>,
}

pub struct Operation {
    attributes: Vec<Attribute>,
    signature: Signature,
    prepare: Block,
    admission: Admission,
    call: Expr,
    output: ReturnAdapter,
}

enum Admission {
    Online,
    OnlineIf {
        condition: Expr,
        otherwise: Unwaited,
    },
}

enum Unwaited {
    Cached,
}

enum ReturnAdapter {
    MapError,
    Project { binding: Ident, expression: Expr },
}

fn attributes(tokens: TokenStream) -> Vec<Attribute> {
    Attribute::parse_outer
        .parse2(tokens)
        .expect("catalog attributes")
}

pub fn catalog() -> Vec<Domain> {
    vec![
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
                    name = Some(meta.value()?.parse::<Ident>()?.to_string());
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
    let mut rust_names = BTreeSet::new();
    let mut js_names = BTreeSet::new();
    for domain in domains {
        for operation in &domain.operations {
            let signature = &operation.signature;
            if signature.asyncness.is_none() || !signature.generics.params.is_empty() {
                return Err(format!(
                    "{} must be a concrete async operation",
                    signature.ident
                ));
            }
            if !matches!(signature.inputs.first(), Some(syn::FnArg::Receiver(r)) if matches!(r.kind, syn::ReceiverKind::Reference(_, None, None)))
            {
                return Err(format!("{} must borrow &self", signature.ident));
            }
            if !rust_names.insert(signature.ident.to_string()) {
                return Err(format!("duplicate Rust operation {}", signature.ident));
            }
            let syn::ReturnType::Type(_, result) = &signature.output else {
                return Err(format!(
                    "{} must return Result<_, BridgeError>",
                    signature.ident
                ));
            };
            let syn::Type::Path(result) = result.as_ref() else {
                return Err(format!("{} requires a typed Result", signature.ident));
            };
            let Some(segment) = result.path.segments.last() else {
                return Err("missing result type".into());
            };
            let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                return Err("missing Result arguments".into());
            };
            if segment.ident != "Result"
                || arguments.args.len() != 2
                || arguments.args.last().unwrap().to_token_stream().to_string()
                    != quote!(crate::errors::BridgeError).to_string()
            {
                return Err(format!("{} must preserve BridgeError", signature.ident));
            }
            let name = js_name(&operation.attributes)?;
            if !js_names.insert(name.clone()) {
                return Err(format!("duplicate JS export {name}"));
            }
        }
    }
    Ok(())
}

impl Operation {
    fn emit(&self) -> TokenStream {
        let attributes = &self.attributes;
        let signature = &self.signature;
        let preparation = &self.prepare.stmts;
        let client = match &self.admission {
            Admission::Online => quote!(self.client.online().await?),
            Admission::OnlineIf {
                condition,
                otherwise: Unwaited::Cached,
            } => quote!(
                if #condition { self.client.online().await? }
                else { self.client.unwaited(Unwaited::Cached) }
            ),
        };
        let call = &self.call;
        let output = match &self.output {
            ReturnAdapter::MapError => {
                quote!(#call.await.map_err(crate::errors::BridgeError::from))
            }
            ReturnAdapter::Project {
                binding,
                expression,
            } => quote!(
                let #binding = #call.await?;
                #expression
            ),
        };
        quote!(#(#attributes)* pub #signature {
            #(#preparation)*
            let client = #client;
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
