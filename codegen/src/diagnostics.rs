use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, parse_quote};

enum Source {
    Present,
    Counter(Ident),
    Method(Ident),
}

struct Field {
    name: Ident,
    source: Source,
}

fn fields() -> Vec<Field> {
    vec![
        Field {
            name: parse_quote!(enabled),
            source: Source::Present,
        },
        Field {
            name: parse_quote!(allocated_bytes),
            source: Source::Counter(parse_quote!(allocated_bytes)),
        },
        Field {
            name: parse_quote!(freed_bytes),
            source: Source::Counter(parse_quote!(freed_bytes)),
        },
        Field {
            name: parse_quote!(allocations),
            source: Source::Counter(parse_quote!(allocations)),
        },
        Field {
            name: parse_quote!(net_bytes),
            source: Source::Method(parse_quote!(net_bytes)),
        },
    ]
}

/// One schema drives the public DTO, disabled defaults and core projection.
/// Counter precision and saturating subtraction retain their existing policy:
/// JS numbers are f64 and net bytes are calculated by the core.
pub fn emit() -> TokenStream {
    let fields = fields();
    let declarations = fields.iter().map(|field| {
        let name = &field.name;
        let ty = match field.source {
            Source::Present => quote!(bool),
            Source::Counter(_) | Source::Method(_) => quote!(f64),
        };
        quote!(pub #name: #ty)
    });
    let disabled = fields.iter().map(|field| {
        let name = &field.name;
        let default = match field.source {
            Source::Present => quote!(false),
            Source::Counter(_) | Source::Method(_) => quote!(0.0),
        };
        quote!(#name: #default)
    });
    let enabled = fields.iter().map(|field| {
        let name = &field.name;
        let value = match &field.source {
            Source::Present => quote!(true),
            Source::Counter(source) => quote!(snapshot.#source as f64),
            Source::Method(method) => quote!(snapshot.#method() as f64),
        };
        quote!(#name: #value)
    });
    quote! {
        /// Allocation churn attributed by whatsapp-rust's own `AllocMeter` to tasks
        /// spawned for this client. Available in diagnostics builds only.
        #[derive(Serialize, Tsify)]
        #[serde(rename_all = "camelCase")]
        pub struct CoreAllocationSnapshotResult {
            #(#declarations,)*
        }

        pub(crate) fn project_core_allocation_snapshot(
            meter: Option<&whatsapp_rust::wacore::stats::AllocMeter>,
        ) -> CoreAllocationSnapshotResult {
            let Some(meter) = meter else {
                return CoreAllocationSnapshotResult { #(#disabled,)* };
            };
            let snapshot = meter.snapshot();
            CoreAllocationSnapshotResult { #(#enabled,)* }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_schema_keeps_core_calculation_and_disabled_defaults() {
        let generated = emit();
        syn::parse2::<syn::File>(generated.clone()).unwrap();
        let source = generated.to_string();
        assert!(source.contains("snapshot . net_bytes () as f64"));
        assert!(!source.contains("saturating_sub"));
        for field in fields() {
            let name = field.name.to_string();
            let default = match field.source {
                Source::Present => "false",
                Source::Counter(_) | Source::Method(_) => "0.0",
            };
            assert!(source.contains(&format!("{name} : {default}")));
        }
    }
}
