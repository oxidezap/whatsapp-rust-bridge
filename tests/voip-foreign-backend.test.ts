/**
 * The `extensions.voipBackend` install path: parsing, handshake, and the
 * no-plugin default.
 *
 * No mock server runs in CI, so no test here completes a call. What these
 * prove: the trailing `extensions` argument parses the way the other
 * construction arguments do (a misshapen plugin is the caller's
 * `invalid-argument`, naming `extensions`), the version handshake runs
 * before the client exists (a major mismatch rejects with no client), and a
 * well-formed plugin installs (its push handler gets registered). Media
 * itself crossing is covered by the Rust boundary tests against a fake JS
 * plugin; the two-WASM traversal lands with the engine module.
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { initWasmEngine, createWhatsAppClient } from "../dist/index.js";
import type { VoipBackendCallbacks } from "../dist/index.js";
import { createHttp } from "./helpers.js";

beforeAll(() => {
  initWasmEngine();
});

const MAGIC = [0x4f, 0x5a, 0x56, 0x50]; // OZVP
const ABI_MAJOR = 1;

/** Frames one message the way the bridge does: 13-byte header + payload. */
function frame(opcode: number, payload: Uint8Array, major = ABI_MAJOR): Uint8Array {
  const out = new Uint8Array(13 + payload.length);
  out.set(MAGIC, 0);
  out[4] = major;
  out[5] = 0;
  out[6] = opcode;
  out[7] = 0;
  out[8] = 0;
  new DataView(out.buffer).setUint32(9, payload.length, true);
  out.set(payload, 13);
  return out;
}

function helloResponse(caps: number): Uint8Array {
  const payload = new Uint8Array(4);
  new DataView(payload.buffer).setUint32(0, caps, true);
  // opcode 0x01 (HELLO) with the RESPONSE flag.
  const out = frame(0x01, payload);
  out[7] = 1;
  return out;
}

/** A plugin double: answers HELLO, records the push handler. */
function fakePlugin(opts?: { major?: number; caps?: number }): VoipBackendCallbacks & {
  pushHandler: ((frame: Uint8Array) => void) | null;
} {
  const plugin = {
    pushHandler: null as ((frame: Uint8Array) => void) | null,
    async sendFrame(frame: Uint8Array): Promise<Uint8Array> {
      const opcode = frame[6];
      if (opcode === 0x01) return helloResponse(opts?.caps ?? 0xff_ffff);
      return frame.slice(0, 0); // unreachable in these tests
    },
    setPushHandler(handler: (frame: Uint8Array) => void): void {
      plugin.pushHandler = handler;
    },
  };
  if (opts?.major !== undefined) {
    const major = opts.major;
    const inner = plugin.sendFrame;
    plugin.sendFrame = async (frame: Uint8Array) => {
      const resp = await inner(frame);
      resp[4] = major;
      return resp;
    };
  }
  return plugin;
}

async function rejection(
  promise: Promise<unknown>,
): Promise<Error & { kind?: string; field?: string }> {
  try {
    await promise;
  } catch (error) {
    return error as Error & { kind?: string; field?: string };
  }
  throw new Error("expected the call to reject");
}

function offlineClient(extensions?: unknown) {
  return createWhatsAppClient(
    { connect() {}, send() {}, disconnect() {} },
    createHttp(),
    null,
    null,
    null,
    null,
    null,
    null,
    null,
    extensions as never,
  );
}

describe("extensions.voipBackend", () => {
  test("a misshapen plugin is the caller's extensions argument", async () => {
    for (const voipBackend of [
      42,
      { sendFrame: 1, setPushHandler() {} },
      { sendFrame() {}, setPushHandler: "x" },
      "extensions",
    ]) {
      const err = await rejection(offlineClient({ voipBackend }));
      expect(err.name).toBe("WhatsAppError");
      expect(err.kind).toBe("invalid-argument");
      expect(err.field).toBe("extensions");
    }
  });

  test("a major mismatch rejects before any client exists", async () => {
    const plugin = fakePlugin({ major: ABI_MAJOR + 1 });
    const err = await rejection(offlineClient({ voipBackend: plugin }));
    expect(err.name).toBe("WhatsAppError");
    expect(err.kind).toBe("invalid-argument");
    expect(err.field).toBe("extensions");
    expect(plugin.pushHandler).toBeNull();
  });

  test("a well-formed plugin installs and registers its push handler", async () => {
    const plugin = fakePlugin();
    const client = await offlineClient({ voipBackend: plugin });
    expect(client).toBeDefined();
    expect(plugin.pushHandler).not.toBeNull();
    await client.disconnect();
  });

  test("absent extensions build exactly as before", async () => {
    const client = await offlineClient();
    expect(client).toBeDefined();
    await client.disconnect();
  });
});
