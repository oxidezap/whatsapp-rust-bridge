/**
 * A non-stream where a stream is declared rejects instead of vanishing.
 *
 * #25 fixed this failure for tsify-typed parameters. `encryptMediaStream` had
 * it by a second route: wasm-bindgen casts an imported JS class *unchecked*,
 * so a plain object was accepted at the boundary and only failed once the
 * stream machinery reached for a reader. That throw came out of the async shim
 * as an uncaught exception — not a rejection, not catchable at the call site —
 * and the promise stayed pending for good:
 *
 *     encryptMediaStream({}, {}, "image")
 *       UNCAUGHT: Error: already locked to a reader
 *       outcome: STILL PENDING
 *
 * Both cases below would hang rather than fail if that came back.
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { initWasmEngine, createWhatsAppClient } from "../dist/index.js";
import { createHttp } from "./helpers.js";

beforeAll(() => {
  initWasmEngine();
});

async function offlineClient() {
  return createWhatsAppClient(
    { connect() {}, send() {}, disconnect() {} },
    createHttp()
  );
}

async function rejection(promise: Promise<unknown>): Promise<Error & { kind?: string; field?: string }> {
  try {
    await promise;
  } catch (error) {
    return error as Error & { kind?: string; field?: string };
  }
  throw new Error("expected the call to reject");
}

const readable = () => new ReadableStream({ start: (c) => c.close() });
const writable = () => new WritableStream();

describe("encryptMediaStream", () => {
  test("a non-stream argument rejects, naming which one", async () => {
    const client = await offlineClient();
    try {
      const input = await rejection(
        (client as any).encryptMediaStream({}, writable(), "image")
      );
      expect(input.kind).toBe("invalid-argument");
      expect(input.field).toBe("input");

      // The output stream is checked too. Passing a good input here is what
      // makes the second rejection attributable to `output` alone.
      const output = await rejection(
        (client as any).encryptMediaStream(readable(), {}, "image")
      );
      expect(output.kind).toBe("invalid-argument");
      expect(output.field).toBe("output");
    } finally {
      client.free();
    }
  }, 20000);

  test("real streams still cross", async () => {
    const client = await offlineClient();
    try {
      // Encryption is local, so this completes with no connection: proof the
      // check lets a well-formed argument through rather than rejecting both.
      const result = await (client as any).encryptMediaStream(
        readable(),
        writable(),
        "image"
      );
      expect(result.fileLength).toBe(0);
      expect(result.mediaKey).toBeInstanceOf(Uint8Array);
    } finally {
      client.free();
    }
  }, 20000);
});
