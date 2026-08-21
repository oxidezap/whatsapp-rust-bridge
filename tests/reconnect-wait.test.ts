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
  // Let `run()` reach its first failed attempt, so the client is between
  // connections rather than merely not started yet.
  await new Promise((resolve) => setTimeout(resolve, 150));
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

    const call = client.fetchPrivacySettings();
    expect(await settlesWithin(call, 500)).toBe(false);

    // Release it: a disconnect is terminal, which is what ends the wait.
    await client.disconnect();
    expect((await rejection(call)).kind).toBe("not-connected");
  }, 20000);

  test("a finished client is not waited on — it fails at once", async () => {
    const client = await reconnectingClient();
    await client.disconnect();

    const call = client.fetchPrivacySettings();
    expect(await settlesWithin(call, 300)).toBe(true);
    expect((await rejection(call)).kind).toBe("not-connected");
  }, 20000);
});
