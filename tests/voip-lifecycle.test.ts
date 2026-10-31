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


// The normal suite has no signaling mock. The pump's dual-queue boundary
// regression always runs in Rust; this adds the complete public API traversal.
test.skipIf(!process.env.MOCK_SERVER_URL)("public callPushVideo delivers a real IDR through both WASMs", async () => {
  const fixture = new URL("./fixtures/voip-public-video.mjs", import.meta.url).pathname;
  for (const runtime of ["node", process.execPath]) {
    const child = Bun.spawn([runtime, fixture], {
      stdout: "pipe", stderr: "pipe",
      env: { ...process.env, NODE_TLS_REJECT_UNAUTHORIZED: "0" },
    });
    let guard!: ReturnType<typeof setTimeout>;
    try {
      const status = await Promise.race([
        child.exited,
        new Promise<never>((_, reject) => {
          guard = setTimeout(() => reject(new Error(`${runtime}: public video or cleanup stalled`)), 30_000);
        }),
      ]);
      const stdout = await new Response(child.stdout).text();
      const stderr = await new Response(child.stderr).text();
      if (status !== 0) throw new Error(`${runtime}: ${stdout}\n${stderr}`);
      expect(stdout).toContain("public callPushVideo delivered matching H.264 through both WASMs");
    } finally {
      clearTimeout(guard);
      child.kill();
    }
  }
}, 65_000);
