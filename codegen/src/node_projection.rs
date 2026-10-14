//! One BinaryNode projection algorithm, specialized for the core's owned and
//! yoke-borrowed representations. Borrowed access never materializes a Node.
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Path, Type, parse_quote};

#[derive(Clone, Copy)]
enum NodeRepresentation {
    Owned,
    Borrowed,
}
#[derive(Clone, Copy)]
enum AttributeCodec {
    OwnedText,
    BorrowedValue,
}
#[derive(Clone, Copy)]
enum Accessor {
    AsSlice,
    AsRef,
}
struct Descriptor {
    function: Ident,
    source: Type,
    content: Path,
    attribute_codec: AttributeCodec,
    bytes: Accessor,
    text: Option<Accessor>,
}
impl NodeRepresentation {
    fn descriptor(self) -> Descriptor {
        match self {
            Self::Owned => Descriptor {
                function: parse_quote!(node_to_js),
                source: parse_quote!(wacore_binary::node::Node),
                content: parse_quote!(wacore_binary::node::NodeContent),
                attribute_codec: AttributeCodec::OwnedText,
                bytes: Accessor::AsSlice,
                text: None,
            },
            Self::Borrowed => Descriptor {
                function: parse_quote!(node_ref_to_js),
                source: parse_quote!(wacore_binary::node::NodeRef<'_>),
                content: parse_quote!(wacore_binary::node::NodeContentRef),
                attribute_codec: AttributeCodec::BorrowedValue,
                bytes: Accessor::AsRef,
                text: Some(Accessor::AsRef),
            },
        }
    }
}
impl Accessor {
    fn apply(self, value: TokenStream) -> TokenStream {
        match self {
            Self::AsSlice => quote!(#value.as_slice()),
            Self::AsRef => quote!(#value.as_ref()),
        }
    }
}
impl AttributeCodec {
    fn tag(self) -> TokenStream {
        match self {
            Self::OwnedText => quote!(node.tag.as_ref().into()),
            Self::BorrowedValue => quote!(JsValue::from_str(&node.tag)),
        }
    }
    fn iterator(self) -> TokenStream {
        match self {
            Self::OwnedText => quote!(node.attrs.iter()),
            Self::BorrowedValue => quote!(node.attrs_iter()),
        }
    }
    fn key(self) -> TokenStream {
        match self {
            Self::OwnedText => quote!(key.as_ref().into()),
            Self::BorrowedValue => quote!(JsValue::from_str(key)),
        }
    }
    fn value(self) -> TokenStream {
        match self {
            Self::OwnedText => quote!({
                let text: String = value.as_str().into_owned();
                text.into()
            }),
            Self::BorrowedValue => quote!(match value {
                wacore_binary::node::ValueRef::String(text) => JsValue::from_str(text),
                wacore_binary::node::ValueRef::Jid(jid) => {
                    let mut text = String::with_capacity(jid.user.len() + 20);
                    wacore_binary::push_jid_to_string(
                        &jid.user, jid.server, jid.agent, jid.device, &mut text,
                    );
                    JsValue::from_str(&text)
                }
            }),
        }
    }
}
fn emit_representation(representation: NodeRepresentation) -> TokenStream {
    let descriptor = representation.descriptor();
    let function = descriptor.function;
    let source = descriptor.source;
    let content = descriptor.content;
    let tag = descriptor.attribute_codec.tag();
    let attributes = descriptor.attribute_codec.iterator();
    let key = descriptor.attribute_codec.key();
    let value = descriptor.attribute_codec.value();
    let bytes = descriptor.bytes.apply(quote!(bytes));
    let text = descriptor
        .text
        .map_or_else(|| quote!(text), |accessor| accessor.apply(quote!(text)));
    quote!(
        fn #function(node:&#source)->Result<JsValue,JsValue>{
            let object=js_sys::Object::new();
            js_sys::Reflect::set(&object,&"tag".into(),&#tag)?;
            let attributes=js_sys::Object::new();
            for (key,value) in #attributes {
                let value:JsValue=#value;
                js_sys::Reflect::set(&attributes,&#key,&value)?;
            }
            js_sys::Reflect::set(&object,&"attrs".into(),&attributes.into())?;
            if let Some(content)=node.content.as_ref() {
                let projected=match content {
                    #content::Nodes(children)=>{
                        let array=js_sys::Array::new_with_length(children.len() as u32);
                        for (index,child) in children.iter().enumerate(){
                            array.set(index as u32,#function(child)?);
                        }
                        array.into()
                    }
                    #content::Bytes(bytes)=>js_sys::Uint8Array::from(#bytes).into(),
                    #content::String(text)=>JsValue::from_str(#text),
                };
                js_sys::Reflect::set(&object,&"content".into(),&projected)?;
            }
            Ok(object.into())
        }
    )
}
pub fn emit() -> TokenStream {
    let owned = emit_representation(NodeRepresentation::Owned);
    let borrowed = emit_representation(NodeRepresentation::Borrowed);
    quote!(#owned #borrowed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::visit::Visit;
    #[derive(Default)]
    struct Calls {
        functions: Vec<String>,
        methods: Vec<String>,
    }
    impl<'ast> Visit<'ast> for Calls {
        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            if let syn::Expr::Path(path) = node.func.as_ref() {
                self.functions.push(quote!(#path).to_string());
            }
            syn::visit::visit_expr_call(self, node);
        }
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            self.methods.push(node.method.to_string());
            syn::visit::visit_expr_method_call(self, node);
        }
    }
    #[test]
    fn each_representation_recurses_without_converting_to_the_other() {
        for (representation, recursive, forbidden) in [
            (NodeRepresentation::Owned, "node_to_js", "node_ref_to_js"),
            (NodeRepresentation::Borrowed, "node_ref_to_js", "node_to_js"),
        ] {
            let function = syn::parse2::<syn::ItemFn>(emit_representation(representation)).unwrap();
            let mut calls = Calls::default();
            calls.visit_item_fn(&function);
            assert_eq!(
                calls
                    .functions
                    .iter()
                    .filter(|name| name.as_str() == recursive)
                    .count(),
                1
            );
            assert!(!calls.functions.iter().any(|name| name == forbidden));
            assert!(
                !calls
                    .methods
                    .iter()
                    .any(|name| name == "clone" || name == "to_owned_node")
            );
        }
    }
    #[test]
    fn borrowed_projection_keeps_jid_writer_and_borrowed_accessors() {
        let function =
            syn::parse2::<syn::ItemFn>(emit_representation(NodeRepresentation::Borrowed)).unwrap();
        let mut calls = Calls::default();
        calls.visit_item_fn(&function);
        assert!(
            calls
                .functions
                .iter()
                .any(|name| name == "wacore_binary :: push_jid_to_string")
        );
        assert!(calls.methods.iter().any(|name| name == "attrs_iter"));
        assert!(
            !calls
                .methods
                .iter()
                .any(|name| name == "into_owned" || name == "as_slice")
        );
    }
    #[test]
    fn content_is_omitted_when_absent_and_reflect_errors_propagate() {
        let file = syn::parse2::<syn::File>(emit()).unwrap();
        for item in file.items {
            let syn::Item::Fn(function) = item else {
                panic!("projection must be a concrete function");
            };
            let conditionals = function
                .block
                .stmts
                .iter()
                .filter(|statement| matches!(statement, syn::Stmt::Expr(syn::Expr::If(_), _)))
                .count();
            assert_eq!(conditionals, 1, "content presence must be decided once");
            struct Reflect {
                writes: usize,
                fallible: usize,
            }
            impl<'ast> Visit<'ast> for Reflect {
                fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
                    if let syn::Expr::Path(path) = node.func.as_ref()
                        && quote!(#path).to_string() == "js_sys :: Reflect :: set"
                    {
                        self.writes += 1;
                    }
                    syn::visit::visit_expr_call(self, node);
                }
                fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
                    if let syn::Expr::Call(call) = node.expr.as_ref()
                        && let syn::Expr::Path(path) = call.func.as_ref()
                        && quote!(#path).to_string() == "js_sys :: Reflect :: set"
                    {
                        self.fallible += 1;
                    }
                    syn::visit::visit_expr_try(self, node);
                }
            }
            let mut reflect = Reflect {
                writes: 0,
                fallible: 0,
            };
            reflect.visit_item_fn(&function);
            assert_eq!(reflect.writes, 4);
            assert_eq!(reflect.fallible, reflect.writes);
        }
    }
}
