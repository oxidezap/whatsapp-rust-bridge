/**
 * A bad argument reports as `invalid-argument`, not as `internal`.
 *
 * The two kinds ask different things of a host. `internal` says the bridge
 * broke and there is nothing the caller can do; `invalid-argument` names the
 * argument and says fix it. Getting the first where the second is true sends
 * a consumer looking for a bug that is theirs.
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

const USER = "5511999999999@s.whatsapp.net";
const GROUP = "120363000000000000@g.us";

describe("the bridge's own checks name the argument", () => {
  test("a jid that is not a group, on the two calls that require one", async () => {
    const client = await offlineClient();
    try {
      const set = await rejection(
        client.setGroupProfilePicture(USER, new Uint8Array(8))
      );
      expect(set.kind).toBe("invalid-argument");
      expect(set.field).toBe("groupJid");

      const remove = await rejection(client.removeGroupProfilePicture(USER));
      expect(remove.kind).toBe("invalid-argument");
      expect(remove.field).toBe("groupJid");
    } finally {
      client.free();
    }
  }, 20000);

  test("a server id that is not a number", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(
        client.newsletterReactMessage(GROUP, "not-a-number", "👍")
      );
      expect(error.kind).toBe("invalid-argument");
      expect(error.field).toBe("serverId");
    } finally {
      client.free();
    }
  }, 20000);

  test("an encryption type the wire has no name for", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(
        client.signalDecryptMessage(USER, "not-an-enc-type", new Uint8Array(16))
      );
      expect(error.kind).toBe("invalid-argument");
      expect(error.field).toBe("msgType");
    } finally {
      client.free();
    }
  }, 20000);

  test("a comment with no way to identify the post's author", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(
        client.sendCommentBytes(GROUP, { id: "MSG1", fromMe: false }, new Uint8Array(4))
      );
      expect(error.kind).toBe("invalid-argument");
      expect(error.field).toBe("parent_key");
    } finally {
      client.free();
    }
  }, 20000);
});

describe("muteChat", () => {
  test("a timestamp that would saturate the cast is refused", async () => {
    const client = await offlineClient();
    try {
      // `as i64` saturates, so without the check these arrive as `i64::MAX` and
      // pass every rule downstream — a mute roughly 292 million years out.
      for (const value of [Infinity, NaN, 1e30]) {
        const error = await rejection(client.muteChat(USER, value));
        expect(error.kind).toBe("invalid-argument");
        expect(error.field).toBe("muteUntil");
      }
    } finally {
      client.free();
    }
  }, 20000);

  test("a representable timestamp reaches the core's own rule", async () => {
    const client = await offlineClient();
    try {
      // A millisecond after the epoch is representable, so the bridge passes it
      // on, and the core rejects it for being in the past. That rejection is an
      // `AppStateError::InvalidRequest`, which used to reach JS as `internal`.
      const error = await rejection(client.muteChat(USER, 1));
      expect(error.kind).toBe("invalid-argument");
      expect(error.field).toBe("request");
      expect(error.message).toContain("timestamp");
    } finally {
      client.free();
    }
  }, 20000);
});
