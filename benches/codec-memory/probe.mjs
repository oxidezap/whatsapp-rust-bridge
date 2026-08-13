/**
 * One process, one bundle: import it and report private memory.
 *
 * `Private_Dirty` from smaps_rollup, never `Rss`: the file-backed half of Rss
 * moves with whatever else the machine is doing, and a clean page stops being
 * private as soon as a second process maps the same file.
 *
 * argv: <bundle.js> [touch]
 */

import { readFileSync } from "node:fs";

const field = (text, name) => {
  const match = new RegExp(`^${name}:\\s+(\\d+) kB$`, "m").exec(text);
  if (!match) throw new Error(`/proc/self/smaps_rollup carries no ${name} line — this probe needs Linux`);
  return Number(match[1]);
};
const smaps = () => {
  const text = readFileSync("/proc/self/smaps_rollup", "utf8");
  return { privateDirty: field(text, "Private_Dirty"), rss: field(text, "Rss") };
};
// Two full collections: the first can promote, the second collects what it
// promoted.
const settle = () => {
  global.gc();
  global.gc();
};

const bundle = process.argv[2];
const touch = process.argv[3] === "touch";

settle();
const before = smaps();
// Deltas for the same reason as Private_Dirty: a fresh node does not start from
// a fixed heap.
const baseline = process.memoryUsage();

const mod = await import(bundle);
globalThis.__keep = mod;
const count = Object.keys(mod.codecs).length;

if (touch) {
  const name = mod.codecs.ADVDeviceIdentity ? "ADVDeviceIdentity" : Object.keys(mod.codecs)[0];
  if (name) {
    const codec = mod.codecs[name];
    globalThis.__touched = codec.decode(codec.encode(codec.fromPartial({})).finish());
  }
}

settle();
const after = smaps();
const heap = process.memoryUsage();
console.log(
  JSON.stringify({
    bundle,
    codecs: count,
    privateDirty: after.privateDirty,
    deltaPrivateDirty: after.privateDirty - before.privateDirty,
    rss: after.rss,
    heapUsed: Math.round((heap.heapUsed - baseline.heapUsed) / 1024),
    heapTotal: Math.round((heap.heapTotal - baseline.heapTotal) / 1024),
    external: Math.round((heap.external - baseline.external) / 1024),
  }),
);
