import { expect, test } from "bun:test";
import { join } from "node:path";

test.each([
  ["native", "keeps outputs and nested encodes independent"],
  ["process-shim", "accepts a host with partial Node metadata"],
])("the packaged proto writer in %s %s", async (runtime) => {
  const child = Bun.spawn(["node", join(import.meta.dir, "fixtures/proto-writer-reuse.mjs"), runtime], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const [exit, stderr] = await Promise.all([child.exited, new Response(child.stderr).text()]);
  expect({ exit, stderr }).toEqual({ exit: 0, stderr: "" });
});
