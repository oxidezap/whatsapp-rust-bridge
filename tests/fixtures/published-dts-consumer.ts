/**
 * A consumer of the published declarations, compiled by
 * `tests/published-dts.test.ts`. Every assertion below fails to compile if the
 * field's type is `any` — which is what an unresolved name in a `.d.ts` becomes
 * for the `skipLibCheck: true` consumers this package actually has, so a check
 * that only asks "does it resolve" would pass on the very thing being fixed.
 *
 * One entry per shape the core writes a waproto type behind, plus the error
 * union, whose usefulness is a property of these declarations rather than of
 * anything the Rust side can assert.
 */

import type {
  ArchiveUpdate,
  BridgeError,
  InboundMessage,
  JsonValue,
  MessageInfo,
  MexResponse,
  MsgSecretEntry,
  Receipt,
  ReceiptType,
  WasmWhatsAppClient,
} from "../../dist/index.js";
import type { proto } from "../../dist/proto-types.js";

type IsAny<T> = 0 extends 1 & T ? true : false;

/**
 * Assignable both ways, so a field widening to `object` or `unknown` fails
 * rather than passing on the half that still holds. `false` rather than `never`
 * on the failing branches: `never` satisfies every constraint, so an assertion
 * that fails to `never` never fails.
 */
type Resolves<Actual, Expected> =
  IsAny<Actual> extends true ? false
  : [Expected] extends [Actual] ?
    [Actual] extends [Expected] ?
      true
    : false
  : false;
type Assert<T extends true> = T;

// Box<wa::sync_action_value::ArchiveChatAction>
type _Boxed = Assert<
  Resolves<ArchiveUpdate["action"], proto.SyncActionValue.IArchiveChatAction>
>;

// Arc<wa::Message>
type _Shared = Assert<Resolves<InboundMessage["message"], proto.IMessage>>;

// Option<wa::MessageKey>, carried beside the message rather than on its info:
// the inner proto has no slot for the threading link, and `info` is shared
// with every `<enc>` of the stanza by the time decryption knows it.
type _Optional = Assert<
  Resolves<
    InboundMessage["comment_target"],
    proto.IMessageKey | null | undefined
  >
>;

// `pub type MessageSecret = [u8; 32]`
type _Aliased = Assert<Resolves<MsgSecretEntry["secret"], Uint8Array>>;

// serde_json::Value. Not `NonNullable`: `JsonValue` carries `null` itself, so
// stripping it would compare against a narrower type than the field declares.
type _Json = Assert<
  Resolves<MexResponse["data"], JsonValue | null | undefined>
>;

// The core's hand-written `impl Serialize for ReceiptType`.
type _Receipt = Assert<Resolves<Receipt["type"], ReceiptType>>;

// The assertions above are only worth their line count if they reject what a
// regression actually looks like: `any`, which is what an unresolved name
// becomes, and a widening to `object` or `unknown`.
//
// The widening controls use `Uint8Array` rather than a proto interface on
// purpose: protobufjs declares every field optional, so `object` is assignable
// to `proto.IMessage` in both directions and no assertion can tell them apart.
/**
 * A rejected caller narrows on `kind` and then reads the delay the server
 * directed. Both halves are checked here because both are this file's doing:
 * the Rust tests pin the payload's keys, not whether the declaration lets a
 * consumer branch on one.
 */
type Rejection = Extract<BridgeError, { kind: "server" }>;
type _RejectionNarrows = Assert<Resolves<Rejection["serverCode"], number>>;
type _RejectionCarriesTheDelay = Assert<
  Resolves<Rejection["backoffSeconds"], number | undefined>
>;
type _RejectionCarriesTheClass = Assert<
  Resolves<Rejection["errorType"], string | undefined>
>;

type _RejectsAny = Assert<
  Resolves<any, proto.IMessage> extends false ? true : false
>;
type _RejectsObject = Assert<
  Resolves<object, Uint8Array> extends false ? true : false
>;
type _RejectsUnknown = Assert<
  Resolves<unknown, Uint8Array> extends false ? true : false
>;

// The edit's optional caller-supplied stanza id: a fourth parameter a caller
// may omit, so the three-argument form keeps compiling. Indexing at 3 fails
// outright while the declaration has only three parameters.
type _EditPinsAStanzaId = Assert<
  Resolves<
    Parameters<WasmWhatsAppClient["editMessageBytes"]>[3],
    string | null | undefined
  >
>;

export type Checked = [
  _Boxed,
  _RejectionNarrows,
  _RejectionCarriesTheDelay,
  _RejectionCarriesTheClass,
  _Shared,
  _Optional,
  _Aliased,
  _Json,
  _Receipt,
  _RejectsAny,
  _RejectsObject,
  _RejectsUnknown,
  _EditPinsAStanzaId,
];
