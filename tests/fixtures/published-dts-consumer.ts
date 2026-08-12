/**
 * A consumer of the published declarations, compiled by
 * `tests/published-dts.test.ts`. Every assertion below fails to compile if the
 * field's type is `any` — which is what an unresolved name in a `.d.ts` becomes
 * for the `skipLibCheck: true` consumers this package actually has, so a check
 * that only asks "does it resolve" would pass on the very thing being fixed.
 *
 * One entry per shape the core writes a waproto type behind.
 */

import type {
  ArchiveUpdate,
  InboundMessage,
  MessageInfo,
  MexResponse,
  MsgSecretEntry,
  Receipt,
} from "../../dist/index.js";
import type { proto } from "../../dist/proto-types.js";

type IsAny<T> = 0 extends 1 & T ? true : false;
// `false` rather than `never` on the failing branches: `never` satisfies every
// constraint, so an assertion that fails to `never` never fails.
type Resolves<Actual, Expected> =
  IsAny<Actual> extends true ? false
  : [Expected] extends [Actual] ? true
  : false;
type Assert<T extends true> = T;

// Box<wa::sync_action_value::ArchiveChatAction>
type _Boxed = Assert<
  Resolves<
    ArchiveUpdate["action"],
    proto.SyncActionValue.IArchiveChatAction
  >
>;

// Arc<wa::Message>
type _Shared = Assert<Resolves<InboundMessage["message"], proto.IMessage>>;

// Option<wa::MessageKey>
type _Optional = Assert<
  Resolves<NonNullable<MessageInfo["comment_target"]>, proto.IMessageKey>
>;

// `pub type MessageSecret = [u8; 32]`
type _Aliased = Assert<Resolves<MsgSecretEntry["secret"], Uint8Array>>;

// serde_json::Value
type _Json = Assert<
  Resolves<NonNullable<MexResponse["data"]>, { field: number }>
>;

// The core's hand-written `impl Serialize for ReceiptType`.
type _Receipt = Assert<Resolves<Receipt["type"], "ReadSelf">>;

export type Checked = [
  _Boxed,
  _Shared,
  _Optional,
  _Aliased,
  _Json,
  _Receipt,
];
