import { expect, test } from "bun:test";

// A bare Node process exits only when the last live WASM timer has gone.
// Use a guard solely to bound a broken subprocess; success never waits it out.
test("closing an opened plugin call releases its stats timer", async () => {
  const fixture = new URL("./fixtures/voip-lifecycle.mjs", import.meta.url).pathname;
  const child = Bun.spawn(["node", fixture], { stdout: "pipe", stderr: "pipe" });
  let guard!: ReturnType<typeof setTimeout>;
  try {
    const status = await Promise.race([
      child.exited,
      new Promise<never>((_, reject) => {
        guard = setTimeout(() => reject(new Error("plugin kept Node alive after CLOSE")), 3_000);
      }),
    ]);
    const stdout = await new Response(child.stdout).text();
    const stderr = await new Response(child.stderr).text();
    expect(stdout).toContain("opened and closed");
    expect(stderr).toBe("");
    expect(status).toBe(0);
  } finally {
    clearTimeout(guard);
    child.kill();
  }
});
