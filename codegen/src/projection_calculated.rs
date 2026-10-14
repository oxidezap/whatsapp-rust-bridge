use super::*;
pub(super) fn descriptors() -> Vec<Descriptor> {
    use Codec::*;
    vec![
        Descriptor {
            source: parse_quote!(whatsapp_rust::client::ResourceReport),
            target: parse_quote!(MemoryDiagnosticsResult),
            function: parse_quote!(project_memory_diagnostics),
            feature: "",
            doc: "Result from `getMemoryDiagnostics`.",
            ownership: Ownership::Borrowed,
            fields: vec![
                field_path(
                    field("group_cache", parse_quote!(f64), false, NumberF64),
                    "client.group_cache.entries",
                ),
                field_path(
                    field("group_cache_bytes", parse_quote!(f64), false, NumberF64),
                    "client.group_cache.bytes",
                ),
                field_path(
                    field("device_registry_cache", parse_quote!(f64), false, NumberF64),
                    "client.device_registry_cache.entries",
                ),
                field_path(
                    field(
                        "device_registry_cache_bytes",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.device_registry_cache.bytes",
                ),
                field_path(
                    field(
                        "sender_key_device_cache",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.sender_key_device_cache.entries",
                ),
                field_path(
                    field(
                        "sender_key_device_cache_bytes",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.sender_key_device_cache.bytes",
                ),
                field_path(
                    field("group_devices_memo", parse_quote!(f64), false, NumberF64),
                    "client.group_devices_memo.entries",
                ),
                field_path(
                    field(
                        "group_devices_memo_bytes",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.group_devices_memo.bytes",
                ),
                field_path(
                    field("lid_pn_lid_entries", parse_quote!(f64), false, NumberF64),
                    "client.lid_pn_lid_entries.entries",
                ),
                field_path(
                    field("lid_pn_lid_bytes", parse_quote!(f64), false, NumberF64),
                    "client.lid_pn_lid_entries.bytes",
                ),
                field_path(
                    field("lid_pn_pn_entries", parse_quote!(f64), false, NumberF64),
                    "client.lid_pn_pn_entries.entries",
                ),
                field_path(
                    field("lid_pn_pn_bytes", parse_quote!(f64), false, NumberF64),
                    "client.lid_pn_pn_entries.bytes",
                ),
                field_path(
                    field("recent_messages", parse_quote!(f64), false, NumberF64),
                    "client.recent_messages.entries",
                ),
                field_path(
                    field("recent_messages_bytes", parse_quote!(f64), false, NumberF64),
                    "client.recent_messages.bytes",
                ),
                field_path(
                    field("message_retry_counts", parse_quote!(f64), false, NumberF64),
                    "client.message_retry_counts",
                ),
                field_path(
                    field(
                        "undecryptable_dispatched",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.undecryptable_dispatched",
                ),
                field_path(
                    field("pdo_pending_requests", parse_quote!(f64), false, NumberF64),
                    "client.pdo_pending_requests",
                ),
                field_path(
                    field("pdo_requested", parse_quote!(f64), false, NumberF64),
                    "client.pdo_requested",
                ),
                field_path(
                    field("session_locks", parse_quote!(f64), false, NumberF64),
                    "client.session_locks",
                ),
                field_path(
                    field("chat_lanes", parse_quote!(f64), false, NumberF64),
                    "client.chat_lanes",
                ),
                field_path(
                    field(
                        "group_distribution_locks",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.group_distribution_locks",
                ),
                field_path(
                    field(
                        "group_distribution_lock_evictions",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.group_distribution_lock_evictions",
                ),
                field_path(
                    field(
                        "group_distribution_lock_eviction_blocks",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.group_distribution_lock_eviction_blocks",
                ),
                field_path(
                    field(
                        "resend_rate_limiter_chats",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.resend_rate_limiter_chats",
                ),
                field_path(
                    field("response_waiters", parse_quote!(f64), false, NumberF64),
                    "client.response_waiters",
                ),
                field_path(
                    field("node_waiters", parse_quote!(f64), false, NumberF64),
                    "client.node_waiters",
                ),
                field_path(
                    field("pending_retries", parse_quote!(f64), false, NumberF64),
                    "client.pending_retries",
                ),
                field_path(
                    field(
                        "presence_subscriptions",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.presence_subscriptions",
                ),
                field_path(
                    field(
                        "app_state_key_requests",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.app_state_key_requests",
                ),
                field_path(
                    field("app_state_syncing", parse_quote!(f64), false, NumberF64),
                    "client.app_state_syncing",
                ),
                field_path(
                    field("signal_cache_sessions", parse_quote!(f64), false, NumberF64),
                    "client.signal_sessions.entries",
                ),
                field_path(
                    field(
                        "signal_cache_sessions_bytes",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.signal_sessions.bytes",
                ),
                field_path(
                    field(
                        "signal_cache_identities",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.signal_identities.entries",
                ),
                field_path(
                    field(
                        "signal_cache_identities_bytes",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.signal_identities.bytes",
                ),
                field_path(
                    field(
                        "signal_cache_sender_keys",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.signal_sender_keys.entries",
                ),
                field_path(
                    field(
                        "signal_cache_sender_keys_bytes",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.signal_sender_keys.bytes",
                ),
                field_path(
                    field("history_sync_tasks", parse_quote!(f64), false, NumberF64),
                    "client.history_sync_tasks.entries",
                ),
                field_path(
                    field(
                        "history_sync_payload_bytes",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.history_sync_tasks.bytes",
                ),
                field_path(
                    field(
                        "history_sync_peak_tasks",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.history_sync_tasks_peak",
                ),
                field_path(
                    field(
                        "history_sync_peak_payload_bytes",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.history_sync_payload_bytes_peak",
                ),
                field_path(
                    field("chatstate_handlers", parse_quote!(f64), false, NumberF64),
                    "client.chatstate_handlers",
                ),
                field_path(
                    field("custom_enc_handlers", parse_quote!(f64), false, NumberF64),
                    "client.custom_enc_handlers",
                ),
                field_path(
                    field(
                        "client_estimated_bytes",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "client.total_estimated_bytes()",
                ),
                field_path(
                    field("storage_memory_bytes", parse_quote!(f64), true, NumberF64),
                    "storage.memory_bytes",
                ),
                field_path(
                    field("storage_pages", parse_quote!(f64), true, NumberF64),
                    "storage.pages",
                ),
                field_path(
                    field("storage_io_read_bytes", parse_quote!(f64), true, NumberF64),
                    "storage.io_read_bytes",
                ),
                field_path(
                    field("storage_io_write_bytes", parse_quote!(f64), true, NumberF64),
                    "storage.io_write_bytes",
                ),
                field_flatten(
                    field(
                        "transport_read_buffer_bytes",
                        parse_quote!(f64),
                        true,
                        NumberF64,
                    ),
                    "transport",
                    "read_buffer_bytes",
                ),
                field_flatten(
                    field(
                        "transport_write_buffer_bytes",
                        parse_quote!(f64),
                        true,
                        NumberF64,
                    ),
                    "transport",
                    "write_buffer_bytes",
                ),
                field_flatten(
                    field(
                        "transport_tls_state_bytes",
                        parse_quote!(f64),
                        true,
                        NumberF64,
                    ),
                    "transport",
                    "tls_state_bytes",
                ),
                field_flatten(
                    field("http_pool_connections", parse_quote!(f64), true, NumberF64),
                    "http",
                    "pool_connections",
                ),
                field_flatten(
                    field("http_pool_buffer_bytes", parse_quote!(f64), true, NumberF64),
                    "http",
                    "pool_buffer_bytes",
                ),
                field_flatten(
                    field("http_inflight_bytes", parse_quote!(f64), true, NumberF64),
                    "http",
                    "inflight_bytes",
                ),
                field_path(
                    field(
                        "resource_estimated_bytes",
                        parse_quote!(f64),
                        false,
                        NumberF64,
                    ),
                    "total_estimated_bytes()",
                ),
            ],
        },
        Descriptor {
            source: parse_quote!(whatsapp_rust::NewChatMessageCapping),
            target: parse_quote!(NewChatMessageCappingResult),
            function: parse_quote!(project_new_chat_message_capping),
            feature: "",
            doc: "Result from `fetchNewChatMessageCappingInfo`.\n\nEvery field is optional because the server omits the ones that do not apply\nto an account's tier. `remainingQuota` is the core's own derivation, present\nonly when both quota fields are.",
            ownership: Ownership::Borrowed,
            fields: vec![
                field("capping_status", parse_quote!(String), true, WireAsStr),
                field("ote_status", parse_quote!(String), true, WireAsStr),
                field("mv_status", parse_quote!(String), true, WireAsStr),
                field("total_quota", parse_quote!(f64), true, NumberF64),
                field("used_quota", parse_quote!(f64), true, NumberF64),
                field("cycle_start_timestamp", parse_quote!(f64), true, NumberF64),
                field("cycle_end_timestamp", parse_quote!(f64), true, NumberF64),
                field("server_sent_timestamp", parse_quote!(f64), true, NumberF64),
                field_path(
                    field("remaining_quota", parse_quote!(f64), true, NumberF64),
                    "remaining_quota()",
                ),
            ],
        },
        Descriptor {
            source: parse_quote!(whatsapp_rust::CreatedPoll),
            target: parse_quote!(CreatePollResult),
            function: parse_quote!(project_create_poll),
            feature: "",
            doc: "Result from `createPoll`.",
            ownership: Ownership::Borrowed,
            fields: vec![
                field_path(
                    field("message_id", parse_quote!(String), false, DisplayString),
                    "send_result().message_id",
                ),
                field_path(
                    field("message_secret", parse_quote!(Vec<u8>), false, BinaryToVec),
                    "secret().as_bytes()",
                ),
            ],
        },
        Descriptor {
            source: parse_quote!(whatsapp_rust::NewsletterMessage),
            target: parse_quote!(NewsletterMessageResult),
            function: parse_quote!(project_newsletter_message),
            feature: "client-newsletter",
            doc: "One message from `newsletterMessages`.\n\n`serverId` is the pagination cursor and the key `newsletterReactMessage`\nuses; `messageId` is what edit and revoke key on. They are different ids and\nboth cross as strings, since a `serverId` is a u64.",
            ownership: Ownership::Borrowed,
            fields: vec![
                field("message_id", parse_quote!(String), true, DisplayString),
                field("server_id", parse_quote!(String), false, DisplayString),
                field("timestamp", parse_quote!(f64), false, NumberF64),
                field("message_type", parse_quote!(String), false, WireAsStr),
                field("message_type_raw", parse_quote!(String), true, CloneValue),
                field("edit", parse_quote!(String), false, WireAsStr),
                field("is_sender", parse_quote!(bool), false, Identity),
                field("media_type", parse_quote!(String), true, WireAsStr),
                field(
                    "votes",
                    parse_quote!(Vec<NewsletterPollVoteResult>),
                    false,
                    List(Projection::NewsletterPollVote),
                ),
                field("forwards_count", parse_quote!(String), true, DisplayString),
                field("views_count", parse_quote!(String), true, DisplayString),
                field("responses_count", parse_quote!(String), true, DisplayString),
                field(
                    "original_timestamp",
                    parse_quote!(String),
                    true,
                    DisplayString,
                ),
                field(
                    "last_edit_timestamp_ms",
                    parse_quote!(String),
                    true,
                    DisplayString,
                ),
                field("poll_type", parse_quote!(String), true, WireAsStr),
                field("poll_type_raw", parse_quote!(String), true, CloneValue),
                field("content_type", parse_quote!(String), true, CloneValue),
                field("question_type", parse_quote!(String), true, WireAsStr),
                field(
                    "message_association_type",
                    parse_quote!(String),
                    true,
                    WireAsStr,
                ),
                field("is_wamo_sub", parse_quote!(bool), false, Identity),
                field(
                    "admin_profile",
                    parse_quote!(NewsletterAdminProfileResult),
                    true,
                    Nested(Projection::NewsletterAdminProfile),
                ),
                field("rcat", parse_quote!(serde_bytes::ByteBuf), true, ByteBuf),
                field_doc(
                    field(
                        "message",
                        parse_quote!(serde_bytes::ByteBuf),
                        true,
                        ProtoMessage,
                    ),
                    "The decoded protobuf, re-encoded. Absent when the stanza carried none.",
                ),
                field(
                    "reactions",
                    parse_quote!(Vec<NewsletterReactionCountResult>),
                    false,
                    List(Projection::NewsletterReactionCount),
                ),
            ],
        },
        Descriptor {
            source: parse_quote!(whatsapp_rust::NewsletterPollVote),
            target: parse_quote!(NewsletterPollVoteResult),
            function: parse_quote!(project_newsletter_poll_vote),
            feature: "client-newsletter",
            doc: "The core's u64 tally crosses exactly, just like the server-id cursor.",
            ownership: Ownership::Borrowed,
            fields: vec![
                field(
                    "option_hash",
                    parse_quote!(serde_bytes::ByteBuf),
                    false,
                    ByteBuf,
                ),
                field("count", parse_quote!(String), false, DisplayString),
            ],
        },
        Descriptor {
            source: parse_quote!(whatsapp_rust::wacore::iq::usync::IsOnWhatsAppResult),
            target: parse_quote!(IsOnWhatsAppResult),
            function: parse_quote!(project_is_on_whats_app),
            feature: "client-contacts",
            doc: "Result from `isOnWhatsApp`.\n\nMirrors the core `IsOnWhatsAppResult` so callers get the LID/PN counterpart\nand business flag from the same usync round trip — no follow-up\n`fetchUserInfo` IQ needed for the common \"check + enrich\" flow.",
            ownership: Ownership::Borrowed,
            fields: vec![
                field("jid", parse_quote!(String), false, JidOptimized),
                field("is_registered", parse_quote!(bool), false, Identity),
                field_doc(
                    field("lid", parse_quote!(String), true, JidOptimized),
                    "LID counterpart of `jid` when the input was a PN, populated from the\nusync `<lid>` attribute (or the local LID/PN cache).",
                ),
                field_doc(
                    field("pn_jid", parse_quote!(String), true, JidOptimized),
                    "PN counterpart, set when the server responds with a LID as primary JID.",
                ),
                field("is_business", parse_quote!(bool), false, Identity),
                field_doc(
                    field_flatten(
                        field("verified_name", parse_quote!(String), true, CloneValue),
                        "verified_name",
                        "name",
                    ),
                    "Verified business name from the usync `<business><verified_name>` cert, if any.",
                ),
                field_doc(
                    field("username", parse_quote!(String), true, DisplayString),
                    "Meta username, without the display-only `@` prefix. Absent when the\nserver reported none, which is also how it reports a deleted one.",
                ),
            ],
        },
        Descriptor {
            source: parse_quote!(whatsapp_rust::wacore::iq::usync::UserInfo),
            target: parse_quote!(UserInfoResult),
            function: parse_quote!(project_user_info),
            feature: "client-contacts",
            doc: "A single entry from `fetchUserInfo`.",
            ownership: Ownership::Borrowed,
            fields: vec![
                field("jid", parse_quote!(String), false, DisplayString),
                field("lid", parse_quote!(String), true, DisplayString),
                field("status", parse_quote!(String), true, CloneValue),
                field("picture_id", parse_quote!(String), true, CloneValue),
                field("is_business", parse_quote!(bool), false, Identity),
                field_doc(
                    field_flatten(
                        field("verified_name", parse_quote!(String), true, CloneValue),
                        "verified_name",
                        "name",
                    ),
                    "Verified business name from the usync `<business><verified_name>` cert, if any.",
                ),
                field_skip_empty(field_doc(
                    field("devices", parse_quote!(Vec<u16>), false, CloneValue),
                    "Device IDs from the usync `<devices>` sublist the same query returns. Empty when\nthe server returned no device list.",
                )),
                field_doc(
                    field("username", parse_quote!(String), true, DisplayString),
                    "Meta username, without the display-only `@` prefix. Absent when the\nserver reported none, which is also how it reports a deleted one.",
                ),
            ],
        },
        Descriptor {
            source: parse_quote!(whatsapp_rust::wacore::iq::contacts::ProfilePicture),
            target: parse_quote!(ProfilePictureInfo),
            function: parse_quote!(project_profile_picture_info),
            feature: "client-contacts",
            doc: "Result from `profilePictureUrl`.",
            ownership: Ownership::Borrowed,
            fields: vec![
                field("id", parse_quote!(String), false, CloneValue),
                field("url", parse_quote!(String), false, CloneValue),
                field("direct_path", parse_quote!(String), true, CloneValue),
                field("hash", parse_quote!(String), true, CloneValue),
            ],
        },
    ]
}
