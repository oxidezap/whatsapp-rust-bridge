/**
 * The host-loaded entrypoint contract, pinned without a build.
 *
 * `dist/host.js` is the same bridge as the default entrypoint, minus the
 * `node:fs` read: the host supplies the wasm and calls `initSync` itself.
 * What a pass proves: the host shell carries no `node:` import at all (so a
 * future `node:fs` introducion into its graph breaks here rather than in a
 * workerd deploy), both shells stay thin over the one shared `dist/bridge.js`
 * implementation (so the package never pays for the bridge twice), and the
 * host export shape matches the default entrypoint's. It proves nothing about
 * any runtime — workerd/wrangler/Deno smoke lives in `docs/host-entrypoint.md`
 * with the evidence, not here.
 *
 * Run: bun run build && bun test tests/host-entrypoint.test.ts
 */

import { describe, test, expect } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

function requireDist(path: string): string {
  const full = join(ROOT, "dist", path);
  if (!existsSync(full)) {
    throw new Error(`dist/${path} is absent — run \`bun run build\` first`);
  }
  return readFileSync(full, "utf8");
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

/** Every `node:` specifier a bundle imports or re-exports. */
function nodeImports(bundle: string): string[] {
  const found: string[] = [];
  for (const match of bundle.matchAll(/from\s*["'](node:[^"']*)["']/g)) {
    found.push(match[1]);
  }
  for (const match of bundle.matchAll(/import\s*["'](node:[^"']*)["']/g)) {
    found.push(match[1]);
  }
  return found;
}

describe("host entrypoint", () => {
  test("dist/host.js exists and imports no node: builtin", () => {
    const host = requireDist("host.js");
    expect(nodeImports(host)).toEqual([]);
    expect(host).not.toContain("readFileSync");
    expect(host).not.toContain("import.meta.url");
  });

  test("dist/bridge.js exists and imports no node: builtin", () => {
    // The shared chunk hinter both shells: a node: import there would ride
    // into workerd through the host shell, past the test above.
    expect(nodeImports(requireDist("bridge.js"))).toEqual([]);
  });

  test("both shells stay thin over the shared implementation", () => {
    // Two entries, one copy of the bridge: a shell that outgrew this carries
    // its own implementation, which is the ~1.2 MB duplication this guards.
    for (const shell of ["index.js", "host.js"]) {
      expect(requireDist(shell).length).toBeLessThan(8_192);
    }
    expect(requireDist("bridge.js").length).toBeGreaterThan(1_000_000);
  });

  test("the host export shape matches the default entrypoint", () => {
    // One surface (`ts/surface.ts`): a name on one entrypoint is on both.
    const host = exportedNames(requireDist("host.js"));
    const index = exportedNames(requireDist("index.js"));
    expect([...index].sort()).toEqual([...host].sort());
  });

  test("dist/host.d.ts exists and references only dist siblings", () => {
    const dts = requireDist("host.d.ts");
    expect(dts).not.toContain("../pkg/");
    expect(dts).toContain("initSync");
  });
});
