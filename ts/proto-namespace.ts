/**
 * Auto-assembled `proto` namespace, protobufjs-API-compatible.
 *
 * Walks values exported by `./generated/whatsapp` (the ts-proto output), maps
 * the flat `Parent_Child_GrandChild` naming into a nested namespace, and wraps
 * each `MessageFns` into the `{ encode, decode,
 * fromObject, create, toObject }` surface that protobufjs-style consumers
 * expect from `WAProto.X.encode(obj).finish()` and friends.
 *
 * Why: there used to be a hand-maintained shim covering ~90 of the 1,352
 * generated types. Bots that touched anything outside the manual list
 * type-checked against `WAProto/index.d.ts` but crashed at runtime. Generating
 * the runtime here from the same ts-proto output the bridge already builds
 * eliminates that drift entirely.
 *
 * Conventions matched:
 * - `encode(obj).finish()` returns `Uint8Array` (ts-proto's `BinaryWriter`
 *   already exposes `.finish()`, so we reuse it directly).
 * - `decode(bytes)` returns the typed object with a no-op `toJSON` so
 *   `JSON.stringify` round-trips cleanly without protobufjs's bytes→base64
 *   conversion (matches baileyrs' previous shim behavior).
 * - `fromObject(obj)` / `create(obj)` accept partial input and return the same
 *   shape (no normalization — ts-proto's runtime accepts plain objects).
 * - Enums are flattened into the namespace as plain `{ NAME: value }` objects.
 * - Each top-level message is wrapped in a `Proxy` so unknown capitalized
 *   sub-properties (`proto.Message.SomeUnreleasedThing.fromObject({...})`)
 *   synthesize a passthrough at access time, preventing TypeError on bot code
 *   that compiled against an updated `.d.ts` but runs against an older runtime.
 */

import * as gen from "./generated/whatsapp";
import { encodeProto } from "./proto";
import { BinaryReader } from "./proto-reader";

// Tag the union type loosely — the generated module is huge and individual
// member types matter less than the shape we extract from each value.
type AnyExport = unknown;

interface MessageFnsLike {
  encode: (msg: any, writer?: any) => any;
  decode: (input: any, length?: number) => any;
  fromPartial?: (obj: any) => any;
  create?: (obj: any) => any;
}

const isMessageFns = (v: AnyExport): v is MessageFnsLike =>
  typeof v === "object" &&
  v !== null &&
  typeof (v as MessageFnsLike).encode === "function" &&
  typeof (v as MessageFnsLike).decode === "function";

// Enums in ts-proto come out as plain objects whose values are numbers (and
// reverse-mappings for numeric enums). We treat any non-MessageFns object
// export as an enum — there's nothing else generated at this layer.
const isEnumLike = (v: AnyExport): v is Record<string, number | string> =>
  typeof v === "object" && v !== null && !isMessageFns(v);

function toJSONIdentity(this: unknown): unknown {
  return this;
}

const attachToJSONIdentity = (obj: unknown): void => {
  if (obj && typeof obj === "object" && !("toJSON" in obj)) {
    Object.defineProperty(obj, "toJSON", {
      value: toJSONIdentity,
      enumerable: false,
      writable: true,
      configurable: true,
    });
  }
};

const wrapMessage = (typeName: string, fns: MessageFnsLike) => ({
  encode(obj: any) {
    return {
      finish(): Uint8Array {
        return encodeProto(typeName, obj);
      },
    };
  },
  decode(buffer: BinaryReader | Uint8Array | ArrayBuffer | ArrayBufferView, length?: number): any {
    const reader =
      buffer instanceof BinaryReader
        ? buffer
        : new BinaryReader(
            buffer instanceof Uint8Array
              ? buffer
              : ArrayBuffer.isView(buffer)
                ? new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength)
                : new Uint8Array(buffer as ArrayBuffer),
          );
    // The wrapper already captured the generated codec while assembling the
    // namespace. Decode through that typed reference instead of resolving the
    // same type name in the string registry for every message.
    const decoded = fns.decode(reader, length);
    attachToJSONIdentity(decoded);
    return decoded;
  },
  create(obj?: any) {
    return obj || {};
  },
  fromObject(obj?: any) {
    if (obj && typeof obj === "object") attachToJSONIdentity(obj);
    return obj || {};
  },
  toObject(obj: any) {
    return obj;
  },
  // Expose ts-proto's fromPartial for callers that prefer it / for the
  // namespace shim's own bookkeeping.
  fromPartial: fns.fromPartial?.bind(fns) ?? ((obj: any) => obj || {}),
});

const passthrough = () => ({
  create(obj?: any) {
    return obj || {};
  },
  fromObject(obj?: any) {
    return obj || {};
  },
});

const NAMESPACE_SEPARATOR = "_";
const PROTO_TYPE_SEPARATOR = ".";
const PROTOBUF_PACKAGE_EXPORT = "protobufPackage";

// Some message-class names need protobufjs-style aliases that don't fall out
// of the underscore-split scheme. Add them here as needed — `ADV*` is the
// classic case (proto has `ADVDeviceIdentity`; ts-proto preserves it; both
// shapes remain accepted for compatibility with persisted data).
const HISTORICAL_ALIASES: Record<string, string> = {
  ADVKeyIndexList: "ADVSignedKeyIndexList",
};

/** Top-level carriers that synthesize unknown future children. */
const FORWARD_COMPATIBLE_PARENTS = [
  "Message",
  "WebMessageInfo",
  "ContextInfo",
  "SyncActionValue",
] as const;

const isVisibleGeneratedExport = (name: string, value: AnyExport): boolean =>
  name !== PROTOBUF_PACKAGE_EXPORT &&
  !name.startsWith(NAMESPACE_SEPARATOR) &&
  typeof value !== "function" &&
  (isMessageFns(value) || isEnumLike(value));

const isCapitalizedIdentifier = (name: string): boolean => {
  const first = name.at(0);
  return first !== undefined && first === first.toUpperCase() && first !== first.toLowerCase();
};

/**
 * Preserve forward compatibility for the few well-known carrier namespaces.
 * The generated tree itself stays eager and plain; only an unknown capitalized
 * child on these carriers is synthesized and cached on demand.
 */
const wrapWithForwardCompatibleChildren = <T extends Record<string, any>>(target: T): T =>
  new Proxy(target, {
    get(current, prop, receiver) {
      const value = Reflect.get(current, prop, receiver);
      if (value !== undefined || typeof prop !== "string" || !isCapitalizedIdentifier(prop)) {
        return value;
      }
      const synthesized = passthrough();
      Reflect.set(current, prop, synthesized);
      return synthesized;
    },
  });

const buildNamespace = (): Record<string, any> => {
  const root: Record<string, any> = {};

  for (const [flatName, value] of Object.entries(gen)) {
    if (!isVisibleGeneratedExport(flatName, value)) continue;

    const path = flatName.split(NAMESPACE_SEPARATOR);
    const leaf = path.pop()!;
    let cursor = root;
    for (const segment of path) {
      cursor[segment] ??= {};
      cursor = cursor[segment];
    }

    if (isMessageFns(value)) {
      // A nested namespace may have been attached before its parent message;
      // merge it onto the wrapper so export order cannot discard children.
      const wrapped = wrapMessage(flatName.replaceAll(NAMESPACE_SEPARATOR, PROTO_TYPE_SEPARATOR), value);
      cursor[leaf] = Object.assign(wrapped, cursor[leaf] ?? {});
    } else if (isEnumLike(value)) {
      cursor[leaf] = Object.assign({}, cursor[leaf] ?? {}, value);
    }
  }

  for (const [alias, target] of Object.entries(HISTORICAL_ALIASES)) {
    if (root[target] && !root[alias]) root[alias] = root[target];
  }

  for (const parent of FORWARD_COMPATIBLE_PARENTS) {
    if (root[parent]) root[parent] = wrapWithForwardCompatibleChildren(root[parent]);
  }

  return root;
};

/**
 * Protobufjs-shaped namespace covering every type the bridge knows about.
 * Stable surface; safe to import as `proto`.
 */
export const proto: Record<string, any> = buildNamespace();
