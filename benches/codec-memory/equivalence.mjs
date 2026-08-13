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
 * What this does not cover: scalar field values beyond map entries (the
 * payloads set message and map fields), and nesting past one level. Two
 * differences it reports rather than asserts, because both are inherent to any
 * lazy namespace: before a type is first read its property is an accessor
 * where the eager namespace has a data descriptor, and the key order differs —
 * today's comes from the bundler's namespace object over 869 individual
 * exports, which a design that moves the codecs into one bag cannot reproduce.
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
  // Maps go in whatever their value type is: ts-proto routes every map through
  // a generated `…MapEntry` codec, so a `map<string,string>` is a cross-codec
  // edge the lazy rewrite touched just as much as a message field is.
  if (type !== "message" && !type.startsWith("map<")) continue;
  const fields = messageFields.get(path) ?? [];
  // ts-proto emits lowerCamelCase, and a few fields are declared snake_case.
  fields.push({
    field: field.replace(/_([a-z])/g, (_, c) => c.toUpperCase()),
    label,
    scalarMap: type.startsWith("map<") && !type.endsWith("message>"),
  });
  messageFields.set(path, fields);
}

/** One empty instance of every message field — every cross-codec edge, once. */
const payloadFor = (path) => {
  const payload = {};
  for (const { field, label, scalarMap } of messageFields.get(path) ?? []) {
    payload[field] = scalarMap ? { k: "v" } : label === "repeated" ? [{}] : label === "map" ? { 1: {} } : {};
  }
  return payload;
};

const walk = (node, path, out, depth = 0) => {
  if (!node || typeof node !== "object" || depth > 8) return;
  for (const key of Object.keys(node)) {
    const next = path ? `${path}.${key}` : key;
    out.push(next);
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
  // Every spelling only `proto.ts`'s hand-written registry knows. The all-codec
  // sweep drives generated names, so nothing else reaches these five, and a
  // typo in any of them would leave the suite green.
  ["AdvSignedDeviceIdentity", { details: new Uint8Array([9]) }],
  ["AdvSignedKeyIndexList", { accountSignatureKey: new Uint8Array([1]) }],
  ["AdvDeviceIdentity", { rawId: 1, keyIndex: 2 }],
  ["AdvSignedDeviceIdentityHmac", { details: new Uint8Array([2]), hmac: new Uint8Array([3]) }],
  ["LidMigrationMappingSyncPayload", { chatDbMigrationTimestamp: 7 }],
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
  // The same six types `in-situ-probe.mjs` touches, so this counts what the
  // measured workload materializes rather than a smaller stand-in.
  fresh.proto.HandshakeMessage.encode({ clientHello: { ephemeral: new Uint8Array(32) } }).finish();
  fresh.proto.ClientPayload.encode({ passive: false }).finish();
  fresh.proto.ADVSignedDeviceIdentity.encode({ details: new Uint8Array(8) }).finish();
  fresh.proto.DeviceProps.encode({ os: "bridge" }).finish();
  const wire = fresh.proto.Message.encode({ extendedTextMessage: { text: "ping" } }).finish();
  fresh.proto.Message.decode(wire);
  fresh.proto.WebMessageInfo.encode({ key: { id: "A" }, message: { conversation: "pong" } }).finish();
  const after = stillLazy();
  console.log(
    `lazyboth-pertype: ${before}/${total} top-level types unmaterialized after import, ${after} after a ping-pong exchange`,
  );
  // Absolute, not just the delta: a rewrite that materialized unrelated
  // wrappers during import would move both numbers together and still pass.
  check(`${total} top-level types, ${before} unmaterialized after import`, total === 270 && before === 250);
  check(`the six types the measured workload reads materialize exactly six (${before - after})`, after === 244);
}

const codecPaths = [];
{
  const paths = [];
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

  // Ordered, not a set: `Object.keys`, spread and `JSON.stringify` all expose
  // insertion order, so a tree built codecs-first would read differently.
  const expected = [];
  walk(stock.proto, "", expected);
  const actual = [];
  walk(mod.proto, "", actual);
  const expectedSet = new Set(expected);
  const actualSet = new Set(actual);
  const missing = expected.filter((p) => !actualSet.has(p));
  const extra = actual.filter((p) => !expectedSet.has(p));
  check(`namespace paths (${expected.length}) — missing ${missing.length}, extra ${extra.length}`, !missing.length && !extra.length);
  if (missing.length) console.log("    missing:", missing.slice(0, 5));
  if (extra.length) console.log("    extra:", extra.slice(0, 5));

  // Reported, not asserted. Today's key order is bun's order over 869
  // individual exports of the generated module; a design that moves the codecs
  // into one bag cannot reproduce it, whatever order it builds in. It is the
  // second inherent difference, alongside the descriptor shape.
  const firstDivergence = expected.findIndex((path, index) => actual[index] !== path);
  console.log(
    firstDivergence < 0
      ? "  note key order matches stock"
      : `  note key order differs from stock at ${firstDivergence}: ${expected[firstDivergence]} vs ${actual[firstDivergence]} — inherent, see docs/proto-codec-memory.md`,
  );

  const broken = [];
  for (const path of codecPaths) {
    const payload = payloadFor(path);
    try {
      const a = Buffer.from(at(stock.proto, path).encode(payload).finish());
      const b = Buffer.from(at(mod.proto, path).encode(payload).finish());
      const da = JSON.stringify(at(stock.proto, path).decode(a));
      const db = JSON.stringify(at(mod.proto, path).decode(b));
      // `fromPartial` is rewritten by the lazy transform too, and it is public.
      const pa = JSON.stringify(at(stock.proto, path).fromPartial(payload));
      const pb = JSON.stringify(at(mod.proto, path).fromPartial(payload));
      if (Buffer.compare(a, b) !== 0 || da !== db || pa !== pb) broken.push(path);
    } catch (error) {
      broken.push(`${path} (${error.message})`);
    }
  }
  const edges = codecPaths.reduce((total, path) => total + (messageFields.get(path)?.length ?? 0), 0);
  check(
    `all ${codecPaths.length} codecs round-trip identically through encode/decode/fromPartial, ${edges} nested fields exercised`,
    broken.length === 0,
  );
  if (broken.length) console.log("    broken:", broken.slice(0, 5));

  for (const [name, value] of CASES) {
    const viaFn = Buffer.compare(Buffer.from(stock.encodeProto(name, value)), Buffer.from(mod.encodeProto(name, value))) === 0;
    const decoded =
      JSON.stringify(stock.decodeProto(name, stock.encodeProto(name, value))) ===
      JSON.stringify(mod.decodeProto(name, mod.encodeProto(name, value)));
    check(`${name}: encodeProto ${viaFn}, decodeProto ${decoded}`, viaFn && decoded);
  }

  // `HISTORICAL_ALIASES` installs `ADVKeyIndexList` only when the generated
  // module has no such export, and it has one — so today the alias never fires
  // and the two stay distinct codecs. Checking the target alone would just
  // recheck one of the 657.
  check(
    "ADVKeyIndexList and ADVSignedKeyIndexList are both present and distinct, as on stock",
    typeof mod.proto.ADVKeyIndexList?.encode === "function" &&
      typeof mod.proto.ADVSignedKeyIndexList?.encode === "function" &&
      (mod.proto.ADVKeyIndexList === mod.proto.ADVSignedKeyIndexList) ===
        (stock.proto.ADVKeyIndexList === stock.proto.ADVSignedKeyIndexList),
  );
  check(
    "unknown child on a forward-compatible carrier synthesizes",
    typeof mod.proto.Message.SomeUnreleasedThing?.fromObject === "function",
  );
}

// A consumer may freeze the namespace before reading a type. The eager tree
// stays readable after that; a lazy one that insists on writing itself back
// would throw, which is a difference a consumer can see.
{
  const frozen = await load("lazyboth-pertype", "?frozen=1");
  Object.freeze(frozen.proto);
  let ok = false;
  let stable = false;
  try {
    ok = Buffer.compare(
      Buffer.from(frozen.proto.HistorySync.encode({}).finish()),
      Buffer.from(stock.proto.HistorySync.encode({}).finish()),
    ) === 0;
    // Identity too: a getter that cannot write itself back must still hand out
    // the same object, and the forward-compatible carriers wrap on the way out.
    stable = frozen.proto.HistorySync === frozen.proto.HistorySync && frozen.proto.Message === frozen.proto.Message;
  } catch (error) {
    console.log(`    ${error.message}`);
  }
  console.log("\nfrozen namespace");
  check("a type read for the first time after Object.freeze(proto) still resolves", ok);
  check("a frozen namespace hands out the same object twice", stable);
}

// `Object.seal` leaves data properties writable, so assigning a type still
// works on the eager namespace. A lazy one whose setter insists on redefining
// the now non-configurable accessor would throw instead.
{
  const sealed = await load("lazyboth-pertype", "?sealed=1");
  const eager = await load("stock", "?sealed=1");
  const assign = (mod) => {
    Object.seal(mod.proto);
    try {
      mod.proto.HistorySync = { marker: true };
      return mod.proto.HistorySync?.marker === true;
    } catch (error) {
      return error.message;
    }
  };
  const lazyResult = assign(sealed);
  const eagerResult = assign(eager);
  check(`assignment after Object.seal(proto) behaves as stock does (${eagerResult})`, lazyResult === eagerResult);

  // And after a freeze it must refuse, the way a frozen data property does.
  const frozenLazy = await load("lazyboth-pertype", "?frozen-write=1");
  const frozenEager = await load("stock", "?frozen-write=1");
  const assignFrozen = (mod) => {
    Object.freeze(mod.proto);
    try {
      mod.proto.HistorySync = { marker: true };
      return `accepted, marker=${mod.proto.HistorySync?.marker === true}`;
    } catch (error) {
      return error.constructor.name;
    }
  };
  const lazyFrozen = assignFrozen(frozenLazy);
  const eagerFrozen = assignFrozen(frozenEager);
  check(`assignment after Object.freeze(proto) behaves as stock does (${eagerFrozen})`, lazyFrozen === eagerFrozen);

  // The same write from non-strict code. A frozen data property ignores it
  // silently; a setter cannot see the caller's strictness, so it has to pick
  // one behaviour for both. Reported, not asserted — the third difference.
  // `new Function` is the sloppy-mode call site an ES module cannot otherwise
  // write.
  const sloppyAssign = new Function("target", "target.HistorySync = { marker: true }; return target.HistorySync;");
  const sloppy = async (name) => {
    const mod = await load(name, "?sloppy=1");
    Object.freeze(mod.proto);
    try {
      return `accepted, marker=${sloppyAssign(mod.proto)?.marker === true}`;
    } catch (error) {
      return error.constructor.name;
    }
  };
  // An overlay: `Object.create(proto)` then a write, before the type is read.
  // A data property makes an own property on the overlay and leaves the shared
  // namespace alone; a setter that ignored its receiver would rewrite `proto`
  // for every holder instead.
  const overlay = async (name) => {
    const mod = await load(name, "?overlay=1");
    const local = Object.create(mod.proto);
    local.HistorySync = { replacement: true };
    return `own=${Object.hasOwn(local, "HistorySync")} local=${local.HistorySync?.replacement === true} shared=${mod.proto.HistorySync?.replacement === true}`;
  };
  const lazyOverlay = await overlay("lazyboth-pertype");
  const eagerOverlay = await overlay("stock");
  check(`a write through Object.create(proto) behaves as stock does (${eagerOverlay})`, lazyOverlay === eagerOverlay);

  // `Object.defineProperty` after a seal. Stock's sealed data property is still
  // writable, so a value descriptor lands; a non-configurable accessor cannot
  // become a data property, so the same call throws. Reported, not asserted.
  const redefine = async (name) => {
    const mod = await load(name, "?redefine=1");
    Object.seal(mod.proto);
    try {
      Object.defineProperty(mod.proto, "HistorySync", { value: { marker: true } });
      return `accepted, marker=${mod.proto.HistorySync?.marker === true}`;
    } catch (error) {
      return error.constructor.name;
    }
  };
  const lazyRedefine = await redefine("lazyboth-pertype");
  const eagerRedefine = await redefine("stock");

  // The same call without sealing first. Redefining an existing data property
  // leaves omitted attributes alone, so stock stays writable; converting an
  // accessor defaults them to false, so the next assignment throws. Reported —
  // `defineProperty` bypasses the accessor, so only a Proxy could see it.
  const redefineOpen = async (name) => {
    const mod = await load(name, "?redefine-open=1");
    Object.defineProperty(mod.proto, "HistorySync", { value: { marker: true } });
    const writable = Object.getOwnPropertyDescriptor(mod.proto, "HistorySync").writable;
    try {
      mod.proto.HistorySync = { second: true };
      return `writable=${writable}, later assignment ok`;
    } catch (error) {
      return `writable=${writable}, later assignment ${error.constructor.name}`;
    }
  };
  const lazyOpen = await redefineOpen("lazyboth-pertype");
  const eagerOpen = await redefineOpen("stock");

  // `Reflect.set` reports rather than throws: a frozen data property's [[Set]]
  // returns false, while a setter runs and this one throws. Same root as the
  // sloppy-mode case — the accessor cannot hand the decision back to the caller.
  const reflectFrozen = async (name) => {
    const mod = await load(name, "?reflect=1");
    Object.freeze(mod.proto);
    try {
      return `returned ${Reflect.set(mod.proto, "HistorySync", { marker: true })}`;
    } catch (error) {
      return `threw ${error.constructor.name}`;
    }
  };
  const lazyReflect = await reflectFrozen("lazyboth-pertype");
  const eagerReflect = await reflectFrozen("stock");
  console.log(
    `  note Reflect.set after Object.freeze(proto): stock ${eagerReflect}, lazy ${lazyReflect}` +
      (lazyReflect === eagerReflect ? "" : " — inherent to an accessor, see docs/proto-codec-memory.md"),
  );
  console.log(
    `  note Object.defineProperty(proto, …, { value }) before any read: stock ${eagerOpen}, lazy ${lazyOpen}` +
      (lazyOpen === eagerOpen ? "" : " — inherent to an accessor, see docs/proto-codec-memory.md"),
  );
  console.log(
    `  note Object.defineProperty after Object.seal(proto): stock ${eagerRedefine}, lazy ${lazyRedefine}` +
      (lazyRedefine === eagerRedefine ? "" : " — inherent to an accessor, see docs/proto-codec-memory.md"),
  );

  const lazySloppy = await sloppy("lazyboth-pertype");
  const eagerSloppy = await sloppy("stock");
  console.log(
    `  note sloppy-mode assignment after Object.freeze(proto): stock ${eagerSloppy}, lazy ${lazySloppy}` +
      (lazySloppy === eagerSloppy ? "" : " — inherent to an accessor, see docs/proto-codec-memory.md"),
  );
}

console.log(failures ? `\n${failures} check(s) failed` : "\nall checks passed");
process.exit(failures ? 1 : 0);
