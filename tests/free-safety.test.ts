/**
 * free() with a call in flight must not fault.
 *
 * Each case runs in a child process because a use-after-free regression
 * takes the whole process with it — and a hang would take the watchdog.
 * The child frees the client on the same tick as the call, then churns
 * the wasm heap (200 small allocations) before waiting: without the churn
 * the freed wrapper memory usually survives intact and the fault hides,
 * which is what makes this a use-after-free rather than a logic error.
 * The churn makes the freed chunk reuse deterministic.
 */

import { describe, test, expect, beforeAll } from "bun:test";

const WASM_MEMORY_FAULTS = [
  "memory access out of bounds",
  "RefCell already borrowed",
  "Unreachable code should not be executed",
  "panicked",
];

function childSource(body: string): string {
  return `
    import { createWhatsAppClient, initWasmEngine, getEnabledFeatures } from ${JSON.stringify(
      new URL("../dist/index.js", import.meta.url).href
    )};
    initWasmEngine();
    const c = await createWhatsAppClient(
      { connect() {}, send() {}, disconnect() {} },
      { execute: async () => ({ statusCode: 0, body: new Uint8Array(0) }) }
    );
    console.log("READY");
    ${body}
    c.free();
    for (let i = 0; i < 200; i++) getEnabledFeatures();
    await new Promise((r) => setTimeout(r, 1500));
  `;
}

async function runChild(
  body: string
): Promise<{ code: number | null; stderr: string; stdout: string }> {
  const proc = Bun.spawn(["bun", "--eval", childSource(body)], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const timeout = setTimeout(() => {
    try {
      proc.kill();
    } catch {
      // Already exited; the exit below reports it.
    }
  }, 20_000);
  const code = await proc.exited;
  clearTimeout(timeout);
  const stdout = await new Response(proc.stdout).text();
  const stderr = await new Response(proc.stderr).text();
  return { code, stderr, stdout };
}

describe("free() with a call in flight", () => {
  test("freeing with fetchBlocklist pending exits cleanly", async () => {
    const outcome = await runChild(`c.fetchBlocklist().catch(() => {});`);
    expect(outcome.stdout).toContain("READY");
    expect(outcome.code).toBe(0);
    for (const fault of WASM_MEMORY_FAULTS) {
      expect(outcome.stderr).not.toContain(fault);
    }
  }, 30000);

  test("freeing with logout pending exits cleanly", async () => {
    const outcome = await runChild(`c.logout().catch(() => {});`);
    expect(outcome.stdout).toContain("READY");
    expect(outcome.code).toBe(0);
    for (const fault of WASM_MEMORY_FAULTS) {
      expect(outcome.stderr).not.toContain(fault);
    }
  }, 30000);

  // The call domain's async methods run on owned state (see `CallMedia`),
  // so freeing underneath them is safe by construction rather than by
  // heap luck. One body per shape: validation failure, gate plus core
  // failure, unknown record, and stanza send.
  const callBodies = [
    `c.acceptCall("NEVER-RANG", "mlow").catch(() => {});`,
    `c.dialCall("5511999999999@s.whatsapp.net", "mlow").catch(() => {});`,
    `c.endCall("NEVER-LIVE").catch(() => {});`,
    `c.rejectCall("ID", "5511999999999@s.whatsapp.net", "5511888888888@s.whatsapp.net").catch(() => {});`,
  ];

  for (const body of callBodies) {
    test(`freeing with ${body.slice(2, body.indexOf("("))} pending exits cleanly`, async () => {
      const outcome = await runChild(body);
      expect(outcome.stdout).toContain("READY");
      expect(outcome.code).toBe(0);
      for (const fault of WASM_MEMORY_FAULTS) {
        expect(outcome.stderr).not.toContain(fault);
      }
    }, 30000);
  }
});
