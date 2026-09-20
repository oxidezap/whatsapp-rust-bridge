import { describe, test, expect } from "bun:test";
import { join } from "node:path";

async function runFixture(name: string) {
  const proc = Bun.spawn(["bun", "run", join(import.meta.dir, "fixtures", name)], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const [exit, stderr] = await Promise.all([
    proc.exited,
    new Response(proc.stderr).text(),
  ]);
  if (exit !== 0) {
    throw new Error(`${name} exited ${exit}; child stderr:\n${stderr}`);
  }
  return stderr;
}

describe("client teardown", () => {
  test("free after logout enters core does not crash", async () => {
    expect(await runFixture("teardown-logout-free.ts")).toBe("");
  });

  test("free after a pending manual connect is safe", async () => {
    expect(await runFixture("teardown-pending-free.ts")).toBe("");
  });

  test("disconnect is a barrier before free", async () => {
    expect(await runFixture("teardown-barrier.ts")).toBe("");
  });
});
