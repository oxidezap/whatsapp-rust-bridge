import {
  Message,
  WebMessageInfo,
  HistorySync,
  SyncActionData,
  ClientPayload,
  ADVSignedDeviceIdentity,
  ADVSignedKeyIndexList,
  ADVDeviceIdentity,
  ADVSignedDeviceIdentityHMAC,
  HandshakeMessage,
  SyncdRecord,
  SyncdMutation,
  SyncdMutations,
  SyncdPatch,
  SyncdSnapshot,
  ExitCode,
  SyncActionValue,
  DeviceProps,
  SenderKeyDistributionMessage,
  SenderKeyMessage,
  ServerErrorReceipt,
  CertChain,
  CertChain_NoiseCertificate,
  CertChain_NoiseCertificate_Details,
  ExternalBlobReference,
  LIDMigrationMappingSyncPayload,
  MediaRetryNotification,
  VerifiedNameCertificate,
  VerifiedNameCertificate_Details,
  Message_PollVoteMessage,
  Message_EventResponseMessage,
} from "./generated/whatsapp";

interface MessageFns<T> {
  encode(message: T, writer?: any): any;
  decode(input: Uint8Array | any, length?: number): T;
  fromPartial(obj: any): T;
}

const REGISTRY: Record<string, MessageFns<any>> = {
  "Message": Message,
  "WebMessageInfo": WebMessageInfo,
  "HistorySync": HistorySync,
  "SyncActionData": SyncActionData,
  "ClientPayload": ClientPayload,
  "AdvSignedDeviceIdentity": ADVSignedDeviceIdentity,
  "AdvSignedKeyIndexList": ADVSignedKeyIndexList,
  "AdvDeviceIdentity": ADVDeviceIdentity,
  "AdvSignedDeviceIdentityHmac": ADVSignedDeviceIdentityHMAC,
  "HandshakeMessage": HandshakeMessage,
  "SyncdRecord": SyncdRecord,
  "SyncdMutation": SyncdMutation,
  "SyncdMutations": SyncdMutations,
  "SyncdPatch": SyncdPatch,
  "SyncdSnapshot": SyncdSnapshot,
  "ExitCode": ExitCode,
  "SyncActionValue": SyncActionValue,
  "DeviceProps": DeviceProps,
  "SenderKeyDistributionMessage": SenderKeyDistributionMessage,
  "SenderKeyMessage": SenderKeyMessage,
  "ServerErrorReceipt": ServerErrorReceipt,
  "CertChain": CertChain,
  "CertChain.NoiseCertificate": CertChain_NoiseCertificate,
  "CertChain.NoiseCertificate.Details": CertChain_NoiseCertificate_Details,
  "ExternalBlobReference": ExternalBlobReference,
  "LidMigrationMappingSyncPayload": LIDMigrationMappingSyncPayload,
  "MediaRetryNotification": MediaRetryNotification,
  "VerifiedNameCertificate": VerifiedNameCertificate,
  "VerifiedNameCertificate.Details": VerifiedNameCertificate_Details,
  "Message.PollVoteMessage": Message_PollVoteMessage,
  "Message.EventResponseMessage": Message_EventResponseMessage,
};

// Star-import the generated module so any ts-proto type is resolvable by name
// without us having to register each manually. Bundled at build time (bun
// includes all imports), so the runtime cost is one extra Object.entries-style
// lookup on the cold path.
import * as gen from "./generated/whatsapp";
import {
  BinaryReader,
  InvalidUtf8CountingReader,
  UnpairedSurrogateError,
  longToBigInt,
  unpairedSurrogateIndex,
  type Long as LongObject,
} from "./proto-reader";

export { UnpairedSurrogateError };

const GENERATED_MODULE = gen as unknown as Record<string, unknown>;

function resolve(typeName: string): MessageFns<any> {
  // Hot path: the small REGISTRY of well-known top-level types above.
  const direct = REGISTRY[typeName];
  if (direct) return direct;
  // Fallback: protobufjs-style namespace path (e.g. `Message.VideoMessage`)
  // is mapped to ts-proto's flat `Message_VideoMessage` and looked up in the
  // generated module. Any wacore proto type the bridge knows about resolves
  // here, no manual registration needed.
  const flatName = typeName.replace(/\./g, "_");
  const candidate = GENERATED_MODULE[flatName];
  if (candidate && typeof candidate === "object" && "encode" in candidate) {
    return candidate as MessageFns<any>;
  }
  throw new Error(`unknown proto type: ${typeName}`);
}

// The bridge serializes protobuf 64-bit fields (int64/uint64/sfixed64/…) as
// protobufjs-style `Long` objects `{ low, high, unsigned }` (see
// `src/camel_serializer.rs`, and `decodeProto` for a value past 2^53) so
// consumers can `JSON.stringify` events without the BigInt serialization error.
// The writer in `proto-reader` takes those back on the 64-bit fields, but a
// `Long` also reaches here inside a message that embeds a decoded one — e.g.
// `contextInfo.quotedMessage` from a quoted reply (`sendMessage(..., { quoted })`).
// Normalize to precision-safe BigInt before encoding. `unsigned` is the
// discriminant (matches the serializer's own Long detection), so plain
// `{ low, high }` data objects are left untouched.
function isLongObject(v: unknown): v is LongObject {
  // `unsigned` (a boolean) is the discriminant: testing it first short-circuits
  // virtually every non-Long object in one comparison, and matches the guard in
  // camel_serializer.rs — a plain `{ low, high }` data object without `unsigned`
  // is NOT a Long and is left untouched.
  return (
    typeof v === "object" &&
    v !== null &&
    typeof (v as LongObject).unsigned === "boolean" &&
    typeof (v as LongObject).low === "number" &&
    typeof (v as LongObject).high === "number"
  );
}

// Normalize the two protobufjs inputs that ts-proto cannot encode directly:
// `Long` objects become BigInts and explicit null fields become absent fields.
// Allocation-conscious implementation:
//  - Zero allocation on the clean path: a message with neither case (the
//    common non-quoted send) is returned by reference, untouched — no GC churn.
//  - Structural sharing: only the objects/arrays ON THE PATH to a change are
//    copied (`{ ...obj }` / `slice()`); unchanged subtrees are shared by ref.
//  - Never mutates the input (the locally-echoed message keeps its Long objects).
//  - Short-circuits on primitives and `Uint8Array`/typed-array byte fields.
// Runs once per `encodeProto` — the outgoing-message path, not a hot receive loop.
function normalizeProtoInput(value: unknown): unknown {
  if (value === null) return undefined;
  if (isLongObject(value)) return longToBigInt(value);
  if (typeof value !== "object") return value;
  if (ArrayBuffer.isView(value) || value instanceof ArrayBuffer) return value;
  if (Array.isArray(value)) {
    let copy: unknown[] | undefined;
    for (let i = 0; i < value.length; i++) {
      const nv = normalizeProtoInput(value[i]);
      if (nv !== value[i]) (copy ??= value.slice())[i] = nv;
    }
    return copy ?? value;
  }
  const obj = value as Record<string, unknown>;
  const proto = Object.getPrototypeOf(obj);
  if (proto !== Object.prototype && proto !== null) {
    // protobufjs-style runtime instance: schema defaults live as enumerable
    // `null`s on the prototype, and only own properties carry set fields.
    // Serialize own fields into a fresh plain object — walking (or spreading)
    // the prototype would materialize every unset field per node, and the
    // ts-proto encoder only guards `!== undefined`, so it must never see the
    // prototype's nulls.
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(obj)) {
      const nv = normalizeProtoInput(obj[k]);
      if (nv !== undefined) out[k] = nv;
    }
    return out;
  }
  let copy: Record<string, unknown> | undefined;
  // `for...in` (not `Object.keys`) is deliberate: it visits keys WITHOUT
  // allocating a keys array, keeping the clean path zero-allocation for the
  // plain objects that reach here.
  for (const k in obj) {
    const nv = normalizeProtoInput(obj[k]);
    if (nv !== obj[k]) (copy ??= { ...obj })[k] = nv;
  }
  return copy ?? value;
}

interface SurrogateCandidate {
  path: string;
  index: number;
  codeUnit: number;
}

// Only walked once the writer has already rejected the message, so the cost of
// finding the field name is paid by the failing encode alone.
function collectUnpairedSurrogates(
  value: unknown,
  path: string,
  found: SurrogateCandidate[],
): void {
  if (typeof value === "string") {
    const index = unpairedSurrogateIndex(value);
    if (index >= 0) found.push({ path, index, codeUnit: value.charCodeAt(index) });
    return;
  }
  if (typeof value !== "object" || value === null) return;
  if (ArrayBuffer.isView(value) || value instanceof ArrayBuffer) return;
  if (Array.isArray(value)) {
    for (let index = 0; index < value.length; index++) {
      collectUnpairedSurrogates(value[index], `${path}[${index}]`, found);
    }
    return;
  }
  for (const [key, entry] of Object.entries(value)) {
    // A `map<string, _>` field encodes its keys, so they can fail too. The
    // top-level object is always the message itself, never a map.
    if (path !== "") {
      const keyIndex = unpairedSurrogateIndex(key);
      if (keyIndex >= 0) {
        found.push({ path: `${path} (map key)`, index: keyIndex, codeUnit: key.charCodeAt(keyIndex) });
      }
    }
    collectUnpairedSurrogates(entry, path === "" ? key : `${path}.${key}`, found);
  }
}

/**
 * Only the codec knows which keys it encodes, and `fromPartial` is where that
 * knowledge is reachable: it drops the rest, so a property the message does not
 * declare cannot make a declared field look ambiguous. Cold path — reached only
 * after an encode has already failed.
 */
function declaredFieldsOf(fns: MessageFns<unknown>, normalized: unknown): unknown {
  try {
    return fns.fromPartial(normalized);
  } catch {
    return normalized;
  }
}

/**
 * The writer rejects in schema order; this walk sees insertion order. Naming a
 * field is only honest when exactly one candidate matches what the writer
 * reported — two declared fields carrying the same bad code unit at the same
 * index are genuinely indistinguishable from here, and guessing between them
 * would point the caller at a value that may not be the one that failed.
 */
function unpairedSurrogatePath(
  value: unknown,
  error: UnpairedSurrogateError,
): string | undefined {
  const found: SurrogateCandidate[] = [];
  collectUnpairedSurrogates(value, "", found);
  const matches = found.filter(
    (candidate) => candidate.index === error.index && candidate.codeUnit === error.codeUnit,
  );
  return matches.length === 1 ? matches[0]!.path : undefined;
}

export function encodeProto(typeName: string, obj: unknown): Uint8Array {
  const fns = resolve(typeName);
  // ts-proto's encoder accepts partial objects directly. Calling fromPartial
  // first rebuilt the entire protobuf tree and walked every possible field
  // before the encoder immediately walked it again.
  const normalized = normalizeProtoInput(obj ?? {});
  try {
    return fns.encode(normalized).finish();
  } catch (error) {
    if (error instanceof UnpairedSurrogateError && error.path === undefined) {
      const path = unpairedSurrogatePath(declaredFieldsOf(fns, normalized), error);
      if (path !== undefined) {
        throw new UnpairedSurrogateError(error.index, error.codeUnit, path);
      }
    }
    throw error;
  }
}

/**
 * Opt-in account of what a decode had to change to hand back JavaScript
 * strings. Requesting one turns the check on; without it the decode path is
 * untouched.
 */
export interface ProtoDecodeReport {
  /** `string` fields whose wire bytes were not valid UTF-8 and got U+FFFD instead. */
  invalidUtf8Fields: number;
}

export function decodeProto(
  typeName: string,
  data: Uint8Array,
  report?: ProtoDecodeReport,
): unknown {
  const fns = resolve(typeName);
  if (report === undefined) return fns.decode(data);

  const reader = new InvalidUtf8CountingReader(data);
  const decoded = fns.decode(reader, data.length);
  report.invalidUtf8Fields = reader.invalidUtf8Fields;
  return decoded;
}

const BATCH_OFFSET_SENTINEL_COUNT = 1;

/**
 * Decode concatenated protobuf payloads with one reader and no per-entry
 * `Uint8Array.subarray()`. `offsets` must contain N + 1 monotonic positions,
 * starting at zero and ending at `data.length`.
 */
export function decodeProtoBatch(
  typeName: string,
  data: Uint8Array,
  offsets: Uint32Array,
  report?: ProtoDecodeReport,
): unknown[] {
  if (offsets.length < BATCH_OFFSET_SENTINEL_COUNT) {
    throw new RangeError("protobuf batch offsets must contain the leading sentinel");
  }
  if (offsets[0] !== 0) {
    throw new RangeError("protobuf batch offsets must start at zero");
  }

  const entryCount = offsets.length - BATCH_OFFSET_SENTINEL_COUNT;
  for (let index = 0; index < entryCount; index++) {
    if (offsets[index + 1]! < offsets[index]!) {
      throw new RangeError("protobuf batch offsets must be monotonic");
    }
  }
  if (offsets[entryCount] !== data.length) {
    throw new RangeError("protobuf batch offsets must end at the data length");
  }

  const codec = resolve(typeName);
  const reader = report === undefined ? new BinaryReader(data) : new InvalidUtf8CountingReader(data);
  const decoded = new Array<unknown>(entryCount);
  for (let index = 0; index < entryCount; index++) {
    const start = offsets[index]!;
    const end = offsets[index + 1]!;
    // The previous decoder normally leaves the reader at `start`; assigning it
    // explicitly makes each offset authoritative and keeps zero-length entries
    // deterministic.
    reader.pos = start;
    decoded[index] = codec.decode(reader, end - start);
    if (reader.pos !== end) {
      throw new RangeError("protobuf decoder did not consume the delimited entry");
    }
  }
  if (report !== undefined) {
    report.invalidUtf8Fields = (reader as InvalidUtf8CountingReader).invalidUtf8Fields;
  }
  return decoded;
}
