import { expect, test } from "bun:test";

// Separate processes keep codec initialization honest and detect leaked timers.
test("VoIP entrypoints share codecs while each engine releases its timers", async () => {
  const fixture = new URL("./fixtures/voip-lifecycle.mjs", import.meta.url).pathname;
  for (const runtime of ["node", process.execPath]) {
    for (const entry of ["root", "host"]) {
      const child = Bun.spawn([runtime, fixture], {
        stdout: "pipe", stderr: "pipe", env: { ...process.env, VOIP_ENTRY: entry },
      });
      let guard!: ReturnType<typeof setTimeout>;
      try {
        const status = await Promise.race([
          child.exited,
          new Promise<never>((_, reject) => {
            guard = setTimeout(() => reject(new Error(`plugin kept ${runtime} ${entry} alive after CLOSE`)), 3_000);
          }),
        ]);
        const stdout = await new Response(child.stdout).text();
        const stderr = await new Response(child.stderr).text();
        if (status !== 0) throw new Error(`${runtime} ${entry} failed:
${stderr}`);
        expect(stdout).toContain("opened and closed");
        expect(stderr).toBe("");
        expect(status).toBe(0);
      } finally {
        clearTimeout(guard);
        child.kill();
      }
    }
  }
});


test("isolated production engines deliver duplex PCM, timed playout and video", async () => {
  const fixture = new URL("./fixtures/voip-playout.mjs", import.meta.url).pathname;
  for (const runtime of ["node", process.execPath]) {
    const child = Bun.spawn([runtime, fixture], { stdout: "pipe", stderr: "pipe" });
    let guard!: ReturnType<typeof setTimeout>;
    try {
      const status = await Promise.race([
        child.exited,
        new Promise<never>((_, reject) => {
          guard = setTimeout(() => reject(new Error(`${runtime}: playout or teardown stalled`)), 4_000);
        }),
      ]);
      const stdout = await new Response(child.stdout).text();
      const stderr = await new Response(child.stderr).text();
      if (status !== 0) throw new Error(`${runtime}: ${stderr}`);
      expect(stdout).toContain("duplex PCM, timed playout and video delivered");
      expect(status).toBe(0);
    } finally {
      clearTimeout(guard);
      child.kill();
    }
  }
});
