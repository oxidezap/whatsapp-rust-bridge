/**
 * The edge entrypoint contract, pinned without a build.
 *
 * `dist/edge.js` is the same bridge with host-supplied wasm bytes instead of
 * the default entrypoint's `node:fs` read. What a pass proves: the bundle
 * carries no filesystem read (importing it must not touch `node:fs`), it
 * exports the host `initEdge` initializer instead of self-initializing, and
 * its export shape matches the default entrypoint's modulo that init swap.
 * It proves nothing about any runtime — workerd/wrangler/Deno smoke lives in
 * `docs/edge-entrypoint.md` with the evidence, not here.
 *
 * Run: bun run build && bun test tests/edge-entrypoint.test.ts
 */

import { describe, test, expect } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const EDGE_JS = join(ROOT, "dist", "edge.js");
const INDEX_JS = join(ROOT, "dist", "index.js");

function requireDist(path: string): string {
  if (!existsSync(path)) {
    throw new Error(`${path} is absent — run \`bun run build\` first`);
  }
  return readFileSync(path, "utf8");
}

/** Export names of the form `export{...}` / `export { ... }` in a bundle. */
function exportedNames(bundle: string): Set<string> {
  const names = new Set<string>();
  for (const match of bundle.matchAll(/export\s*\{([^}]*)\}/g)) {
    for (const part of match[1].split(",")) {
      const alias = part.trim().split(/\s+as\s+/);
      names.add(alias[alias.length - 1].trim());
    }
  }
  return names;
}

describe("edge entrypoint", () => {
  test("dist/edge.js exists and carries no filesystem read", () => {
    const edge = requireDist(EDGE_JS);
    expect(edge).not.toContain("node:fs");
    expect(edge).not.toContain("readFileSync");
    expect(edge).not.toContain("import.meta.url");
  });

  test("dist/edge.js exports initEdge and initializes nothing on import", () => {
    const edge = requireDist(EDGE_JS);
    // Self-initializing would call initSync at top level; the edge bundle
    // only wires it behind the host-called initEdge.
    expect(edge).toContain("initEdge");
    expect(edge.match(/initSync\(\{module/) ?? []).toHaveLength(0);
  });

  test("the edge export shape matches the default entrypoint modulo init", () => {
    const edge = exportedNames(requireDist(EDGE_JS));
    const index = exportedNames(requireDist(INDEX_JS));
    // initEdge is the host-called initializer; the wasm-bindgen initSync it
    // wraps stays exported on both. The default path differs only in calling
    // initSync itself at import (from its node:fs read).
    expect(edge.has("initEdge")).toBe(true);
    expect(index.has("initEdge")).toBe(false);
    for (const name of index) {
      expect(edge.has(name)).toBe(true);
    }
  });

  test("dist/edge.d.ts exists and references only dist siblings", () => {
    const dts = requireDist(join(ROOT, "dist", "edge.d.ts"));
    expect(dts).not.toContain("../pkg/");
    expect(dts).toContain("initEdge");
  });
});
