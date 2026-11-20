import { expect, test } from "bun:test";
import { minifySharedCode } from "../scripts/minify-shared";

test("shared-code compression retains host getter effects and exceptions", async () => {
  const code = await minifySharedCode(`
    export function probe(host) {
      if (host == null) return false;
      host.connected;
      return true;
    }
  `);
  const { probe } = await import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
  let reads = 0;
  expect(probe({ get connected() { reads++; return true; } })).toBe(true);
  expect(reads).toBe(1);
  expect(probe(null)).toBe(false);
  const refusal = new Error("host getter refused");
  expect(() => probe({ get connected() { throw refusal; } })).toThrow(refusal);
});
