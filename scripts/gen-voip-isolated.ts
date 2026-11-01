import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const path = join(import.meta.dir, "../pkg-voip/whatsapp_rust_voip.js");
let glue = readFileSync(path, "utf8");
const replaceOnce = (from: string, to: string) => {
  if (glue.indexOf(from) < 0 || glue.indexOf(from) !== glue.lastIndexOf(from)) {
    throw new Error(`voip wasm-bindgen glue changed: expected exactly one ${from}`);
  }
  glue = glue.replace(from, to);
};
const exports: string[] = [];
glue = glue.replace(/^export (class|(?:async )?function) (\w+)/gm, (_, kind, name) => {
  exports.push(name);
  return `${kind} ${name}`;
});
if (exports.length === 0 || new Set(exports).size !== exports.length) {
  throw new Error("voip wasm-bindgen glue has no exports or duplicate exports");
}
replaceOnce("export { initSync, __wbg_init as default };", "");
// wasm-pack --dev also exposes the raw exports for test harnesses. The
// per-client factory returns only the typed API, not this module singleton.
glue = glue.replace(/^export \{ wasm as __wasm \}\s*$/m, "");
if (/^\s*(?:import |export )/m.test(glue)) {
  throw new Error("voip wasm-bindgen glue added an unhandled module import/export");
}

// wasm-bindgen's glue closes over a module-level `wasm` binding. Put the whole
// generated module in a fresh lexical scope per client, including its imports,
// memory views and finalizers. Every initSync now instantiates its own WASM.
const factory = `export function createVoipBindings() {\n${glue}\nreturn { initSync, ${exports.join(", ")} };\n}\n`;
writeFileSync(join(import.meta.dir, "../pkg-voip/isolated.js"), factory);
writeFileSync(join(import.meta.dir, "../pkg-voip/isolated.d.ts"),
  `export function createVoipBindings(): Pick<typeof import("./whatsapp_rust_voip.js"), "initSync" | ${exports.map(x => `"${x}"`).join(" | ")}>;\n`);
