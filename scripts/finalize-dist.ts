/**
 * Make `dist/` the self-contained publish root: copy the wasm-bindgen
 * declarations in and point `index.d.ts` at the local copy, so the tarball
 * does not need `pkg/` (which forced a second copy of proto-types.d.ts).
 *
 * NodeNext (and node16) resolve type imports by exact file name, so every
 * relative specifier in the published declarations carries its `.js`
 * extension. The sources stay extensionless — bun build and wasm-bindgen emit
 * them that way — and tsc does not rewrite specifiers, so the extension is
 * added here, beside the one rewrite this script already owned. See
 * `scripts/dts-specifiers.ts`: specifiers come from the TypeScript AST rather
 * than a regexp, so a quoted path in a comment or a string-literal type is
 * never mistaken for a module, and whether `.js` is needed is decided by what
 * `dist/` actually contains rather than by a dot heuristic.
 */
import {
  copyFileSync,
  existsSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { rewriteDtsSpecifiers } from "./dts-specifiers";

const ROOT = join(import.meta.dir, "..");
const DIST = join(ROOT, "dist");
const BINDGEN_DTS = "whatsapp_rust_bridge.d.ts";

copyFileSync(join(ROOT, "pkg", BINDGEN_DTS), join(DIST, BINDGEN_DTS));
copyFileSync(join(ROOT, "pkg-voip", "whatsapp_rust_voip.d.ts"), join(DIST, "whatsapp_rust_voip.d.ts"));

// `surface.d.ts` and `host.d.ts` are emitted by tsc from extensionless
// sources, so each carries the same `../pkg/` reference to rewrite.
// `index.d.ts` only re-exports `./surface` and has no `../pkg/` reference;
// the loop below still rewrites its `.js` specifiers like every other file.
for (const entryDts of ["surface.d.ts", "host.d.ts", "voip-host.d.ts"]) {
  const entryPath = join(DIST, entryDts);
  const source = readFileSync(entryPath, "utf8");
  const rewritten = source.replaceAll(
    entryDts === "voip-host.d.ts" ? "../pkg-voip/whatsapp_rust_voip.js" : "../pkg/whatsapp_rust_bridge.js",
    entryDts === "voip-host.d.ts" ? "./whatsapp_rust_voip.js" : "./whatsapp_rust_bridge.js",
  );
  if (rewritten === source) {
    throw new Error(
      `dist/${entryDts} no longer references ../pkg/whatsapp_rust_bridge.js — update finalize-dist.ts`,
    );
  }
  writeFileSync(entryPath, rewritten);
}

const isDistFile = (relPath: string): boolean => {
  const path = join(DIST, relPath);
  return existsSync(path) && statSync(path).isFile();
};

for (const file of readdirSync(DIST).filter((name) => name.endsWith(".d.ts"))) {
  const path = join(DIST, file);
  const before = readFileSync(path, "utf8");
  const { text: after } = rewriteDtsSpecifiers(before, file, isDistFile);
  if (after !== before) writeFileSync(path, after);
}

// Rewriting is idempotent, so a second pass that changes anything — or that
// throws on an unresolvable specifier — is the drift this script exists to
// catch rather than to ship.
for (const file of readdirSync(DIST).filter((name) => name.endsWith(".d.ts"))) {
  const path = join(DIST, file);
  const text = readFileSync(path, "utf8");
  const { text: again } = rewriteDtsSpecifiers(text, file, isDistFile);
  if (again !== text) {
    throw new Error(
      `dist/${file} still has an unrewritten relative import — update finalize-dist.ts`,
    );
  }
}
