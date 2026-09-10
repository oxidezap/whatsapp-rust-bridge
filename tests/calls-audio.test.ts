/**
 * Encoded-audio call media: accept, dial, push, stats, hangup, provider.
 *
 * Starting or driving a call needs a live connection and a peer, and no
 * mock server runs in CI. What these cover is what does not need either:
 * every method validates its arguments before touching the core, unknown
 * call ids are named as such, and failures come back as typed
 * `WhatsAppError`s rather than a hang or `undefined`. Settling itself is
 * covered for all of these by the exported-surface sweep.
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { initWasmEngine, createWhatsAppClient } from "../dist/index.js";
import type { WasmWhatsAppClient } from "../pkg/whatsapp_rust_bridge.js";
import { createHttp } from "./helpers.js";

beforeAll(() => {
  initWasmEngine();
});

async function offlineClient(): Promise<WasmWhatsAppClient> {
  return createWhatsAppClient(
    { connect() {}, send() {}, disconnect() {} },
    createHttp()
  );
}

type CodedError = Error & { kind?: string; field?: string };

async function rejection(promise: Promise<unknown>): Promise<CodedError> {
  try {
    await promise;
  } catch (error) {
    return error as CodedError;
  }
  throw new Error("expected the call to reject");
}

function syncRejection(fn: () => unknown): CodedError {
  try {
    fn();
  } catch (error) {
    return error as CodedError;
  }
  throw new Error("expected the call to throw");
}

const PEER = "5511999999999@s.whatsapp.net";

describe("call media validation", () => {
  test("accepting needs a live offer for the id", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(client.acceptCall("NEVER-RANG", "mlow"));
      expect(error.kind).toBe("invalid-argument");
      expect(error.field).toBe("callId");
    } finally {
      client.free();
    }
  });

  test("the audio format promise is required and validated before the gate", async () => {
    const client = await offlineClient();
    try {
      const badFormat = await rejection(
        client.acceptCall("NEVER-RANG", "g729" as "mlow")
      );
      expect(badFormat.kind).toBe("invalid-argument");
      expect(badFormat.field).toBe("audioFormat");

      // No bridge default: absence rejects instead of silently promising
      // MLOW against whatever the peer speaks.
      const absent = await rejection(
        client.acceptCall("NEVER-RANG", undefined as unknown as "mlow")
      );
      expect(absent.kind).toBe("invalid-argument");
      expect(absent.field).toBe("audioFormat");
    } finally {
      client.free();
    }
  });

  test("dialing names a malformed peer and reaches the core otherwise", async () => {
    const client = await offlineClient();
    try {
      const badPeer = await rejection(client.dialCall("not-a-jid", "mlow"));
      expect(badPeer.kind).toBe("invalid-argument");
      expect(badPeer.field).toBe("peer");

      // No session and no identity offline: the core's missing-LID error
      // proves the dial crossed with its arguments intact, and it reports
      // not-connected (pair first) rather than a bridge failure.
      const offline = await rejection(client.dialCall(PEER, "mlow"));
      expect(offline.kind).toBe("not-connected");
    } finally {
      client.free();
    }
  });

  test("push, stats, end and mute name an unknown call id", async () => {
    const client = await offlineClient();
    try {
      const push = syncRejection(() =>
        client.callPushAudio("NEVER-LIVE", new Uint8Array([1, 2, 3]))
      );
      expect(push.kind).toBe("invalid-argument");
      expect(push.field).toBe("callId");

      const empty = syncRejection(() =>
        client.callPushAudio("NEVER-LIVE", new Uint8Array(0))
      );
      expect(empty.kind).toBe("invalid-argument");
      expect(empty.field).toBe("data");

      const stats = syncRejection(() => client.getCallMediaStats("NEVER-LIVE"));
      expect(stats.kind).toBe("invalid-argument");
      expect(stats.field).toBe("callId");

      const end = await rejection(client.endCall("NEVER-LIVE"));
      expect(end.kind).toBe("invalid-argument");
      expect(end.field).toBe("callId");

      const mute = await rejection(client.setCallMuted("NEVER-LIVE", true));
      expect(mute.kind).toBe("invalid-argument");
      expect(mute.field).toBe("callId");

      expect(client.getActiveCalls()).toEqual([]);
    } finally {
      client.free();
    }
  });

  test("the relay provider names a missing constructor", async () => {
    const client = await offlineClient();
    try {
      const error = syncRejection(() =>
        client.setRelayTransportProvider({} as never)
      );
      expect(error.kind).toBe("invalid-argument");
      expect(error.field).toBe("provider");
    } finally {
      client.free();
    }
  });

  test("an unusable media callback rejects construction", async () => {
    try {
      await createWhatsAppClient(
        { connect() {}, send() {}, disconnect() {} },
        createHttp(),
        { onEvent() {}, onCallAudio: 42 } as never
      );
      throw new Error("expected construction to reject");
    } catch (error) {
      const coded = error as CodedError;
      expect(coded.kind).toBe("invalid-argument");
      expect(coded.field).toBe("on_event.onCallAudio");
    }
  });
});
