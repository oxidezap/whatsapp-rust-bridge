import { test, expect } from "bun:test";

for (const runtime of [process.execPath, "node"]) {
  test(`synthetic Signal storage and host callbacks round-trip in ${runtime === "node" ? "Node" : "Bun"}`, () => {
    const result = Bun.spawnSync([runtime, "tests/fixtures/signal-provider-scenario.mjs"], {
      cwd: process.cwd(), stdout: "pipe", stderr: "pipe",
    });
    expect(result.exitCode, result.stderr.toString()).toBe(0);
    expect(result.stdout.toString()).toContain("provider isolation and failure rollback passed");
  });
}
