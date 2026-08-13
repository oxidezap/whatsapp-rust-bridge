/**
 * Where the `code` section went, by crate and by function.
 *
 * Needs an artifact that still has its `name` section — a release build with
 * `--config 'profile.release.strip="none"'`, and `wasm-opt` without
 * `--strip-debug`. Function body sizes come from the code section itself, so
 * what is reported is the shipped bytes rather than an estimate from source.
 *
 *   node benches/wasm-module-rss/attribute.mjs <named.wasm> [--top=40]
 */
import { readFileSync } from "node:fs";

function readVarUint(bytes, cursor) {
  let value = 0;
  let shift = 0;
  for (;;) {
    const byte = bytes[cursor.at++];
    value += (byte & 0x7f) * 2 ** shift;
    if ((byte & 0x80) === 0) return value;
    shift += 7;
  }
}

/** Function body sizes keyed by function index, plus the imported-function count. */
function parse(path) {
  const bytes = readFileSync(path);
  const cursor = { at: 8 };
  let importedFunctions = 0;
  const bodySizes = new Map();
  const names = new Map();

  while (cursor.at < bytes.byteLength) {
    const id = bytes[cursor.at++];
    const size = readVarUint(bytes, cursor);
    const end = cursor.at + size;

    if (id === 2) {
      const count = readVarUint(bytes, cursor);
      for (let i = 0; i < count; i++) {
        // Each length is read into a local first: `cursor.at += readVarUint(…)`
        // reads cursor.at before the call and writes back the stale value plus
        // the length, losing the varint's own bytes and desynchronising the
        // whole section — which silently shifts every function index below.
        const moduleLength = readVarUint(bytes, cursor);
        cursor.at += moduleLength;
        const nameLength = readVarUint(bytes, cursor);
        cursor.at += nameLength;
        const kind = bytes[cursor.at++];
        if (kind === 0) {
          importedFunctions++;
          readVarUint(bytes, cursor); // type index
        } else if (kind === 1 || kind === 2) {
          if (kind === 1) cursor.at++; // reftype
          const limitsFlag = readVarUint(bytes, cursor);
          readVarUint(bytes, cursor); // minimum
          if (limitsFlag & 1) readVarUint(bytes, cursor); // maximum
        } else if (kind === 3) {
          cursor.at += 2; // valtype + mutability
        } else {
          throw new Error(`unhandled import kind ${kind}`);
        }
      }
    } else if (id === 10) {
      const count = readVarUint(bytes, cursor);
      for (let i = 0; i < count; i++) {
        const before = cursor.at;
        const bodySize = readVarUint(bytes, cursor);
        bodySizes.set(importedFunctions + i, bodySize + (cursor.at - before));
        cursor.at += bodySize;
      }
    } else if (id === 0) {
      const nameLen = readVarUint(bytes, cursor);
      const sectionName = bytes.toString("utf8", cursor.at, cursor.at + nameLen);
      cursor.at += nameLen;
      if (sectionName === "name") {
        while (cursor.at < end) {
          const subId = bytes[cursor.at++];
          const subSize = readVarUint(bytes, cursor);
          const subEnd = cursor.at + subSize;
          if (subId === 1) {
            const count = readVarUint(bytes, cursor);
            for (let i = 0; i < count; i++) {
              const index = readVarUint(bytes, cursor);
              const len = readVarUint(bytes, cursor);
              names.set(index, bytes.toString("utf8", cursor.at, cursor.at + len));
              cursor.at += len;
            }
          }
          cursor.at = subEnd;
        }
      }
    }
    cursor.at = end;
  }
  return { bodySizes, names };
}

/**
 * rustc writes wasm names in readable form — `crate[hash]::path::item`, and
 * `<Self[hash]::Ty as Trait[hash]::T>::method` for a trait impl. The first
 * `ident[hash]` is the owner in both, which matches how a trait impl is
 * attributed to the type's crate rather than the trait's. Anything with no
 * hashed segment is a wasm-bindgen shim or a compiler-generated body and gets
 * its own bucket rather than a guess.
 */
const OWNER = /([A-Za-z_][A-Za-z0-9_]*)\[[0-9a-f]+\]/;

function crateOf(symbol) {
  if (!symbol) return "<unnamed>";
  return symbol.match(OWNER)?.[1] ?? "<no-crate>";
}

/** `crate::first::second`, so one crate's cost splits by where it lives. */
function moduleOf(symbol) {
  if (!symbol) return "<unnamed>";
  const owner = symbol.match(OWNER);
  if (!owner) return "<no-crate>";
  const rest = symbol.slice(owner.index + owner[0].length);
  const segments = rest
    .replace(/^::/, "")
    .split("::")
    .filter((s) => s && !s.startsWith("{") && !s.startsWith("<"));
  return [owner[1], ...segments.slice(0, 2)].join("::");
}

const [path] = process.argv.slice(2).filter((a) => !a.startsWith("--"));
const top = Number(process.argv.slice(2).find((a) => a.startsWith("--top="))?.slice(6) ?? 40);
if (!path) {
  console.error("usage: attribute.mjs <named.wasm> [--top=N]");
  process.exit(2);
}

const { bodySizes, names } = parse(path);
if (names.size === 0) {
  console.error(`${path} has no name section — build with strip="none" and no --strip-debug`);
  process.exit(1);
}

const byCrate = new Map();
const byModule = new Map();
let total = 0;
for (const [index, size] of bodySizes) {
  total += size;
  const symbol = names.get(index);
  const crate = crateOf(symbol);
  byCrate.set(crate, (byCrate.get(crate) ?? 0) + size);
  const module = moduleOf(symbol);
  byModule.set(module, (byModule.get(module) ?? 0) + size);
}

const kib = (bytes) => (bytes / 1024).toFixed(1);
const report = (title, map) => {
  console.log(`\n${title}:`);
  for (const [key, size] of [...map].sort((a, b) => b[1] - a[1]).slice(0, top)) {
    console.log(
      `  ${kib(size).padStart(9)} KiB  ${((size / total) * 100).toFixed(1).padStart(5)}%  ${key}`
    );
  }
};

console.log(`${path}: ${bodySizes.size} function bodies, ${kib(total)} KiB of code section`);
report("by crate", byCrate);
report("by module", byModule);

console.log("\nlargest function bodies:");
const largest = [...bodySizes].sort((a, b) => b[1] - a[1]).slice(0, top);
for (const [index, size] of largest) {
  console.log(`  ${kib(size).padStart(9)} KiB  ${names.get(index) ?? `func[${index}]`}`);
}
