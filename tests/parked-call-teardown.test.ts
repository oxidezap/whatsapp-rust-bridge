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
import type { WasmWhatsAppClient } from "../pkg/whatsapp_rust_bridge.js";

beforeAll(() => {
  initWasmEngine();
});

const CHAT = "5511999999999@s.whatsapp.net";

/** How long a parked call is watched before concluding it is holding. */
const HOLDS_BUDGET = 500;

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

/**
 * What `free()` threw, with the handle put back. The generated `free()` nulls
 * the JS handle before it calls in, so a refusal would otherwise strand the
 * client with no way to tear it down.
 */
function refusedFree(client: WasmWhatsAppClient): Error {
  const handle = client as unknown as { __wbg_ptr: number };
  const pointer = handle.__wbg_ptr;
  try {
    client.free();
  } catch (error) {
    handle.__wbg_ptr = pointer;
    return error as Error;
  }
  throw new Error("expected free() to be refused while a call was parked");
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

    const call = park(client);
    expect(await settlesWithin(call, HOLDS_BUDGET)).toBe(false);

    await client.disconnect();

    // `not-connected`, not `withdrawn`: the teardown releases what it is
    // holding, it does not withdraw it.
    expect((await rejection(call)).kind).toBe("not-connected");
    expect(store.counts.deviceReads).toBe(1);
  }, 20000);

  /**
   * And why that is safe. An exported `&self` method anchors the client for
   * the whole of its future, so `free()` is refused rather than dropping the
   * value a parked call is about to run against. That is the guarantee: not
   * that the call rejects, but that nothing it reaches has been released.
   */
  test("a parked call holds the client open, so free() cannot drop it", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    const call = park(client);
    expect(await settlesWithin(call, HOLDS_BUDGET)).toBe(false);

    expect(refusedFree(client).message).toMatch(/borrowed/);

    // Nothing was dropped: the same client answers the released call.
    await client.disconnect();
    expect((await rejection(call)).kind).toBe("not-connected");
    expect(store.counts.deviceReads).toBe(1);
  }, 20000);

  /**
   * Nothing bounds how many calls park, and a terminal state wakes all of
   * them at once. Each anchors on its own, so the count changes how much runs
   * after the teardown, not whether it has anything to run against.
   */
  test("every parked call holds it, however many are waiting", async () => {
    const store = countingStore();
    const client = await reconnectingClient(store);

    const calls = Array.from({ length: A_CROWD }, () => park(client));
    const held = await Promise.all(calls.map((call) => settlesWithin(call, HOLDS_BUDGET)));
    expect(held).toEqual(Array(A_CROWD).fill(false));

    expect(refusedFree(client).message).toMatch(/borrowed/);

    await client.disconnect();
    for (const call of calls) {
      expect((await rejection(call)).kind).toBe("not-connected");
    }
    expect(store.counts.deviceReads).toBe(A_CROWD);
  }, 20000);
});
