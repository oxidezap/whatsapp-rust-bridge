/**
 * The published `WhatsAppEvent` union, pinned entry by entry: every wire name is
 * a consumer's `switch` label, and the union is generated, so a typo in the
 * table publishes a different event rather than failing to compile.
 *
 * Run: bun run build && bun test tests/event-union.test.ts
 */

import { test, expect } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const DECLARATIONS = join(ROOT, "dist", "whatsapp_rust_bridge.d.ts");

/** Wire name -> the `data` type the union declares for it. */
const PUBLISHED: Record<string, string> = {
  receipt: "Receipt",
  server_ack: "ServerAck",
  undecryptable_message: "UndecryptableMessage",
  chat_presence: "ChatPresenceUpdate",
  presence: "PresenceUpdate",
  picture_update: "PictureUpdate",
  user_about_update: "UserAboutUpdate",
  contact_updated: "ContactUpdated",
  contact_number_changed: "ContactNumberChanged",
  contact_sync_requested: "ContactSyncRequested",
  group_update: "GroupUpdate",
  contact_update: "ContactUpdate",
  push_name_update: "PushNameUpdate",
  self_push_name_updated: "SelfPushNameUpdated",
  pin_update: "PinUpdate",
  mute_update: "MuteUpdate",
  archive_update: "ArchiveUpdate",
  star_update: "StarUpdate",
  mark_chat_as_read_update: "MarkChatAsReadUpdate",
  delete_chat_update: "DeleteChatUpdate",
  clear_chat_update: "ClearChatUpdate",
  user_status_mute_update: "UserStatusMuteUpdate",
  delete_message_for_me_update: "DeleteMessageForMeUpdate",
  label_edit_update: "LabelEditUpdate",
  label_association_update: "LabelAssociationUpdate",
  offline_sync_preview: "OfflineSyncPreview",
  offline_sync_completed: "OfflineSyncCompleted",
  dirty_state: "{ dirty_type: DirtyType; timestamp?: number | null }",
  device_list_update: "DeviceListUpdate",
  identity_change: "IdentityChange",
  business_status_update: "BusinessStatusUpdate",
  temporary_ban: "TemporaryBan",
  connect_failure: "ConnectFailure",
  stream_error: "StreamError",
  disappearing_mode_changed: "DisappearingModeChanged",
  newsletter_live_update: "NewsletterLiveUpdate",
  incoming_call: "IncomingCall",
  missed_call: "MissedCall",
  call_ended_elsewhere: "CallEndedElsewhere",
  mex_notification: "MexNotification",
  pairing_code_refresh: "PairingCodeRefresh",
  pair_passkey_request: "PairPasskeyRequest",
  pair_passkey_confirmation: "PairPasskeyConfirmation",
  pair_passkey_error: "PairPasskeyError",
  app_state_sync_failed: "AppStateSyncFailed",
  contact_removed: "ContactRemoved",
  disable_link_previews_update: "DisableLinkPreviewsUpdate",
  message_label_association_update: "MessageLabelAssociationUpdate",
  quick_reply_update: "QuickReplyUpdate",
  pairing_qr_codes_exhausted: "PairingQrCodesExhausted",
  connected: "Record<string, never>",
  disconnected: "Record<string, never>",
  qr: "{ code: string; timeout: number }",
  pairing_code: "{ code: string; timeout: number }",
  pair_success: "{ id: string; lid: string; business_name: string; platform: string }",
  pair_error: "{ id: string; lid: string; business_name: string; platform: string; error: string }",
  logged_out: "{ on_connect: boolean; reason: string }",
  message: "{ message: Record<string, unknown>; info: MessageInfo & { is_view_once: boolean } }",
  notification: "{ tag: string; attrs: Record<string, string>; content?: unknown }",
  stream_replaced: "Record<string, never>",
  qr_scanned_without_multidevice: "Record<string, never>",
  client_outdated: "Record<string, never>",
  raw_node: "{ tag: string; attrs: Record<string, string>; content?: unknown }",
  history_sync: "import('./proto-types').proto.IHistorySync & { syncType: number; chunkOrder?: number; progress?: number; peerDataRequestSessionId?: string }",
  pairing_code_error: "PairingCodeError",
};

function publishedUnion(): Record<string, string> {
  const source = readFileSync(DECLARATIONS, "utf8");
  const block = source.split("export type WhatsAppEvent =\n")[1]?.split("\n;")[0];
  expect(block, "dist declares no WhatsAppEvent union").toBeDefined();

  const entries: Record<string, string> = {};
  for (const line of block!.split("\n")) {
    const match = /^\| \{ type: '([a-z_]+)'; data: (.*) \}$/.exec(line);
    expect(match, `unparsed union entry: ${line}`).not.toBeNull();
    entries[match![1]] = match![2];
  }
  return entries;
}

test("every published event keeps its wire name and payload type", () => {
  expect(
    existsSync(DECLARATIONS),
    "dist/whatsapp_rust_bridge.d.ts is absent \u2014 run `bun run build` first",
  ).toBe(true);

  expect(publishedUnion()).toEqual(PUBLISHED);
});
