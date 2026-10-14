use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, parse_quote};

enum DispatchGuard {
    Always,
    IncludeLinkedGroups,
}

struct ParticipantRoute {
    action: Ident,
    guard: DispatchGuard,
    accessor: Ident,
    method: Ident,
}

fn participant_routes() -> Vec<ParticipantRoute> {
    vec![
        ParticipantRoute {
            action: parse_quote!(Add),
            guard: DispatchGuard::Always,
            accessor: parse_quote!(groups),
            method: parse_quote!(add_participants),
        },
        ParticipantRoute {
            action: parse_quote!(Remove),
            guard: DispatchGuard::IncludeLinkedGroups,
            accessor: parse_quote!(community),
            method: parse_quote!(remove_participants),
        },
        ParticipantRoute {
            action: parse_quote!(Remove),
            guard: DispatchGuard::Always,
            accessor: parse_quote!(groups),
            method: parse_quote!(remove_participants),
        },
        ParticipantRoute {
            action: parse_quote!(Promote),
            guard: DispatchGuard::Always,
            accessor: parse_quote!(groups),
            method: parse_quote!(promote_participants),
        },
        ParticipantRoute {
            action: parse_quote!(Demote),
            guard: DispatchGuard::Always,
            accessor: parse_quote!(groups),
            method: parse_quote!(demote_participants),
        },
    ]
}

struct RejectedAction {
    action: Ident,
    field: &'static str,
    reason: &'static str,
}

fn participant_rejection() -> RejectedAction {
    RejectedAction {
        action: parse_quote!(Modify),
        field: "action",
        reason: "modify represents a received participant identity change and cannot be sent",
    }
}

enum Argument {
    Destination,
    Message,
    Recipients,
    Options(OptionKind),
}

enum OptionKind {
    Message,
    Status,
}

enum Request {
    MessageEnvelope,
    StatusRaw,
}

enum Reply {
    MessageId,
}

struct SendAdapter {
    name: Ident,
    arguments: Vec<Argument>,
    request: Request,
    reply: Reply,
}

fn send_adapters() -> Vec<SendAdapter> {
    vec![
        SendAdapter {
            name: parse_quote!(send_message_with_options),
            arguments: vec![
                Argument::Destination,
                Argument::Message,
                Argument::Options(OptionKind::Message),
            ],
            request: Request::MessageEnvelope,
            reply: Reply::MessageId,
        },
        SendAdapter {
            name: parse_quote!(send_status_message_with_options),
            arguments: vec![
                Argument::Message,
                Argument::Recipients,
                Argument::Options(OptionKind::Status),
            ],
            request: Request::StatusRaw,
            reply: Reply::MessageId,
        },
    ]
}

fn emit_participants() -> TokenStream {
    let routes = participant_routes().into_iter().map(|route| {
        let ParticipantRoute {
            action,
            guard,
            accessor,
            method,
        } = route;
        let guard = match guard {
            DispatchGuard::Always => TokenStream::new(),
            DispatchGuard::IncludeLinkedGroups => quote!(if include_linked_groups_on_remove),
        };
        quote! {
            GroupParticipantAction::#action #guard => {
                client.#accessor().#method(&group_jid, &participant_jids).await?
            }
        }
    });
    let RejectedAction {
        action,
        field,
        reason,
    } = participant_rejection();
    quote! {
        #[cfg(feature = "client-groups")]
        async fn participants_update(
            client: &whatsapp_rust::Client,
            group_jid: wacore_binary::jid::Jid,
            participant_jids: Vec<wacore_binary::jid::Jid>,
            action: crate::result_types::GroupParticipantAction,
            include_linked_groups_on_remove: bool,
        ) -> Result<Vec<Ts<crate::result_types::ParticipantChangeResult>>, crate::errors::BridgeError> {
            use crate::result_types::GroupParticipantAction;
            let responses = match action {
                #(#routes,)*
                // Input validation rejects this before admission too. Keep a
                // structured error if another internal caller bypasses it.
                GroupParticipantAction::#action => {
                    return Err(crate::errors::BridgeError::InvalidArgument {
                        field: #field.into(),
                        reason: #reason.into(),
                    });
                }
            };
            to_ts_vec(
                responses.iter().map(crate::result_types::project_participant_change).collect(),
            )
        }
    }
}

fn emit_send(adapter: SendAdapter) -> TokenStream {
    let SendAdapter {
        name,
        arguments,
        request,
        reply,
    } = adapter;
    let arguments = arguments.into_iter().map(|argument| match argument {
        Argument::Destination => quote!(to: Jid),
        Argument::Message => quote!(msg: waproto::whatsapp::Message),
        Argument::Recipients => quote!(recipients: Vec<wacore_binary::jid::Jid>),
        Argument::Options(OptionKind::Message) => quote!(options: whatsapp_rust::SendOptions),
        Argument::Options(OptionKind::Status) => quote!(options: whatsapp_rust::StatusSendOptions),
    });
    let (prepare, call) = match request {
        Request::MessageEnvelope => (
            quote!(let request = whatsapp_rust::SendRequest::new(&to, msg).with_options(options);),
            quote!(client.send(request)),
        ),
        Request::StatusRaw => (
            TokenStream::new(),
            quote!(client.status().send_raw(msg, &recipients, options)),
        ),
    };
    let reply = match reply {
        Reply::MessageId => quote!(result.message_id.into_string()),
    };
    quote! {
        async fn #name(
            client: &whatsapp_rust::Client,
            #(#arguments,)*
        ) -> Result<String, crate::errors::BridgeError> {
            #prepare
            let result = #call.await?;
            Ok(#reply)
        }
    }
}

/// Compile bridge routing and request policies into ordinary typed calls.
/// No policy table or interpreter is retained in the WASM artifact.
pub fn emit() -> TokenStream {
    let participants = emit_participants();
    let sends = send_adapters().into_iter().map(emit_send);
    quote! {
        #participants
        #(#sends)*
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn participant_remove_routes_linked_groups_before_ordinary_groups() {
        let routes = participant_routes();
        let removes: Vec<_> = routes
            .iter()
            .filter(|route| route.action == "Remove")
            .collect();
        assert_eq!(removes.len(), 2);
        assert!(matches!(
            removes[0].guard,
            DispatchGuard::IncludeLinkedGroups
        ));
        assert_eq!(removes[0].accessor, "community");
        assert!(matches!(removes[1].guard, DispatchGuard::Always));
        assert_eq!(removes[1].accessor, "groups");
    }

    #[test]
    fn adapters_emit_parseable_typed_functions() {
        let file: syn::File = syn::parse2(emit()).unwrap();
        assert_eq!(file.items.len(), 3);
        let names: Vec<_> = file
            .items
            .into_iter()
            .map(|item| match item {
                syn::Item::Fn(function) => function.sig.ident.to_string(),
                _ => panic!("call adapters must emit functions"),
            })
            .collect();
        assert_eq!(
            names,
            [
                "participants_update",
                "send_message_with_options",
                "send_status_message_with_options",
            ]
        );
    }
}
