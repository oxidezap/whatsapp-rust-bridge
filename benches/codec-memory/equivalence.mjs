/**
 * Is the per-type lazy arm the same library as `stock`?
 *
 * A lazy `proto` is only worth measuring if a consumer cannot tell it apart, so
 * this compares the two bundles rather than asserting they match: every
 * namespace path, the bytes each type encodes, what each decodes back, the
 * registry's non-generated spellings, the historical alias, and the synthesized
 * unknown child on the forward-compatible carriers. It also reports how much of
 * the tree is still unmaterialized after a ping-pong exchange, which is the
 * whole point of the arm.
 *
 * Run `bun run bench:codec-memory:in-situ` first — it builds the bundles.
 * Then: node --stack-size=4000 benches/codec-memory/equivalence.mjs
 */

import { join } from "node:path";

const WORK = join(import.meta.dirname, "..", "..", "target", "codec-memory", "in-situ");
const load = (name, query = "") => import(`${join(WORK, `${name}.js`)}${query}`);

const stock = await load("stock");

const walk = (node, path, out, depth = 0) => {
  if (!node || typeof node !== "object" || depth > 8) return;
  for (const key of Object.keys(node)) {
    const next = path ? `${path}.${key}` : key;
    out.add(next);
    walk(node[key], next, out, depth + 1);
  }
};

const CASES = [
  ["Message", { conversation: "hi" }],
  ["Message.ImageMessage", { url: "u", mimetype: "image/jpeg", fileLength: 12 }],
  ["WebMessageInfo", { key: { remoteJid: "1@s.whatsapp.net", id: "X" }, message: { conversation: "y" } }],
  ["SyncActionValue", { starAction: { starred: true } }],
  ["ADVSignedDeviceIdentity", { details: new Uint8Array([1, 2, 3]) }],
  // Only `proto.ts`'s hand-written registry knows this spelling.
  ["AdvSignedDeviceIdentity", { details: new Uint8Array([9]) }],
];

const at = (root, name) => name.split(".").reduce((node, segment) => node[segment], root);

let failures = 0;
const check = (label, ok) => {
  if (!ok) failures++;
  console.log(`  ${ok ? "ok  " : "FAIL"} ${label}`);
};

// Laziness, before anything walks the tree and forces it.
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
  console.log(`lazyboth-pertype: ${before}/${total} top-level types unmaterialized after import, ${stillLazy()} after a ping-pong exchange`);
  check("touching three types leaves the rest unbuilt", stillLazy() >= before - 6);
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

  for (const [name, value] of CASES) {
    const viaFn = Buffer.compare(Buffer.from(stock.encodeProto(name, value)), Buffer.from(mod.encodeProto(name, value))) === 0;
    const decoded =
      JSON.stringify(stock.decodeProto(name, stock.encodeProto(name, value))) ===
      JSON.stringify(mod.decodeProto(name, mod.encodeProto(name, value)));
    // The alias is a registry spelling, not a namespace path.
    const viaNamespace =
      name === "AdvSignedDeviceIdentity" ||
      Buffer.compare(
        Buffer.from(at(stock.proto, name).encode(value).finish()),
        Buffer.from(at(mod.proto, name).encode(value).finish()),
      ) === 0;
    check(`${name}: encodeProto ${viaFn}, decodeProto ${decoded}, proto.${name}.encode ${viaNamespace}`, viaFn && decoded && viaNamespace);
  }

  check("ADVSignedKeyIndexList alias present", typeof mod.proto.ADVSignedKeyIndexList?.encode === "function");
  check(
    "unknown child on a forward-compatible carrier synthesizes",
    typeof mod.proto.Message.SomeUnreleasedThing?.fromObject === "function",
  );
}

console.log(failures ? `\n${failures} check(s) failed` : "\nall checks passed");
process.exit(failures ? 1 : 0);
