/**
 * Slice ts/generated/whatsapp.ts down to a chosen set of message codecs.
 *
 * Parses the generated file into top-level blocks, builds the runtime
 * dependency graph between codecs (`Foo.encode(...)` / `Foo.decode(...)` calls
 * inside another codec's body), then emits a module containing only the
 * transitive closure of a root set.
 *
 * Enums and the shared helpers are always emitted, unchanged, so the only
 * variable between two slices is how many message codecs are present.
 *
 * See docs/proto-codec-memory.md for what this was built to measure.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";

export const ROOT = join(import.meta.dir, "..", "..");
const SOURCE = join(ROOT, "ts", "generated", "whatsapp.ts");

export interface Block {
  kind: "enum" | "interface" | "createBase" | "codec" | "other";
  name: string | undefined;
  text: string;
}

export function parseBlocks(source: string): Block[] {
  const lines = source.split("\n");
  const blocks: Block[] = [];
  let i = 0;
  let pending: string[] = [];

  const flushOther = () => {
    if (pending.length) {
      blocks.push({ kind: "other", name: undefined, text: pending.join("\n") });
      pending = [];
    }
  };

  while (i < lines.length) {
    const line = lines[i]!;
    let kind: Block["kind"] | undefined;
    let name: string | undefined;

    let m: RegExpExecArray | null;
    if ((m = /^export enum ([A-Za-z0-9_]+) \{$/.exec(line))) {
      kind = "enum";
      name = m[1];
    } else if ((m = /^export interface ([A-Za-z0-9_]+) \{$/.exec(line))) {
      kind = "interface";
      name = m[1];
    } else if ((m = /^function createBase([A-Za-z0-9_]+)\(\)/.exec(line))) {
      kind = "createBase";
      name = m[1];
    } else if ((m = /^export const ([A-Za-z0-9_]+):(?:$| MessageFns<)/.exec(line))) {
      kind = "codec";
      name = m[1];
    }

    if (kind === undefined) {
      pending.push(line);
      i++;
      continue;
    }

    flushOther();
    // Brace depth, not a column-zero `}`: prettier indents the object literal
    // of a codec whose type name forced the declaration to wrap, so those end
    // on `  };`. Every brace in this generated file is balanced — the only
    // template literals it emits interpolate `${…}`.
    let end = i;
    let depth = 0;
    let opened = false;
    for (; end < lines.length; end++) {
      for (const ch of lines[end]!) {
        if (ch === "{") {
          depth++;
          opened = true;
        } else if (ch === "}") depth--;
      }
      // `opened`: a declaration whose type name forced a wrap has no brace on
      // its first lines, and depth 0 there is "not started", not "finished".
      if (opened && depth === 0) break;
    }
    if (end >= lines.length) throw new Error(`unterminated block at line ${i + 1}: ${line}`);
    blocks.push({ kind, name, text: lines.slice(i, end + 1).join("\n") });
    i = end + 1;
  }
  flushOther();
  return blocks;
}

export interface Parsed {
  blocks: Block[];
  /** Enum and codec names in generated-source order — `Object.entries(gen)`. */
  exportOrder: string[];
  codecs: Map<string, Block>;
  bases: Map<string, Block>;
  interfaces: Map<string, Block>;
  enums: Map<string, Block>;
  deps: Map<string, Set<string>>;
}

export function parse(source = readFileSync(SOURCE, "utf8")): Parsed {
  const blocks = parseBlocks(source);
  const codecs = new Map<string, Block>();
  const bases = new Map<string, Block>();
  const interfaces = new Map<string, Block>();
  const enums = new Map<string, Block>();
  for (const b of blocks) {
    if (b.kind === "codec") codecs.set(b.name!, b);
    else if (b.kind === "createBase") bases.set(b.name!, b);
    else if (b.kind === "interface") interfaces.set(b.name!, b);
    else if (b.kind === "enum") enums.set(b.name!, b);
  }

  // Runtime dependency: a codec body naming another codec. `X.encode(`,
  // `X.decode(`, `X.fromPartial(` and `X.create(` are the only shapes ts-proto
  // emits for a cross-message call — but prettier will put a long name and the
  // `.decode(` that follows it on separate lines, so the gap has to be allowed
  // or those edges are missing from every closure.
  const deps = new Map<string, Set<string>>();
  for (const [name, block] of codecs) {
    const set = new Set<string>();
    for (const m of block.text.matchAll(/([A-Za-z0-9_]+)\s*\.(?:encode|decode|fromPartial|create)\(/g)) {
      const ref = m[1]!;
      if (ref !== name && codecs.has(ref)) set.add(ref);
    }
    deps.set(name, set);
  }
  const exportOrder = blocks
    .filter((b) => b.kind === "enum" || b.kind === "codec")
    .map((b) => b.name!);
  return { blocks, exportOrder, codecs, bases, interfaces, enums, deps };
}

export function closure(parsed: Parsed, roots: readonly string[]): Set<string> {
  const seen = new Set<string>();
  const stack = [...roots];
  while (stack.length) {
    const n = stack.pop()!;
    if (seen.has(n)) continue;
    if (!parsed.codecs.has(n)) throw new Error(`unknown codec root: ${n}`);
    seen.add(n);
    for (const d of parsed.deps.get(n) ?? []) stack.push(d);
  }
  return seen;
}

export type Mode = "eager" | "lazy";

const CODEC_HEAD = /^export const [A-Za-z0-9_]+:\s*MessageFns<[\s\S]*?>\s*=\s*/;

/**
 * Emit a module exporting `codecs`, an object carrying the codecs in `keep`.
 *
 * `eager` builds every codec object while the module evaluates, as the
 * generated file does today. `lazy` keeps every byte of the same codec source
 * but builds each object only when its property is first read — the control
 * for "does deferring execution pay what removing the text pays?".
 */
export function emit(parsed: Parsed, keep: Set<string>, mode: Mode = "eager", exportBag = true): string {
  const out: string[] = [];
  const names: string[] = [];
  for (const block of parsed.blocks) {
    if (block.kind === "codec") {
      const name = block.name!;
      if (!keep.has(name)) continue;
      names.push(name);
      out.push(mode === "eager" ? block.text : lazify(block, parsed.codecs));
      continue;
    }
    if (block.kind === "createBase" && !keep.has(block.name!)) continue;
    // Enums, interfaces and the shared helpers go into every slice unchanged,
    // so they cannot move the measurement.
    out.push(block.text);
  }
  if (!exportBag) return out.join("\n");
  out.push(
    mode === "eager"
      ? `export const codecs: Record<string, any> = {\n${names.map((n) => `  ${n},`).join("\n")}\n};\n`
      : // The getter writes itself back as a plain property on first read, so
        // `resolve()` pays for the deferral once rather than on every encode.
        `function _install(key: string, value: any): any {\n` +
        `  Object.defineProperty(codecs, key, { value, writable: true, enumerable: true, configurable: true });\n` +
        `  return value;\n}\n\n` +
        `export const codecs: Record<string, any> = {\n${names
          .map((n) => `  get ${n}() { return _install(${JSON.stringify(n)}, _mk_${n}()); },`)
          .join("\n")}\n};\n`,
  );
  return out.join("\n");
}

/**
 * `export const X: MessageFns<X> = { … };` becomes a `_mk_X()` factory holding
 * the identical object literal, memoized in a module-level slot. Cross-codec
 * calls go through the neighbour's factory, so the deferral is real: a slice
 * nothing touches builds no codec object, and one that touches `Message`
 * builds only what that decode actually reaches.
 */
function lazify(block: Block, codecs: Map<string, Block>): string {
  const name = block.name!;
  if (!CODEC_HEAD.test(block.text)) throw new Error(`unrecognized codec head for ${name}`);
  const body = block.text
    .replace(CODEC_HEAD, "")
    .replace(/;\s*$/, "")
    // `\s*` is not cosmetic: prettier puts a long codec name and the `.decode(`
    // that follows it on separate lines, and a rewrite that only matches the
    // adjacent form leaves those call sites naming a binding the lazy module no
    // longer has. `bench:codec-memory:equivalence` is what caught that.
    .replace(/(^|[^.\w])([A-Za-z0-9_]+)(\s*)\.(encode|decode|fromPartial|create)\(/g, (match, lead, ref, gap, method) =>
      codecs.has(ref) ? `${lead}_mk_${ref}()${gap}.${method}(` : match,
    );
  return [
    `let _lz_${name}: any;`,
    `function _mk_${name}(): any {`,
    `  return (_lz_${name} ??= ${body});`,
    `}`,
  ].join("\n");
}

/** Roots a Baileys-compatible client reaches before any cut could apply. */
export const PING_PONG_ROOTS = [
  "Message",
  "WebMessageInfo",
  "MessageKey",
  "ClientPayload",
  "HandshakeMessage",
  "DeviceProps",
  "ADVSignedDeviceIdentity",
  "ADVSignedDeviceIdentityHMAC",
  "ADVKeyIndexList",
  "ADVDeviceIdentity",
  "ADVSignedKeyIndexList",
  "SenderKeyDistributionMessage",
  "SenderKeyMessage",
] as const;

/** The type names `ts/proto.ts` registers by hand. */
export const REGISTRY_ROOTS = [
  "Message", "WebMessageInfo", "HistorySync", "SyncActionData", "ClientPayload",
  "ADVSignedDeviceIdentity", "ADVSignedKeyIndexList", "ADVDeviceIdentity",
  "ADVSignedDeviceIdentityHMAC", "HandshakeMessage", "SyncdRecord", "SyncdMutation",
  "SyncdMutations", "SyncdPatch", "SyncdSnapshot", "ExitCode", "SyncActionValue",
  "DeviceProps", "SenderKeyDistributionMessage", "SenderKeyMessage", "ServerErrorReceipt",
  "CertChain", "CertChain_NoiseCertificate", "CertChain_NoiseCertificate_Details",
  "ExternalBlobReference", "LIDMigrationMappingSyncPayload", "MediaRetryNotification",
  "VerifiedNameCertificate", "VerifiedNameCertificate_Details", "Message_PollVoteMessage",
  "Message_EventResponseMessage",
] as const;

if (import.meta.main) {
  const source = readFileSync(SOURCE, "utf8");
  const parsed = parse(source);

  // A slice keeping every codec must reproduce the file it was cut from, or
  // the parser is dropping something and every count below is fiction.
  const rebuilt = emit(parsed, new Set(parsed.codecs.keys()), "eager");
  const roundTrip = rebuilt.slice(0, rebuilt.lastIndexOf("\nexport const codecs")).trimEnd();
  if (roundTrip !== source.trimEnd()) throw new Error("slicer does not round-trip the generated codec");

  const referenced = new Set<string>();
  for (const deps of parsed.deps.values()) for (const dep of deps) referenced.add(dep);
  const registry = closure(parsed, REGISTRY_ROOTS);

  const rows: [string, number][] = [
    ["message codecs", parsed.codecs.size],
    ["enums", parsed.enums.size],
    ["reached by no other codec", [...parsed.codecs.keys()].filter((n) => !referenced.has(n)).length],
    ["closure of Message", closure(parsed, ["Message"]).size],
    ["closure of WebMessageInfo", closure(parsed, ["WebMessageInfo"]).size],
    ["closure of HistorySync", closure(parsed, ["HistorySync"]).size],
    ["closure of ClientPayload", closure(parsed, ["ClientPayload"]).size],
    ["closure of the ping-pong roots", closure(parsed, PING_PONG_ROOTS).size],
    ["closure of the proto.ts registry", registry.size],
    ["outside that closure", parsed.codecs.size - registry.size],
  ];
  for (const [label, value] of rows) console.log(`${label.padEnd(34)} ${String(value).padStart(4)}`);
}
