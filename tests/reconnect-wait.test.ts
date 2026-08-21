/**
 * What a host can tell about a call that was refused.
 *
 * The core reconnects on its own, so between two sockets there is a window in
 * which the client is alive and momentarily unreachable. A call landing in it
 * came back with the same `kind` a shut-down client produces, and the host had
 * no way to tell "this comes back in a moment" from "this is over" — the
 * difference between waiting a few seconds and restarting a process for
 * nothing.
 *
 * No mock server runs in CI, so nothing here proves a response body. What it
 * proves is which calls the bridge holds and which it lets through: the
 * transport below never completes a connection, so the client sits in its
 * reconnect loop for the whole test.
 */

import { describe, test, expect, beforeAll, afterEach } from "bun:test";
import { initWasmEngine, createWhatsAppClient } from "../dist/index.js";
import { createHttp } from "./helpers.js";
import type { WasmWhatsAppClient } from "../pkg/whatsapp_rust_bridge.js";

beforeAll(() => {
  initWasmEngine();
});

const CHAT = "5511999999999@s.whatsapp.net";

/**
 * How long a call that does not wait is given to come back. Generous on
 * purpose: a call that waits never settles at all, so the two are told apart by
 * whether it settles rather than by how fast, and a slow worker cannot turn one
 * into the other.
 */
const FAILS_FAST_BUDGET = 5000;

/** How long a call that waits is watched before concluding it is holding. */
const HOLDS_BUDGET = 500;

/** Never completes a connection, so `run()` stays in its reconnect backoff. */
function failingTransport() {
  return {
    connect() {
      return Promise.reject(new Error("no route in this test"));
    },
    send() {},
    disconnect() {},
  };
}

let live: WasmWhatsAppClient[] = [];

afterEach(async () => {
  for (const client of live) {
    await client.disconnect();
    client.free();
  }
  live = [];
});

/** A client whose supervision loop is running and will never reach a server. */
async function reconnectingClient(): Promise<WasmWhatsAppClient> {
  const client = await createWhatsAppClient(failingTransport(), createHttp());
  live.push(client);
  client.run();
  // Wait on the state rather than on a duration: `run()` reaching its first
  // failed attempt is what puts the client between connections, and how long
  // that takes is the worker's business.
  const deadline = Date.now() + 10000;
  while (client.reachability() !== "reconnecting") {
    if (Date.now() > deadline) {
      throw new Error(`client never started reconnecting (${client.reachability()})`);
    }
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  return client;
}

/** Whether `promise` settles inside `ms`, without leaving it unhandled. */
async function settlesWithin(promise: Promise<unknown>, ms: number): Promise<boolean> {
  let settled = false;
  const watched = promise.then(
    () => {
      settled = true;
    },
    () => {
      settled = true;
    }
  );
  await Promise.race([watched, new Promise((resolve) => setTimeout(resolve, ms))]);
  return settled;
}

async function rejection(promise: Promise<unknown>): Promise<Error & { kind?: string }> {
  try {
    await promise;
  } catch (error) {
    return error as Error & { kind?: string };
  }
  throw new Error("expected the call to reject");
}

describe("the reconnect window", () => {
  test("a call that a reconnect would answer waits for it", async () => {
    const client = await reconnectingClient();
    expect(client.reachability()).toBe("reconnecting");

    const call = client.fetchPrivacySettings();
    expect(await settlesWithin(call, HOLDS_BUDGET)).toBe(false);

    // Release it: a disconnect is terminal, which is what ends the wait.
    await client.disconnect();
    expect((await rejection(call)).kind).toBe("not-connected");
  }, 20000);

  test("a finished client is reported, not waited on", async () => {
    const client = await reconnectingClient();
    await client.disconnect();
    expect(client.reachability()).toBe("finished");

    const call = client.fetchPrivacySettings();
    expect(await settlesWithin(call, FAILS_FAST_BUDGET)).toBe(true);
    expect((await rejection(call)).kind).toBe("not-connected");
  }, 20000);

  test("a client nothing is reading is reported, not waited on", async () => {
    const client = await createWhatsAppClient(failingTransport(), createHttp());
    live.push(client);
    expect(client.reachability()).toBe("unsupervised");

    const call = client.fetchPrivacySettings();
    expect(await settlesWithin(call, FAILS_FAST_BUDGET)).toBe(true);
    expect((await rejection(call)).kind).toBe("not-connected");
  }, 20000);

  test("waitUntilReachable reports what ended the wait", async () => {
    const client = await reconnectingClient();

    const waiting = client.waitUntilReachable();
    expect(await settlesWithin(waiting, HOLDS_BUDGET)).toBe(false);

    await client.disconnect();
    expect(await waiting).toBe("finished");
  }, 20000);
});

describe("calls a reconnect cannot help", () => {
  /**
   * A typing indicator is about the instant it was sent, and the core discards
   * it on a lost connection rather than retrying it. Holding it would deliver
   * something the caller no longer means. Moving it to the waiting side hangs
   * this test rather than changing its answer.
   */
  test("sendChatState still fails at once", async () => {
    const client = await reconnectingClient();

    const call = client.sendChatState(CHAT, "composing");
    expect(await settlesWithin(call, FAILS_FAST_BUDGET)).toBe(true);
    expect((await rejection(call)).kind).toBe("not-connected");
  }, 20000);

  /**
   * The bridge is handed an opaque node here and cannot tell an IQ that
   * survives a reconnect from an ack that does not, so it does not guess. A
   * caller that knows waits for itself with `waitUntilReachable()`.
   */
  test("queryNode still fails at once", async () => {
    const client = await reconnectingClient();

    const call = client.queryNode(
      { tag: "iq", attrs: { type: "get", to: "s.whatsapp.net", xmlns: "w" } },
      500
    );
    expect(await settlesWithin(call, FAILS_FAST_BUDGET)).toBe(true);
    expect((await rejection(call)).kind).toBe("not-connected");
  }, 20000);

  /**
   * A media transfer routes through the core's media-conn cache, and only a
   * cold one issues an IQ. Holding the call would park an HTTP download that a
   * warm cache serves without touching the socket at all — so whether the
   * socket is needed stays the core's call, made per invocation.
   */
  test("getMediaConn still fails at once", async () => {
    const client = await reconnectingClient();

    const call = client.getMediaConn(false);
    expect(await settlesWithin(call, FAILS_FAST_BUDGET)).toBe(true);
    expect((await rejection(call)).kind).toBe("not-connected");
  }, 20000);

  /**
   * Signed-key rotation stages its candidate before the upload precisely so a
   * refusal can be re-issued without minting a new id — the core says as much
   * on the way out ("keeping the staged key, will retry on a later connect").
   * A wait here would park in front of a retry that already exists.
   */
  test("rotateSignedKey still fails at once", async () => {
    const client = await reconnectingClient();

    const call = client.rotateSignedKey();
    expect(await settlesWithin(call, FAILS_FAST_BUDGET)).toBe(true);
    expect((await rejection(call)).kind).toBe("not-connected");
  }, 20000);
});
