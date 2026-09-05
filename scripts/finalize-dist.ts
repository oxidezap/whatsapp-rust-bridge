/**
 * Make `dist/` the self-contained publish root: copy the wasm-bindgen
 * declarations in and point `index.d.ts` at the local copy, so the tarball
 * does not need `pkg/` (which forced a second copy of proto-types.d.ts).
 *
 * NodeNext (and node16) resolve type imports by exact file name, so every
 * relative specifier in the published declarations carries its `.js`
 * extension. The sources stay extensionless — bun build and wasm-bindgen emit
 * them that way — and tsc does not rewrite specifiers, so the extension is
 * added here, beside the one rewrite this script already owned.
 */
import { copyFileSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..");
const DIST = join(ROOT, "dist");
const BINDGEN_DTS = "whatsapp_rust_bridge.d.ts";

copyFileSync(join(ROOT, "pkg", BINDGEN_DTS), join(DIST, BINDGEN_DTS));

const indexDts = join(DIST, "index.d.ts");
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

// A relative specifier without a trailing extension (`./proto`,
// `import('./proto-types')`): what tsc and wasm-bindgen emit, and what
// NodeNext rejects with TS2834/TS2835. Bare package imports (`@bufbuild/…`,
// `node:…`) never match: the specifier must start with a dot.
const EXTENSIONLESS = /(?<=(?:from\s+|import\s*\(\s*)["'])(\.{1,2}\/[^"']*?)(?=["'])/g;

const withExtension = (specifier: string): string => {
  const leaf = specifier.split("/").pop() ?? specifier;
  return leaf.includes(".") ? specifier : `${specifier}.js`;
};

for (const file of readdirSync(DIST).filter((name) => name.endsWith(".d.ts"))) {
  const path = join(DIST, file);
  const before = readFileSync(path, "utf8");
  const after = before.replace(EXTENSIONLESS, withExtension);
  if (after !== before) writeFileSync(path, after);
}

const offenders: string[] = [];
for (const file of readdirSync(DIST).filter((name) => name.endsWith(".d.ts"))) {
  const path = join(DIST, file);
  const text = readFileSync(path, "utf8");
  for (const match of text.matchAll(EXTENSIONLESS)) {
    const leaf = match[1]!.split("/").pop() ?? match[1]!;
    if (!leaf.includes(".")) offenders.push(`${file}: ${match[1]}`);
  }
}
if (offenders.length > 0) {
  throw new Error(
    `dist/ still has extensionless relative imports — update finalize-dist.ts:\n${offenders.join("\n")}`,
  );
}
