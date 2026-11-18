import { expect, test } from "bun:test";
import { join } from "node:path";

test("the packaged Node writer keeps outputs and nested encodes independent", async () => {
  const child = Bun.spawn(["node", join(import.meta.dir, "fixtures/proto-writer-reuse.mjs")], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const [exit, stderr] = await Promise.all([child.exited, new Response(child.stderr).text()]);
  expect({ exit, stderr }).toEqual({ exit: 0, stderr: "" });
});
