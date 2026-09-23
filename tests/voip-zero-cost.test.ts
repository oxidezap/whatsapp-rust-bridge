/**
 * Zero VoIP cost without the plugin: the core bundle never fetches,
 * instantiates, or embeds the engine WASM.
 *
 * The two WASMs ship in the same npm tarball, but the core bundle
 * (`dist/index.js`) must not reference the engine artifact
 * (`whatsapp_rust_voip_bg.wasm`) in any form — no static import, no
 * dynamic `import()` of its wrapper, no embedded bytes. `voipBackend`
 * stays the only entry point: without it the core uses only the JS
 * callbacks the host passed, and the engine module is never loaded,
 * compiled, or given linear memory.
 *
 * Why a string search proves it: `bun build` inlines every statically
 * reachable module into `dist/index.js`, so any `import ... from` (or a
 * dynamic `import()` with a resolvable specifier) naming the engine
 * artifact or its `pkg-voip/` wrapper would leave that specifier's text
 * in the bundle. Reading the emitted file is therefore exact — no fetch
 * instrumentation, no timing, no window a slow load could slip through.
 * If the bundle ever names the artifact, the count below goes nonzero
 * and this fails.
 */

import { describe, test, expect } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const DIST = join(import.meta.dir, "..", "dist");

describe("voip costs nothing without the plugin", () => {
  test("the core bundle never names the engine artifact", () => {
    const bundle = readFileSync(join(DIST, "index.js"), "utf8");
    const hits = bundle.split("whatsapp_rust_voip_bg").length - 1;
    expect(hits).toBe(0);
  });

  test("both WASMs still ship in the same package", () => {
    for (const artifact of [
      "whatsapp_rust_bridge_bg.wasm",
      "whatsapp_rust_voip_bg.wasm",
    ]) {
      expect(() => readFileSync(join(DIST, artifact))).not.toThrow();
    }
  });
});
