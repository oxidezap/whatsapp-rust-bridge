/**
 * Signaling-only call control: `rejectCall` and `terminateCall`.
 *
 * Both are fire-and-forget stanza sends, so completing one needs a live
 * connection and no mock server runs in CI. What these cover is what does not
 * need a peer: the call reaches the core, well-formed arguments pass the
 * bridge's own checks, and failures come back as typed `WhatsAppError`s
 * rather than a hang or `undefined`. Settling itself is covered for both
 * methods by the exported-surface sweep.
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

type CodedError = Error & { kind?: string; field?: string };

async function rejection(promise: Promise<unknown>): Promise<CodedError> {
  try {
    await promise;
  } catch (error) {
    return error as CodedError;
  }
  throw new Error("expected the call to reject");
}

const PEER = "5511999999999@s.whatsapp.net";
const CREATOR = "5511888888888@s.whatsapp.net";

describe("call signaling validation", () => {
  test("an empty call id names the argument, on both methods", async () => {
    const client = await offlineClient();
    try {
      // The core's own EmptyCallId, reached without a connection: neither
      // method parks behind a reconnect before validating.
      for (const call of [
        client.rejectCall("", PEER, CREATOR),
        client.terminateCall("", PEER, CREATOR),
      ]) {
        const error = await rejection(call);
        expect(error.kind).toBe("invalid-argument");
        expect(error.field).toBe("callId");
      }
    } finally {
      client.free();
    }
  });

  test("a malformed JID names which of the two arguments was wrong", async () => {
    const client = await offlineClient();
    try {
      const badPeer = await rejection(client.rejectCall("CALLID", "not-a-jid", CREATOR));
      expect(badPeer.kind).toBe("invalid-argument");
      expect(badPeer.field).toBe("peer");

      const badCreator = await rejection(
        client.terminateCall("CALLID", PEER, "not-a-jid")
      );
      expect(badCreator.kind).toBe("invalid-argument");
      expect(badCreator.field).toBe("callCreator");
    } finally {
      client.free();
    }
  });

  test("a well-formed stanza with nowhere to go reports not-connected", async () => {
    const client = await offlineClient();
    try {
      // ConnectionBound by design: a reject/terminate names a live call, so
      // it fails on a dead socket instead of waiting out a reconnect that may
      // already have ended the call.
      for (const call of [
        client.rejectCall("CALLID", PEER, CREATOR),
        client.terminateCall("CALLID", PEER, CREATOR),
      ]) {
        const error = await rejection(call);
        expect(error.kind).toBe("not-connected");
      }
    } finally {
      client.free();
    }
  });
});
