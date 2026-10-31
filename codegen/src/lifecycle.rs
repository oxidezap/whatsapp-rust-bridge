use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, parse_quote};

enum Publication {
    BeforeFuture,
    OnOwner,
}
enum Completion {
    Resolve,
    RequireMatchingKind,
}
enum Disposition {
    Abort,
    Detach,
    AbortPendingConnection,
    FinishEstablishedConnection,
}
struct Task {
    handle: Ident,
    disposition: Disposition,
}
struct Teardown {
    method: Ident,
    core_action: Ident,
    kind: Ident,
    publication: Publication,
    completion: Completion,
    captures: Vec<Ident>,
    tasks: Vec<Task>,
}
fn catalog() -> Vec<Teardown> {
    vec![
        Teardown {
            method: parse_quote!(prepare_disconnect),
            core_action: parse_quote!(shutdown),
            kind: parse_quote!(TEARDOWN_DISCONNECT),
            publication: Publication::OnOwner,
            completion: Completion::Resolve,
            captures: vec![
                parse_quote!(transport_callback_state),
                parse_quote!(teardown_kind),
                parse_quote!(saver_handle),
                parse_quote!(connection_handle),
                parse_quote!(connection_established),
                parse_quote!(sync_worker_handle),
            ],
            tasks: vec![
                Task {
                    handle: parse_quote!(saver_handle),
                    disposition: Disposition::Abort,
                },
                Task {
                    handle: parse_quote!(connection_handle),
                    disposition: Disposition::AbortPendingConnection,
                },
                Task {
                    handle: parse_quote!(sync_worker_handle),
                    disposition: Disposition::Abort,
                },
            ],
        },
        Teardown {
            method: parse_quote!(prepare_logout),
            core_action: parse_quote!(logout),
            kind: parse_quote!(TEARDOWN_LOGOUT),
            publication: Publication::BeforeFuture,
            completion: Completion::RequireMatchingKind,
            captures: vec![
                parse_quote!(transport_callback_state),
                parse_quote!(teardown_kind),
                parse_quote!(saver_handle),
                parse_quote!(sync_worker_handle),
                parse_quote!(run_handle),
                parse_quote!(connection_handle),
                parse_quote!(connection_established),
            ],
            tasks: vec![
                Task {
                    handle: parse_quote!(saver_handle),
                    disposition: Disposition::Abort,
                },
                Task {
                    handle: parse_quote!(sync_worker_handle),
                    disposition: Disposition::Abort,
                },
                Task {
                    handle: parse_quote!(run_handle),
                    disposition: Disposition::Detach,
                },
                Task {
                    handle: parse_quote!(connection_handle),
                    disposition: Disposition::FinishEstablishedConnection,
                },
            ],
        },
    ]
}
impl Teardown {
    fn emit(&self) -> TokenStream {
        let method = &self.method;
        let action = &self.core_action;
        let kind = &self.kind;
        let publication = quote!(teardown_kind.store(#kind,std::sync::atomic::Ordering::Release););
        let (gate, claim, owner, assert_owner) = match self.publication {
            Publication::OnOwner => (
                quote!(let admission=self.teardown_gate.admit();),
                quote!(),
                publication.clone(),
                quote!(),
            ),
            Publication::BeforeFuture => (
                quote!(let teardown_gate=self.teardown_gate.clone();let admission=teardown_gate.admit();),
                quote!(if matches!(&admission,TeardownAdmission::Owner) { self.teardown_kind.store(#kind,std::sync::atomic::Ordering::Release); }),
                quote!(),
                quote!(debug_assert_eq!(teardown_kind.load(std::sync::atomic::Ordering::Acquire),#kind);),
            ),
        };
        let clone_gate = matches!(self.publication, Publication::OnOwner)
            .then(|| quote!(let teardown_gate=self.teardown_gate.clone();));
        let completion = match self.completion {
            Completion::Resolve => quote!(Ok(JsValue::UNDEFINED)),
            Completion::RequireMatchingKind => {
                quote!(if teardown_kind.load(std::sync::atomic::Ordering::Acquire)==#kind {Ok(JsValue::UNDEFINED)} else {Err(bridge_error_to_js_value(&crate::errors::BridgeError::NotConnected))})
            }
        };
        let captures = self
            .captures
            .iter()
            .map(|field| quote!(let #field=self.#field.clone();));
        let tasks=self.tasks.iter().map(|task|{let handle=&task.handle;assert!(self.captures.contains(handle),"task handle must be captured owned");match task.disposition {
            Disposition::Abort=>quote!(abort_teardown_task(&#handle);),
            Disposition::Detach=>quote!(detach_teardown_task(&#handle);),
            Disposition::AbortPendingConnection=>quote!(if !connection_established.load(std::sync::atomic::Ordering::Acquire) {abort_teardown_task(&#handle);}),
            Disposition::FinishEstablishedConnection=>quote!(if connection_established.load(std::sync::atomic::Ordering::Acquire) {detach_teardown_task(&#handle);}else{abort_teardown_task(&#handle);}),
        }});
        quote!(fn #method(&self)->impl std::future::Future<Output=Result<JsValue,JsValue>> + use<> + 'static {
            #gate
            if matches!(&admission,TeardownAdmission::Waiting(_)) {self.transport_callback_state.cancel_disconnect_wait();}
            #claim
            let client=self.client.unwaited(Unwaited::ThisSocket).clone();
            #clone_gate
            #(#captures)*
            async move {
                match admission {
                    TeardownAdmission::Owner=>{#owner}
                    TeardownAdmission::Waiting(waiter)=>{let _=waiter.await;return #completion;}
                    TeardownAdmission::Complete=>return #completion,
                }
                #assert_owner
                // The JS teardown contract resolves void; core reports are not exported here.
                let _ = client.#action().await;
                // A pending handshake owns a stream the core has not published yet.
                // Close it after terminal shutdown so retained JS handles cannot keep run() alive.
                transport_callback_state.close_event_streams();
                #(#tasks)*
                teardown_gate.complete();
                Ok(JsValue::UNDEFINED)
            }
        })
    }
}
pub fn emit() -> TokenStream {
    let methods = catalog().iter().map(Teardown::emit).collect::<Vec<_>>();
    quote!(impl WasmWhatsAppClient {#(#methods)*})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_teardown_policies_emit_concrete_rust() {
        syn::parse2::<syn::File>(emit()).unwrap();
    }
    #[test]
    fn uncaptured_task_handle_is_rejected() {
        let mut data = catalog();
        data[0].captures.clear();
        assert!(std::panic::catch_unwind(|| data[0].emit()).is_err());
    }
}
