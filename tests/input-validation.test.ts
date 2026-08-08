/**
 * Typed parameters reject a wrong shape instead of escaping the caller.
 *
 * These all used to be `#[tsify(from_wasm_abi)]`, whose generated
 * `FromWasmAbi` throws from inside the async shim. That throw is not a
 * rejection and not catchable at the call site: it surfaces as an uncaught
 * exception and the returned promise stays pending for good, so a host that
 * mistyped one field lost the call with no diagnostic it could attribute.
 *
 * Every case below would hang rather than fail if that behaviour came back.
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

async function rejection(promise: Promise<unknown>): Promise<Error & { kind?: string }> {
  try {
    await promise;
  } catch (error) {
    return error as Error & { kind?: string };
  }
  throw new Error("expected the call to reject");
}

const CHAT = "5511999999999@s.whatsapp.net";

describe("a mistyped parameter rejects", () => {
  test("a string enum given a number", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection((client as any).sendChatState(CHAT, 42));
      expect(error.kind).toBe("invalid-argument");
      expect(error.message).toContain("state");
    } finally {
      client.free();
    }
  }, 20000);

  test("an options object whose field has the wrong type", async () => {
    const client = await offlineClient();
    try {
      // `after` is a cursor string; a number is the case that used to hang.
      const error = await rejection(
        (client as any).getCollections(CHAT, { after: 42 })
      );
      expect(error.kind).toBe("invalid-argument");
      expect(error.message).toContain("options");
    } finally {
      client.free();
    }
  }, 20000);

  test("a struct given a primitive", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(
        (client as any).setBusinessCoverPhoto("not-an-object")
      );
      expect(error.kind).toBe("invalid-argument");
      expect(error.message).toContain("upload");
    } finally {
      client.free();
    }
  }, 20000);

  test("a list given a non-list", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection((client as any).readMessages("nope"));
      expect(error.kind).toBe("invalid-argument");
      expect(error.message).toContain("keys");
    } finally {
      client.free();
    }
  }, 20000);

  test("a list whose element is missing a required field", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection((client as any).readMessages([{}]));
      expect(error.kind).toBe("invalid-argument");
      expect(error.message).toContain("keys");
    } finally {
      client.free();
    }
  }, 20000);

  test("an unknown variant of a string enum", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(
        (client as any).downloadMedia("http://x", new Uint8Array(0), "not-a-media-type", 0)
      );
      expect(error.kind).toBe("invalid-argument");
      expect(error.message).toContain("media_type");
    } finally {
      client.free();
    }
  }, 20000);
});

describe("a well-formed parameter still crosses", () => {
  test("the valid shapes reach the core rather than being rejected here", async () => {
    const client = await offlineClient();
    try {
      // Each of these is the correct shape, so it must get past deserialization
      // and stop at the connection instead.
      const chatState = await rejection(client.sendChatState(CHAT, "composing"));
      expect(chatState.kind).not.toBe("invalid-argument");

      const options = await rejection(
        client.getCollections(CHAT, { after: "cursor-1", itemLimit: 3 })
      );
      expect(options.kind).toBe("not-connected");

      const omitted = await rejection(client.getCollections(CHAT));
      expect(omitted.kind).toBe("not-connected");

      const keys = await rejection(
        client.readMessages([{ remoteJid: CHAT, id: "MSG1" }])
      );
      expect(keys.kind).not.toBe("invalid-argument");
    } finally {
      client.free();
    }
  }, 30000);
});
