use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::parse_quote;

// A synchronous wasm entry captures only owned state. Rust's 'static check on
// future_to_promise rejects any input or wrapper borrow crossing this boundary.
pub fn lower(tokens: TokenStream, alias: bool, declared_result: Option<String>) -> TokenStream {
    let mut method: syn::ImplItemFn = syn::parse2(tokens).unwrap();
    method.sig.asyncness = None;
    let (value_type, fallible): (syn::Type, bool) = match &method.sig.output {
        syn::ReturnType::Default => (parse_quote!(()), false),
        syn::ReturnType::Type(_, ty) => {
            if let syn::Type::Path(path) = ty.as_ref()
                && path.path.segments.last().unwrap().ident == "Result"
            {
                let syn::PathArguments::AngleBracketed(args) =
                    &path.path.segments.last().unwrap().arguments
                else {
                    panic!("Result arguments")
                };
                let syn::GenericArgument::Type(value) = args.args.first().unwrap() else {
                    panic!("Result value")
                };
                (value.clone(), true)
            } else {
                (ty.as_ref().clone(), false)
            }
        }
    };
    let unit = matches!(&value_type, syn::Type::Tuple(t) if t.elems.is_empty());
    let conversion = if unit {
        quote!(JsValue::UNDEFINED)
    } else {
        quote!(value.into())
    };
    let return_type = format!(
        "Promise<{}>",
        declared_result.unwrap_or_else(|| typescript_result(&value_type))
    );
    method
        .attrs
        .push(parse_quote!(#[wasm_bindgen(unchecked_return_type = #return_type)]));
    method.sig.output = parse_quote!(-> js_sys::Promise);

    // Aliases delegate to the same generated boundary and retain no future.
    if alias {
        fn delegated(expr: &syn::Expr) -> &syn::Expr {
            match expr {
                syn::Expr::Block(block) => {
                    assert_eq!(block.block.stmts.len(), 1, "alias must only delegate");
                    match block.block.stmts.last().unwrap() {
                        syn::Stmt::Expr(expr, None) => delegated(expr),
                        _ => panic!("alias must delegate"),
                    }
                }
                syn::Expr::Await(expr) => &expr.base,
                _ => panic!("alias must await its delegate"),
            }
        }
        let block = &method.block;
        let expr: syn::Expr = syn::parse2(quote!(#block)).unwrap();
        let delegate = delegated(&expr);
        method.block = parse_quote!({ #delegate });
        return quote!(#method);
    }
    let mut captures = Vec::new();
    let mut borrows = Vec::new();
    for input in method.sig.inputs.iter().skip(1) {
        let syn::FnArg::Typed(arg) = input else {
            unreachable!()
        };
        let name = &arg.pat;
        if let syn::Type::Reference(reference) = arg.ty.as_ref() {
            assert!(
                reference.mutability.is_none(),
                "mutable input needs an ownership policy"
            );
            captures.push(quote!(let #name = #name.to_owned();));
            let ty = &arg.ty;
            borrows.push(quote!(let #name: #ty = &#name;));
        }
    }
    let (body, capture_client) = replace_client({
        let block = &method.block;
        quote!(#block)
    });
    // This is deliberately checked after lowering, including adapters. A new
    // adapter cannot silently capture the wrapper and postpone its destruction.

    let capture = capture_client.then(|| quote!(let operation_client = self.client.clone();));
    let future = if fallible {
        quote!((async move #body).await)
    } else if unit {
        quote!({ (async move #body).await; Ok(()) })
    } else {
        quote!(Ok((async move #body).await))
    };
    let settlement = if matches!(&value_type, syn::Type::Path(p) if p.path.is_ident("JsValue")) {
        quote!(result.map_err(Into::into))
    } else {
        quote!(result.map(|value| { let _ = &value; #conversion }).map_err(Into::into))
    };
    method.block = parse_quote!({
        #capture
        let cancelled = self.client.operation_cancellation();
        #(#captures)*
        client_operation(cancelled, async move {
            #(#borrows)*
            let result: Result<#value_type, crate::errors::BridgeError> = #future;
            #settlement
        })
    });
    quote!(#method)
}

fn replace_client(tokens: TokenStream) -> (TokenStream, bool) {
    struct Capture(bool);
    impl syn::visit_mut::VisitMut for Capture {
        fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
            if let syn::Expr::Field(field) = expr
                && matches!(field.base.as_ref(), syn::Expr::Path(p) if p.path.is_ident("self"))
            {
                assert!(
                    matches!(&field.member, syn::Member::Named(i) if i == "client"),
                    "async operations may capture only CoreClient"
                );
                self.0 = true;
                *expr = parse_quote!(operation_client);
            } else {
                syn::visit_mut::visit_expr_mut(self, expr);
            }
        }
        fn visit_expr_path_mut(&mut self, expr: &mut syn::ExprPath) {
            assert!(
                !expr.path.is_ident("self"),
                "async operations cannot retain the wrapper"
            );
            syn::visit_mut::visit_expr_path_mut(self, expr);
        }
    }
    let mut block: syn::Block = syn::parse2(tokens).unwrap();
    let mut capture = Capture(false);
    syn::visit_mut::VisitMut::visit_block_mut(&mut capture, &mut block);
    (quote!(#block), capture.0)
}

fn typescript_result(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Tuple(tuple) if tuple.elems.is_empty() => "void".into(),
        syn::Type::Path(path) => {
            let segment = path.path.segments.last().unwrap();
            let name = segment.ident.to_string();
            match name.as_str() {
                "JsValue" => "any".into(),
                "String" => "string".into(),
                "bool" => "boolean".into(),
                "u32" | "f64" => "number".into(),
                "Vec" | "Option" | "Ts" => {
                    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
                        panic!("generic result")
                    };
                    let syn::GenericArgument::Type(inner) = args.args.first().unwrap() else {
                        panic!("type argument")
                    };
                    let inner = typescript_result(inner);
                    match name.as_str() {
                        "Vec" => format!("{inner}[]"),
                        "Option" => format!("{inner} | undefined"),
                        _ => inner,
                    }
                }
                "JsBinaryNode" => "BinaryNode".into(),
                "JsUsyncResponse" => "UsyncResponse".into(),
                "Uint8Array" | "ReadableStream" => name,
                _ if path.path.segments.iter().any(|s| s.ident == "result_types") => name,
                _ => panic!("unsupported async result type {}", ty.to_token_stream()),
            }
        }
        _ => panic!("unsupported async result {}", ty.to_token_stream()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_exported_class_can_add_a_borrowed_async_method() {
        struct Exports;
        impl<'ast> syn::visit::Visit<'ast> for Exports {
            fn visit_item_impl(&mut self, implementation: &'ast syn::ItemImpl) {
                if implementation
                    .attrs
                    .iter()
                    .any(|a| a.path().is_ident("wasm_bindgen"))
                {
                    for item in &implementation.items {
                        if let syn::ImplItem::Fn(method) = item {
                            assert!(
                                !(method.sig.asyncness.is_some()
                                    && method.sig.receiver().is_some()),
                                "{} must use the owned operation generator",
                                method.sig.ident
                            );
                        }
                    }
                }
                syn::visit::visit_item_impl(self, implementation);
            }
        }
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src");
        for entry in walkdir::WalkDir::new(source) {
            let entry = entry.unwrap();
            if entry.path().extension().is_some_and(|ext| ext == "rs") {
                let file =
                    syn::parse_file(&std::fs::read_to_string(entry.path()).unwrap()).unwrap();
                syn::visit::Visit::visit_file(&mut Exports, &file);
            }
        }
    }

    #[test]
    fn new_operations_copy_borrowed_inputs_without_a_method_registry() {
        let method = lower(
            quote!(
                pub async fn future_method(
                    &self,
                    text: &str,
                    bytes: &[u8],
                ) -> Result<String, crate::errors::BridgeError> {
                    self.client.online().await?.future_method(text, bytes).await
                }
            ),
            false,
            None,
        )
        .to_string();
        assert!(method.contains("text . to_owned"));
        assert!(method.contains("bytes . to_owned"));
        assert!(method.contains("Promise<string>"));
        assert!(!method.contains("async fn"));
    }

    #[test]
    #[should_panic(expected = "alias must only delegate")]
    fn aliases_cannot_silently_discard_preparation() {
        lower(
            quote!(
                pub async fn alias(&self) -> Result<(), crate::errors::BridgeError> {
                    validate_input()?;
                    self.delegate().await
                }
            ),
            true,
            None,
        );
    }

    #[test]
    #[should_panic(expected = "unsupported async result type")]
    fn unknown_result_types_fail_generation_instead_of_becoming_any() {
        lower(
            quote!(
                pub async fn new_method(&self) -> Result<UnknownType, crate::errors::BridgeError> {
                    self.client.online().await?.new_method().await
                }
            ),
            false,
            None,
        );
    }

    #[test]
    #[should_panic(expected = "async operations cannot retain the wrapper")]
    fn a_new_adapter_cannot_capture_the_wrapper() {
        lower(
            quote!(
                pub async fn bad(&self) -> Result<(), crate::errors::BridgeError> {
                    do_something(self).await
                }
            ),
            false,
            None,
        );
    }

    #[test]
    #[should_panic(expected = "async operations may capture only CoreClient")]
    fn a_new_adapter_cannot_capture_an_unregistered_field() {
        lower(
            quote!(
                pub async fn bad(&self) -> Result<(), crate::errors::BridgeError> {
                    self.other_field.wait().await
                }
            ),
            false,
            None,
        );
    }
}
