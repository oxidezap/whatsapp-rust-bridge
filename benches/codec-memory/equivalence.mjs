/**
 * Is the per-type lazy arm the same library as `stock`?
 *
 * A lazy `proto` is only worth measuring if a consumer cannot tell it apart, so
 * this compares the two bundles rather than asserting they match.
 *
 * Every codec is exercised, not a sample of them: the defect a lazy rewrite
 * introduces is a factory that is never reached or names a neighbour that no
 * longer exists, and reading a property does not catch either — only calling
 * `encode`/`decode` does. So each of the 657 types is round-tripped with one
 * empty instance of every message field it declares, which is exactly the set
 * of cross-codec calls the rewrite touched. `ts/generated/whatsapp-surface.txt`
 * says which fields those are.
 *
 * What this does not cover: scalar field values (the payloads set message
 * fields only), and nesting past one level.
 *
 * Run `bun run bench:codec-memory:in-situ` first — it builds the bundles.
 * Then: bun run bench:codec-memory:equivalence
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dirname, "..", "..");
const WORK = join(ROOT, "target", "codec-memory", "in-situ");
const load = (name, query = "") => import(`${join(WORK, `${name}.js`)}${query}`);

const stock = await load("stock");

/** message path -> the message-typed fields it declares, with their label. */
const messageFields = new Map();
for (const line of readFileSync(join(ROOT, "ts", "generated", "whatsapp-surface.txt"), "utf8").split("\n")) {
  if (!line || line.startsWith("#")) continue;
  const [path, field, , label, type] = line.split("\t");
  if (!field || !type) continue;
  const isMessage = type === "message" || type.startsWith("map<") ? type.endsWith("message>") || type === "message" : false;
  if (!isMessage) continue;
  const fields = messageFields.get(path) ?? [];
  fields.push({ field, label });
  messageFields.set(path, fields);
}

/** One empty instance of every message field — every cross-codec edge, once. */
const payloadFor = (path) => {
  const payload = {};
  for (const { field, label } of messageFields.get(path) ?? []) {
    payload[field] = label === "repeated" ? [{}] : label === "map" ? { 1: {} } : {};
  }
  return payload;
};

const walk = (node, path, out, depth = 0) => {
  if (!node || typeof node !== "object" || depth > 8) return;
  for (const key of Object.keys(node)) {
    const next = path ? `${path}.${key}` : key;
    out.add(next);
    walk(node[key], next, out, depth + 1);
  }
};

const at = (root, name) => name.split(".").reduce((node, segment) => node[segment], root);
const isCodec = (value) =>
  value && typeof value === "object" && typeof value.encode === "function" && typeof value.decode === "function";

// Richer payloads for the handful of types a client actually drives, so the
// sweep above is not the only thing carrying real field values.
const CASES = [
  ["Message", { conversation: "hi" }],
  ["Message.ImageMessage", { url: "u", mimetype: "image/jpeg", fileLength: 12 }],
  ["WebMessageInfo", { key: { remoteJid: "1@s.whatsapp.net", id: "X" }, message: { conversation: "y" } }],
  ["SyncActionValue", { starAction: { starred: true } }],
  ["ADVSignedDeviceIdentity", { details: new Uint8Array([1, 2, 3]) }],
  // Only `proto.ts`'s hand-written registry knows this spelling.
  ["AdvSignedDeviceIdentity", { details: new Uint8Array([9]) }],
];

let failures = 0;
const check = (label, ok) => {
  if (!ok) failures++;
  console.log(`  ${ok ? "ok  " : "FAIL"} ${label}`);
};

// Laziness, read before anything walks the tree and forces it.
{
  const fresh = await load("lazyboth-pertype", "?fresh=1");
  const stillLazy = () =>
    Object.keys(fresh.proto).filter((k) => typeof Object.getOwnPropertyDescriptor(fresh.proto, k).get === "function")
      .length;
  const total = Object.keys(fresh.proto).length;
  const before = stillLazy();
  const wire = fresh.proto.Message.encode({ extendedTextMessage: { text: "ping" } }).finish();
  fresh.proto.Message.decode(wire);
  fresh.proto.WebMessageInfo.encode({ key: { id: "A" }, message: { conversation: "pong" } }).finish();
  fresh.proto.ClientPayload.encode({ passive: false }).finish();
  console.log(
    `lazyboth-pertype: ${before}/${total} top-level types unmaterialized after import, ${stillLazy()} after a ping-pong exchange`,
  );
  check("touching three types leaves the rest unbuilt", stillLazy() >= before - 6);
}

const codecPaths = [];
{
  const paths = new Set();
  walk(stock.proto, "", paths);
  for (const path of paths) {
    // A codec's own methods are functions, not codecs; only collect the nodes.
    if (path.split(".").length > 1 && ["encode", "decode", "create", "fromObject", "toObject", "fromPartial"].includes(path.split(".").pop())) {
      continue;
    }
    if (isCodec(at(stock.proto, path))) codecPaths.push(path);
  }
  console.log(`\n${codecPaths.length} codec paths reachable through proto (657 types plus the ADVSignedKeyIndexList alias)`);
}

for (const arm of ["lazyns-pertype", "lazyboth-pertype"]) {
  console.log(`\n${arm} vs stock`);
  const mod = await load(arm);

  const expected = new Set();
  walk(stock.proto, "", expected);
  const actual = new Set();
  walk(mod.proto, "", actual);
  const missing = [...expected].filter((p) => !actual.has(p));
  const extra = [...actual].filter((p) => !expected.has(p));
  check(`namespace paths (${expected.size}) — missing ${missing.length}, extra ${extra.length}`, !missing.length && !extra.length);
  if (missing.length) console.log("    missing:", missing.slice(0, 5));
  if (extra.length) console.log("    extra:", extra.slice(0, 5));

  const broken = [];
  for (const path of codecPaths) {
    const payload = payloadFor(path);
    try {
      const a = Buffer.from(at(stock.proto, path).encode(payload).finish());
      const b = Buffer.from(at(mod.proto, path).encode(payload).finish());
      const da = JSON.stringify(at(stock.proto, path).decode(a));
      const db = JSON.stringify(at(mod.proto, path).decode(b));
      if (Buffer.compare(a, b) !== 0 || da !== db) broken.push(path);
    } catch (error) {
      broken.push(`${path} (${error.message})`);
    }
  }
  const edges = codecPaths.reduce((total, path) => total + (messageFields.get(path)?.length ?? 0), 0);
  check(`all ${codecPaths.length} codecs round-trip identically, ${edges} nested fields exercised`, broken.length === 0);
  if (broken.length) console.log("    broken:", broken.slice(0, 5));

  for (const [name, value] of CASES) {
    const viaFn = Buffer.compare(Buffer.from(stock.encodeProto(name, value)), Buffer.from(mod.encodeProto(name, value))) === 0;
    const decoded =
      JSON.stringify(stock.decodeProto(name, stock.encodeProto(name, value))) ===
      JSON.stringify(mod.decodeProto(name, mod.encodeProto(name, value)));
    check(`${name}: encodeProto ${viaFn}, decodeProto ${decoded}`, viaFn && decoded);
  }

  check("ADVSignedKeyIndexList alias present", typeof mod.proto.ADVSignedKeyIndexList?.encode === "function");
  check(
    "unknown child on a forward-compatible carrier synthesizes",
    typeof mod.proto.Message.SomeUnreleasedThing?.fromObject === "function",
  );
}

console.log(failures ? `\n${failures} check(s) failed` : "\nall checks passed");
process.exit(failures ? 1 : 0);
