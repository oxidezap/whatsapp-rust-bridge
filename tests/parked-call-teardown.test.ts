import { unexercisedAtomicStorage } from "./fixtures/unexercised-atomic-storage.js";
/**
 * What a parked call can still touch once the client is torn down.
 *
 * A terminal state is one of the two things that ends a park, so tearing the
 * client down releases whatever is waiting at the gate — and `online()`
 * publishes no verdict of its own, so what it releases carries on into the
 * core. These ask what is left of the client by the time it gets there.
 *
 * No mock server runs in CI, so nothing here proves a response body. The
 * effect read instead is the core's own first move on this path: a
 * `device_list` read from the store, which happens once a call is past the
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

/** Enough parked calls that they cannot all be resumed by one poll. */
const A_CROWD = 8;

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
function countingStore(readGate?: Promise<void>) {
  const kept = new Map<string, Uint8Array>();
  const counts = { deviceReads: 0 };
  return {
    counts,
    callbacks: {
      ...unexercisedAtomicStorage,
      async get(store: string, key: string) {
        if (store === "device_list") {
          counts.deviceReads++;
          if (readGate) await readGate;
        }
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

// wasm-bindgen consumes the JS owner even when free() reports an active borrow.
// Pending calls retain their own Rust owners; never resurrect the private pointer.
function consumeWhileBorrowed(client: WasmWhatsAppClient): Error {
  live = live.filter((value) => value !== client);
  try {
    client.free();
  } catch (error) {
    return error as Error;
  }
  throw new Error("expected free() to report an active borrow");
}

describe("tearing down a client that is holding parked calls", () => {
  /**
   * The step the severity of this rests on. `online()` hands back the client
   * once the wait ends, whatever ended it, so a terminal state releases the
   * call *into* the core rather than failing it at the gate.
   */
  test("a parked call released by the teardown goes on into the core", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    const call = watch(park(client));
    await aCallThatDoesNotPark(client, CHAT);
    expect(call.settled()).toBe(false);
    expect(store.counts.deviceReads).toBe(0);

    await client.disconnect();

    // `not-connected`, not `withdrawn`: the teardown releases what it is
    // holding, it does not withdraw it.
    expect((await rejection(call.promise)).kind).toBe("not-connected");
    expect(store.counts.deviceReads).toBe(1);
  }, 20000);

  for (const count of [1, A_CROWD]) {
    test(`free() consumes the JS handle while ${count} held calls finish safely`, async () => {
      let release!: () => void;
      const readGate = new Promise<void>((resolve) => { release = resolve; });
      const store = countingStore(readGate);
      const client = await reconnectingClient(store);
      const calls = Array.from({ length: count }, () => watch(park(client)));
      await aCallThatDoesNotPark(client, CHAT);
      expect(calls.map((call) => call.settled())).toEqual(Array(count).fill(false));
      expect(store.counts.deviceReads).toBe(0);

      // Enrol teardown before giving up the JS owner, so cleanup needs no stale handle.
      const teardown = client.disconnect();
      const error = consumeWhileBorrowed(client);
      try {
        expect(error.message).toMatch(/borrowed/);
        expect(() => client.reachability()).toThrow(/null pointer/);
        await teardown;
        expect(store.counts.deviceReads).toBe(count);
        expect(calls.map((call) => call.settled())).toEqual(Array(count).fill(false));
      } finally {
        release();
        await teardown;
        await Promise.allSettled(calls.map((call) => call.promise));
      }
      for (const call of calls) {
        expect((await rejection(call.promise)).kind).toBe("not-connected");
      }
    }, 20000);
  }
});
