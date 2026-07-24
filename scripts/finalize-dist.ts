/**
 * Make `dist/` the self-contained publish root: copy the wasm-bindgen
 * declarations in and point `index.d.ts` at the local copy, so the tarball
 * does not need `pkg/` (which forced a second copy of proto-types.d.ts).
 */
import { copyFileSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..");
const BINDGEN_DTS = "whatsapp_rust_bridge.d.ts";

copyFileSync(join(ROOT, "pkg", BINDGEN_DTS), join(ROOT, "dist", BINDGEN_DTS));

const indexDts = join(ROOT, "dist", "index.d.ts");
const source = readFileSync(indexDts, "utf8");
const rewritten = source.replaceAll(
  "../pkg/whatsapp_rust_bridge.js",
  "./whatsapp_rust_bridge.js",
);
if (rewritten === source) {
  throw new Error(
    "dist/index.d.ts no longer references ../pkg/whatsapp_rust_bridge.js — update finalize-dist.ts",
  );
}
writeFileSync(indexDts, rewritten);
