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
import * as ts from "../scripts/typescript-api";

const DTS = new URL("../dist/whatsapp_rust_bridge.d.ts", import.meta.url);

type Method = { name: string; params: string[] };

/** Read declarations structurally so package whitespace is not a test input. */
async function readSurface(): Promise<{ methods: Method[]; unions: Map<string, string> }> {
  const source = await ts.createSourceFile("surface.d.ts", readFileSync(DTS, "utf8"));
  const methods: Method[] = [];
  const unions = new Map<string, string>();
  for (const declaration of source.statements) {
    if ((ts.isClassDeclaration(declaration) || ts.isInterfaceDeclaration(declaration)) &&
        declaration.name?.text === "WasmWhatsAppClient") {
      for (const member of declaration.members) {
        if (ts.isConstructorDeclaration(member)) continue;
        if (!ts.isMethodDeclaration(member) && !ts.isMethodSignatureDeclaration(member)) {
          throw new Error(`unparsed client member: ${member.getText(source)}`);
        }
        if (ts.isComputedPropertyName(member.name) && member.name.expression.getText(source) === "Symbol.dispose") continue;
        if (!ts.isIdentifier(member.name)) throw new Error(`unnamed client method: ${member.getText(source)}`);
        methods.push({ name: member.name.text, params: member.parameters.map(p => p.type?.getText(source) ?? "any") });
      }
    }
    if (ts.isTypeAliasDeclaration(declaration) && ts.isUnionTypeNode(declaration.type) &&
        declaration.type.types.every(t => ts.isLiteralTypeNode(t) && ts.isStringLiteral(t.literal))) {
      const first = declaration.type.types[0] as ts.LiteralTypeNode;
      unions.set(declaration.name.text, (first.literal as ts.StringLiteral).text);
    }
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
    // Real streams rather than the empty object every other class gets: they
    // take the method through its own work instead of stopping at the
    // argument check. A non-stream is covered in `stream-arguments.test.ts`,
    // where the rejection can be attributed to one parameter.
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
    const { methods, unions } = await readSurface();

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
