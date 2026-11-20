import { expect, test } from "bun:test";
import { join } from "node:path";

test("the packaged Node proto reader preserves UTF-8 and byte view semantics", async () => {
  const child = Bun.spawn(["node", join(import.meta.dir, "fixtures/proto-native-utf8.mjs")], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const [exit, stderr] = await Promise.all([child.exited, new Response(child.stderr).text()]);
  expect({ exit, stderr }).toEqual({ exit: 0, stderr: "" });
});
