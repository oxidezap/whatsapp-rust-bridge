use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, parse_quote};

#[derive(Clone, Copy)]
enum Policy {
    Required,
    OptionalStrict,
    OptionalTolerant,
}
struct Callback {
    field: &'static str,
    method: &'static str,
    policy: Policy,
    parameters: &'static str,
    result: &'static str,
    doc: &'static str,
}
struct Adapter {
    name: Ident,
    macro_name: Ident,
    public: bool,
    borrowed: bool,
    constructor_public: bool,
    prefix: &'static str,
    interface: &'static str,
    interface_doc: &'static str,
    callbacks: Vec<Callback>,
    extra_fields: TokenStream,
    extra_values: TokenStream,
}
fn callback(
    field: &'static str,
    method: &'static str,
    parameters: &'static str,
    result: &'static str,
) -> Callback {
    Callback {
        field,
        method,
        policy: Policy::Required,
        parameters,
        result,
        doc: "",
    }
}
fn catalog() -> Vec<Adapter> {
    vec![
        Adapter {
            name: parse_quote!(JsCacheStoreAdapter),
            macro_name: parse_quote!(generated_cache_callbacks),
            public: true,
            borrowed: true,
            constructor_public: true,
            prefix: "cacheStore",
            interface: "JsCacheStore",
            interface_doc: "/** Custom cache backend. */",
            callbacks: vec![
                callback(
                    "get_fn",
                    "get",
                    "namespace: string, key: string",
                    "Promise<Uint8Array | null>",
                ),
                callback(
                    "set_fn",
                    "set",
                    "namespace: string, key: string, value: Uint8Array, ttlSecs?: number",
                    "Promise<void>",
                ),
                callback(
                    "delete_fn",
                    "delete",
                    "namespace: string, key: string",
                    "Promise<void>",
                ),
                callback("clear_fn", "clear", "namespace: string", "Promise<void>"),
            ],
            extra_fields: quote!(),
            extra_values: quote!(),
        },
        Adapter {
            name: parse_quote!(JsCryptoAdapter),
            macro_name: parse_quote!(generated_crypto_callbacks),
            public: true,
            borrowed: true,
            constructor_public: true,
            prefix: "crypto",
            interface: "JsCryptoCallbacks",
            interface_doc: "/**\n * Native crypto callbacks installed via `initWasmEngine` for large AES operations.\n * Supply an implementation backed by the host platform's native crypto API.\n */",
            callbacks: vec![
                callback(
                    "aes_cbc_encrypt",
                    "aesCbc256Encrypt",
                    "key: Uint8Array, iv: Uint8Array, plaintext: Uint8Array",
                    "Uint8Array",
                ),
                callback(
                    "aes_cbc_decrypt",
                    "aesCbc256Decrypt",
                    "key: Uint8Array, iv: Uint8Array, ciphertext: Uint8Array",
                    "Uint8Array",
                ),
                callback(
                    "aes_gcm_encrypt",
                    "aesGcm256Encrypt",
                    "key: Uint8Array, nonce: Uint8Array, aad: Uint8Array, plaintext: Uint8Array",
                    "Uint8Array",
                ),
                callback(
                    "aes_gcm_decrypt",
                    "aesGcm256Decrypt",
                    "key: Uint8Array, nonce: Uint8Array, aad: Uint8Array, ciphertextWithTag: Uint8Array",
                    "Uint8Array",
                ),
                callback(
                    "hmac_sha256",
                    "hmacSha256",
                    "key: Uint8Array, data: Uint8Array",
                    "Uint8Array",
                ),
                Callback {
                    field: "hmac_sha256_two_part",
                    method: "hmacSha256TwoPart",
                    policy: Policy::OptionalStrict,
                    parameters: "key: Uint8Array, first: Uint8Array, second: Uint8Array",
                    result: "Uint8Array",
                    doc: "",
                },
            ],
            extra_fields: quote!(),
            extra_values: quote!(),
        },
        Adapter {
            name: parse_quote!(JsHttpClientAdapter),
            macro_name: parse_quote!(generated_http_callbacks),
            public: true,
            borrowed: false,
            constructor_public: true,
            prefix: "httpClient",
            interface: "JsHttpClientConfig",
            interface_doc: "/**\n * JS HTTP client callbacks. Implement using fetch() or any HTTP library.\n */",
            callbacks: vec![callback(
                "execute_fn",
                "execute",
                "url: string, method: string, headers: Record<string, string>, body: Uint8Array | null",
                "Promise<{ statusCode: number; body: Uint8Array }>",
            )],
            extra_fields: quote!(_js_obj: wasm_bindgen::JsValue,),
            extra_values: quote!(_js_obj: obj,),
        },
        Adapter {
            name: parse_quote!(RawTransportCallbacks),
            macro_name: parse_quote!(generated_transport_callbacks),
            public: false,
            borrowed: false,
            constructor_public: false,
            prefix: "transport",
            interface: "JsTransportCallbacks",
            interface_doc: "/**\n * Every Promise handed back to the bridge must settle, including on failure:\n * reject rather than leaving it pending. The bridge awaits them through a\n * wasm-bindgen `JsFuture`; an overlapping teardown cancels only the wait that\n * would otherwise form a callback/teardown cycle.\n */",
            callbacks: vec![
                callback(
                    "connect_fn",
                    "connect",
                    "handle: JsTransportHandle",
                    "void | Promise<void>",
                ),
                callback(
                    "send_fn",
                    "send",
                    "data: Uint8Array",
                    "void | Promise<void>",
                ),
                Callback {
                    field: "send_borrowed_fn",
                    method: "sendBorrowed",
                    policy: Policy::OptionalTolerant,
                    parameters: "data: Uint8Array",
                    result: "void",
                    doc: "",
                },
                callback("disconnect_fn", "disconnect", "", "void | Promise<void>"),
            ],
            extra_fields: quote!(send_borrowed_checked: std::cell::Cell<bool>, callback_state: Arc<HostCallbackState>, _js_obj: wasm_bindgen::JsValue,),
            extra_values: quote!(send_borrowed_checked:std::cell::Cell::new(false),callback_state:Arc::new(HostCallbackState::new()),_js_obj:obj,),
        },
    ]
}
fn typescript(adapter: &Adapter) -> String {
    let mut output = format!(
        "{}\nexport interface {} {{\n",
        adapter.interface_doc, adapter.interface
    );
    for callback in &adapter.callbacks {
        if !callback.doc.is_empty() {
            output.push_str(callback.doc);
            output.push('\n');
        }
        output.push_str(&format!(
            "    {}{}({}): {};\n",
            callback.method,
            if matches!(callback.policy, Policy::Required) {
                ""
            } else {
                "?"
            },
            callback.parameters,
            callback.result
        ));
    }
    output.push_str("}\n");
    if adapter.interface == "JsTransportCallbacks" {
        output.insert_str(0, &transport_handle_typescript());
    }
    output
}
fn transport_handle_typescript() -> String {
    let mut output = String::from(
        "/**\n * JS transport callbacks for WebSocket management.\n *\n * Passed to `createWhatsAppClient` as the transport config.\n *\n * `connect(handle)` is called when the client needs a connection:\n *   - Create a WebSocket\n *   - Wire ws.onopen → handle.onConnected()\n *   - Wire ws.onmessage → handle.onData(data)\n *   - Wire ws.onclose → handle.onDisconnected()\n *\n * `send(data)` sends raw bytes over the active WebSocket.\n * `sendBorrowed(data)`, when provided, is the zero-copy fast path. The view is\n * borrowed from WASM memory and is valid ONLY for the synchronous duration of\n * the call: implementations MUST consume/copy it before returning and MUST NOT\n * retain it, re-enter WASM, or return a Promise. The mandatory `send` method is\n * kept as the safe copying fallback for every other transport.\n * `disconnect()` closes the WebSocket.\n */",
    );
    output.push_str("\nexport interface JsTransportHandle {\n");
    for (method, parameters, result) in [
        ("onConnected", "", "void"),
        ("onData", "data: Uint8Array", "void"),
        ("onDisconnected", "", "void"),
    ] {
        output.push_str(&format!("    {method}({parameters}): {result};\n"));
    }
    output.push_str("}\n");
    output
}

pub fn emit() -> TokenStream {
    let mut output = TokenStream::new();
    for adapter in catalog() {
        let typescript = typescript(&adapter);
        let ts_constant = format_ident!("GENERATED_{}_CALLBACKS_TS", adapter.prefix.to_uppercase());
        let name = adapter.name;
        let macro_name = adapter.macro_name;
        let visibility = adapter.public.then(|| quote!(pub));
        let constructor_visibility = adapter.constructor_public.then(|| quote!(pub));
        let argument = if adapter.borrowed {
            quote!(&wasm_bindgen::JsValue)
        } else {
            quote!(wasm_bindgen::JsValue)
        };
        let object = if adapter.borrowed {
            quote!(obj)
        } else {
            quote!(&obj)
        };
        let mut fields = Vec::new();
        let mut extraction = Vec::new();
        let mut values = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for callback in adapter.callbacks {
            assert!(names.insert(callback.method), "duplicate host callback");
            let field = format_ident!("{}", callback.field);
            let method = callback.method;
            let message = format!("{}.{} must be a function", adapter.prefix, method);
            let optional = !matches!(callback.policy, Policy::Required);
            let ty = if optional {
                quote!(Option<js_sys::Function>)
            } else {
                quote!(js_sys::Function)
            };
            fields.push(quote!(#field:#ty,));
            values.push(quote!(#field,));
            let value = match callback.policy {
                Policy::Required => {
                    quote!(js_sys::Reflect::get(#object,&wasm_bindgen::JsValue::from_str(#method))?.dyn_into::<js_sys::Function>().map_err(|_|wasm_bindgen::JsValue::from_str(#message))?)
                }
                Policy::OptionalStrict => {
                    quote!({let value=js_sys::Reflect::get(#object,&wasm_bindgen::JsValue::from_str(#method))?;if value.is_null()||value.is_undefined(){None}else{Some(value.dyn_into::<js_sys::Function>().map_err(|_|wasm_bindgen::JsValue::from_str(#message))?)}})
                }
                Policy::OptionalTolerant => {
                    quote!(js_sys::Reflect::get(#object,&wasm_bindgen::JsValue::from_str(#method)).ok().and_then(|value|value.dyn_into::<js_sys::Function>().ok()))
                }
            };
            extraction.push(quote!(let #field=#value;));
        }
        let extra_fields = adapter.extra_fields;
        let extra_values = adapter.extra_values;
        output.extend(quote!(macro_rules! #macro_name {()=>{
            #[wasm_bindgen(typescript_custom_section)]
            const #ts_constant: &str = #typescript;
            #visibility struct #name {#(#fields)*#extra_fields}
            impl #name {#constructor_visibility fn from_js(obj:#argument)->Result<Self,wasm_bindgen::JsValue>{#(#extraction)*Ok(Self{#(#values)*#extra_values})}}
        };}));
    }
    output
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emits_distinct_adapter_macros() {
        syn::parse2::<syn::File>(emit()).unwrap();
        assert_eq!(catalog().len(), 4);
    }
}
