/**
 * What "giving up" on a call can mean.
 *
 * A call that lands in a reconnect is held at the gate until the reconnect
 * settles. wasm-bindgen drives an exported async method to completion whether
 * or not JS still holds its promise, so a host that races the promise against
 * a deadline bounds its own waiting and not the call: the abandoned call is
 * still parked, and it goes out when the reconnect lands. A host that then
 * re-issues has made one request twice.
 *
 * No mock server runs in CI, so nothing here proves what left the machine. The
 * effect these tests read instead is the core's own first move on this path: a
 * `device_list` read from the store, which happens once the call is past the
 * gate and not before. Counting it says whether the call reached the core.
 */

import { describe, test, expect, beforeAll, afterEach } from "bun:test";
import { initWasmEngine, createWhatsAppClient, encodeProto } from "../dist/index.js";
import { createHttp } from "./helpers.js";
import { aCallThatDoesNotPark, watch } from "./parked-calls.js";
import type { WasmWhatsAppClient } from "../pkg/whatsapp_rust_bridge.js";

beforeAll(() => {
  initWasmEngine();
});

const CHAT = "5511999999999@s.whatsapp.net";

/** How long a call that does not park is given to come back. */
const FAILS_FAST_BUDGET = 5000;

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

/**
 * A store that answers nothing and counts what was asked of it. `deviceReads`
 * is the marker: the core reads `device_list` as its first move once a send
 * path is past the gate.
 */
function countingStore() {
  const kept = new Map<string, Uint8Array>();
  const counts = { deviceReads: 0 };
  return {
    counts,
    callbacks: {
      async get(store: string, key: string) {
        if (store === "device_list") counts.deviceReads++;
        return kept.get(`${store}/${key}`) ?? null;
      },
      async set(store: string, key: string, value: Uint8Array) {
        kept.set(`${store}/${key}`, value);
      },
      async delete(store: string, key: string) {
        kept.delete(`${store}/${key}`);
      },
    },
  };
}

let live: WasmWhatsAppClient[] = [];

afterEach(async () => {
  const clients = live;
  live = [];
  for (const client of clients) {
    try {
      await client.disconnect();
      client.free();
    } catch {
      // A test that failed early can leave a client half-torn-down; the next
      // test gets a fresh one either way.
    }
  }
});

async function reconnectingClient(store: ReturnType<typeof countingStore>) {
  const client = await createWhatsAppClient(
    failingTransport(),
    createHttp(),
    null,
    store.callbacks as never
  );
  live.push(client);
  client.run();
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

/** A parked call, and the bytes it would have sent. */
function park(client: WasmWhatsAppClient) {
  const bytes = encodeProto("Message", { conversation: "hi" });
  return client.createParticipantNodesBytes([CHAT], bytes, null);
}

describe("giving up on a parked call", () => {
  /**
   * The cost, stated as a test. Abandoning the promise is not withdrawal and
   * this does not change that — it is what makes an explicit withdrawal worth
   * having, and it should keep passing after one exists.
   */
  test("abandoning the promise and re-issuing makes one request twice", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    const abandoned = watch(park(client));
    await aCallThatDoesNotPark(client, CHAT);
    expect(abandoned.settled()).toBe(false);
    expect(store.counts.deviceReads).toBe(0);

    // The host concluded it failed, and asked again.
    const reissued = park(client);

    await client.disconnect();
    await Promise.allSettled([abandoned.promise, reissued]);

    // Two calls reached the core for one intention.
    expect(store.counts.deviceReads).toBe(2);
  }, 20000);

  test("a withdrawn call never reaches the core", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    const call = watch(park(client));
    await aCallThatDoesNotPark(client, CHAT);
    expect(call.settled()).toBe(false);

    // And the count says where it is: at the gate, not merely slow.
    expect(client.withdrawParkedCalls()).toBe(1);
    expect((await rejection(call.promise)).kind).toBe("withdrawn");

    // The absence is the point: the core never got as far as its first read.
    expect(store.counts.deviceReads).toBe(0);
  }, 20000);

  test("withdrawing and re-issuing makes one request once", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    const abandoned = watch(park(client));
    await aCallThatDoesNotPark(client, CHAT);
    expect(abandoned.settled()).toBe(false);

    // Giving up, said out loud.
    expect(client.withdrawParkedCalls()).toBe(1);
    expect((await rejection(abandoned.promise)).kind).toBe("withdrawn");

    const reissued = park(client);
    await client.disconnect();
    await Promise.allSettled([reissued]);

    expect(store.counts.deviceReads).toBe(1);
  }, 20000);

  test("withdrawing releases what is parked and nothing else", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    const first = watch(park(client));
    const second = watch(park(client));
    await aCallThatDoesNotPark(client, CHAT);
    expect([first.settled(), second.settled()]).toEqual([false, false]);

    expect(client.withdrawParkedCalls()).toBe(2);
    expect((await rejection(first.promise)).kind).toBe("withdrawn");
    expect((await rejection(second.promise)).kind).toBe("withdrawn");

    // Not a mode: the next call parks like any other.
    const later = watch(park(client));
    await aCallThatDoesNotPark(client, CHAT);
    expect(later.settled()).toBe(false);
    expect(client.withdrawParkedCalls()).toBe(1);
    expect((await rejection(later.promise)).kind).toBe("withdrawn");
  }, 20000);

  test("withdrawing with nothing parked releases nothing", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    expect(client.withdrawParkedCalls()).toBe(0);

    // And a call issued afterwards still parks.
    const call = watch(park(client));
    await aCallThatDoesNotPark(client, CHAT);
    expect(call.settled()).toBe(false);
    expect(client.withdrawParkedCalls()).toBe(1);
    expect((await rejection(call.promise)).kind).toBe("withdrawn");
  }, 20000);

  /**
   * A call whose wait ended on its own is no longer at the gate, so there is
   * nothing left of it to release.
   */
  test("a call released by the wait ending is no longer withdrawable", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    const call = watch(park(client));
    await aCallThatDoesNotPark(client, CHAT);
    expect(call.settled()).toBe(false);
    expect(store.counts.deviceReads).toBe(0);

    // A disconnect is terminal, which is what ends the wait.
    await client.disconnect();
    expect((await rejection(call.promise)).kind).toBe("not-connected");

    expect(client.withdrawParkedCalls()).toBe(0);
  }, 20000);

  /**
   * The count is the promise the host acts on: it says the call did not go
   * out, so asking again is safe. A wait that ends in the same turn as the
   * withdrawal must not take that back — and it is an ordinary turn, not a
   * narrow one, because `withdrawParkedCalls()` is synchronous and therefore
   * always runs between two polls of the parked call.
   */
  test("a counted call stays withdrawn even when the wait ended in the same turn", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    const call = watch(park(client));
    await aCallThatDoesNotPark(client, CHAT);
    expect(call.settled()).toBe(false);

    // End the wait and withdraw without yielding in between.
    const ending = client.disconnect();
    expect(client.withdrawParkedCalls()).toBe(1);
    await ending;

    expect((await rejection(call.promise)).kind).toBe("withdrawn");
    expect(store.counts.deviceReads).toBe(0);
  }, 20000);

  /**
   * The 19 calls that do not wait reach the core immediately, so there is
   * nothing at the gate to withdraw and their answer is unchanged.
   */
  test("a call that never parks is not withdrawable", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    const call = client.sendChatState(CHAT, "composing");
    expect(client.withdrawParkedCalls()).toBe(0);
    expect(await settlesWithin(call, FAILS_FAST_BUDGET)).toBe(true);
    expect((await rejection(call)).kind).toBe("not-connected");
  }, 20000);
});
