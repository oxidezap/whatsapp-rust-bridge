/**
 * Every exported method settles.
 *
 * A bridge method that neither resolves nor rejects is the worst failure the
 * surface can have: the host gets no value, no error, and no signal that the
 * call is gone. #25 fixed one instance of it — `#[tsify(from_wasm_abi)]`
 * throwing from inside the async shim — one method at a time, against a test
 * per method. That only covers the methods someone thought to list.
 *
 * This sweeps instead. It reads the method list out of the emitted `.d.ts`, so
 * a method added tomorrow is covered on the next run without anyone adding it
 * here, and calls each one with arguments synthesised from its declared
 * parameter types.
 *
 * What a pass proves: the call settled. It does not prove the outcome is the
 * right one — the synthesised arguments are well-typed but semantically
 * arbitrary, and there is no mock server here, so nothing checks a response
 * body. Settling is the whole property.
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { readFileSync } from "node:fs";
import { initWasmEngine, createWhatsAppClient } from "../dist/index.js";
import { createHttp } from "./helpers.js";

const DTS = new URL("../dist/whatsapp_rust_bridge.d.ts", import.meta.url);

type Method = { name: string; params: string[] };

/** The body of a declaration block, by its opening line. */
function block(dts: string, opening: string): string {
  const start = dts.indexOf(opening);
  if (start < 0) throw new Error(`${opening} is missing from the emitted .d.ts`);
  return dts.slice(start, dts.indexOf("\n}", start));
}

/** The methods declared on the exported client, with their parameter types. */
function readSurface(): { methods: Method[]; unions: Map<string, string> } {
  const dts = readFileSync(DTS, "utf8");

  // Two blocks, not one. `skip_typescript` keeps a method out of the class and
  // hand-writes it into a merged `interface` instead, so reading only the class
  // would leave those methods unswept while the sweep still reported a pass.
  const methods: Method[] = [];
  for (const opening of [
    "export class WasmWhatsAppClient {",
    "interface WasmWhatsAppClient {",
  ]) {
    const before = methods.length;
    for (const m of block(dts, opening).matchAll(
      /^ {4}([a-zA-Z_][A-Za-z0-9_]*)\(([^)]*)\): [^;]+;$/gm
    )) {
      const list = m[2].trim();
      // Splitting on "," is only safe while no parameter type carries one of
      // its own. A generic would, so fail loudly rather than synthesise
      // nonsense.
      if (list.includes("<")) throw new Error(`generic parameter in ${m[1]}: ${list}`);
      methods.push({
        name: m[1],
        params: list ? list.split(",").map((p) => p.slice(p.indexOf(":") + 1).trim()) : [],
      });
    }
    // A block that parses to nothing is the failure this loop exists to
    // prevent, and it would otherwise be invisible.
    if (methods.length === before) throw new Error(`no methods parsed from ${opening}`);
  }

  const unions = new Map<string, string>();
  for (const u of dts.matchAll(/^export type (\w+) = ("[^"]*")(?: \| "[^"]*")*;$/gm)) {
    unions.set(u[1], JSON.parse(u[2]));
  }

  return { methods, unions };
}

/** A well-typed value for a declared parameter type. */
function synthesise(type: string, unions: Map<string, string>): unknown {
  const t = type.replace(/\s*\|\s*(null|undefined)/g, "").trim();

  if (t.endsWith("[]")) return [synthesise(t.slice(0, -2), unions)];
  if (unions.has(t)) return unions.get(t);

  switch (t) {
    case "string":
      // Most string parameters here are JIDs, and one that parses gets the call
      // past the boundary and into the core, which is the deeper path to sweep.
      return "5511999999999@s.whatsapp.net";
    case "number":
      return 1;
    case "boolean":
      return false;
    case "Uint8Array":
      return new Uint8Array(32);
    case "Function":
      return () => {};
    // Real streams, because a non-stream here is a second, separate instance
    // of the same trap: wasm-bindgen casts an imported type unchecked, so the
    // failure surfaces as an uncaught exception and the promise never settles.
    // Feeding garbage would make this sweep fail on a defect it is not the
    // place to fix; it is reported and fixed on its own.
    case "ReadableStream":
      return new ReadableStream({
        start(c) {
          c.close();
        },
      });
    case "WritableStream":
      return new WritableStream();
    default:
      // Interfaces and `any`. An empty object is usually rejected, which is a
      // settled outcome and so still exercises the property.
      return {};
  }
}

/**
 * Methods the sweep cannot call, each for a reason that is about the method
 * rather than about the property under test.
 */
const EXCLUDED: Record<string, string> = {
  free: "drops the client the rest of the sweep is still calling",
  connect: "opens the transport; the sweep is deliberately offline",
  disconnect: "tears down the transport under the calls still in flight",
  logout: "same teardown, plus it clears the store",
  run: "starts the reconnect loop, which never settles by design",
  assertSessions: "settles, but only after the core's 60s gate — covered on its own below",
};

async function offlineClient() {
  return createWhatsAppClient({ connect() {}, send() {}, disconnect() {} }, createHttp());
}

/** Whether `call` settled within `budgetMs`. */
async function settles(call: unknown, budgetMs: number): Promise<boolean> {
  if (typeof (call as Promise<unknown>)?.then !== "function") return true;
  let timer: ReturnType<typeof setTimeout>;
  const pending = new Promise<false>((resolve) => {
    timer = setTimeout(() => resolve(false), budgetMs);
  });
  return Promise.race([
    (call as Promise<unknown>).then(
      () => true,
      () => true
    ),
    pending,
  ]).finally(() => clearTimeout(timer!));
}

beforeAll(() => {
  initWasmEngine();
});

describe("the exported surface", () => {
  test("every method settles rather than leaving its promise pending", async () => {
    const { methods, unions } = readSurface();

    // A regex that stopped matching would leave this test green while testing
    // nothing, so hold it to the size the surface actually has.
    expect(methods.length).toBeGreaterThan(150);

    const names = new Set(methods.map((m) => m.name));
    for (const name of Object.keys(EXCLUDED)) {
      // An exclusion for a method that no longer exists is a stale exemption.
      expect(names).toContain(name);
    }

    const client = await offlineClient();
    try {
      const swept = methods.filter((m) => !(m.name in EXCLUDED));
      const outcomes = await Promise.all(
        swept.map(async ({ name, params }) => {
          const args = params.map((p) => synthesise(p, unions));
          try {
            // The calls run together against one client: they are independent
            // and offline, and sequentially each budget would compound.
            return { name, settled: await settles((client as any)[name](...args), 15000) };
          } catch {
            // A synchronous throw is catchable at the call site, so the host
            // still learns the call failed.
            return { name, settled: true };
          }
        })
      );

      expect(outcomes.filter((o) => !o.settled).map((o) => o.name)).toEqual([]);
    } finally {
      client.free();
    }
  }, 30000);

  test("assertSessions settles once the core's offline-sync gate expires", async () => {
    // `Client::ensure_e2e_sessions` waits for offline delivery to end before it
    // touches the socket, and disconnected that wait runs to its full
    // `DEFAULT_OFFLINE_SYNC_TIMEOUT` of 60s before the call reports
    // `not-connected`. Slow, but bounded — an unbounded gate would be the
    // pending promise this file exists to catch, so it is worth the wall clock.
    const client = await offlineClient();
    try {
      expect(await settles(client.assertSessions([
        "5511999999999@s.whatsapp.net",
      ], false), 90000)).toBe(true);
    } finally {
      client.free();
    }
  }, 120000);
});
