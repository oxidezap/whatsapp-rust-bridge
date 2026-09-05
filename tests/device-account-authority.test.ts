/**
 * Device/account persistence failures must surface as `storage`, not `internal`.
 *
 * A missing-inline device record with a corrupt account sidecar makes the
 * backend `load` fail with a typed store error. `createWhatsAppClient` used
 * to wrap that in `internal`, hiding that the host's persisted data needs
 * repair. What this covers is the crossing, not the backend rule itself:
 * the rejection settles as a real `WhatsAppError` with `kind === 'storage'`
 * and the record context in its message.
 */

import { describe, test, expect, beforeAll, afterEach } from "bun:test";
import {
  initWasmEngine,
  createWhatsAppClient,
  type WasmWhatsAppClient,
} from "../dist/index.js";
import { createHttp } from "./helpers.js";

beforeAll(() => {
  initWasmEngine();
});

function mapStore() {
  const kept = new Map<string, Uint8Array>();
  return {
    kept,
    callbacks: {
      async get(store: string, key: string) {
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

function offlineTransport() {
  return { connect() {}, send() {}, disconnect() {} };
}

async function rejection(promise: Promise<unknown>) {
  try {
    await promise;
  } catch (error) {
    return error as Error & { kind?: string; operation?: string };
  }
  throw new Error("expected creation to reject");
}

let live: WasmWhatsAppClient[] = [];

async function track(client: WasmWhatsAppClient) {
  live.push(client);
  return client;
}

afterEach(async () => {
  const clients = live;
  live = [];
  for (const client of clients) {
    try {
      await client.disconnect();
      client.free();
    } catch {
      // A failed creation leaves nothing to tear down.
    }
  }
});

describe("device/account persistence failures", () => {
  test("creation over an empty store succeeds", async () => {
    const store = mapStore();
    const client = await track(
      await createWhatsAppClient(
        offlineTransport(),
        createHttp(),
        null,
        store.callbacks as never
      )
    );
    expect(client).toBeDefined();
    expect(store.kept.has("device/device")).toBe(true);
  });

  test("corrupt sidecar on a missing-inline record rejects as storage", async () => {
    const store = mapStore();
    await track(
      await createWhatsAppClient(
        offlineTransport(),
        createHttp(),
        null,
        store.callbacks as never
      )
    );

    const raw = store.kept.get("device/device");
    expect(raw).toBeDefined();
    const record = JSON.parse(new TextDecoder().decode(raw!));
    expect(record.account).toBeNull();
    delete record.account;
    expect("account" in record).toBe(false);
    store.kept.set(
      "device/device",
      new TextEncoder().encode(JSON.stringify(record))
    );
    store.kept.set(
      "device/account",
      new TextEncoder().encode("not-protobuf")
    );

    const error = await rejection(
      createWhatsAppClient(
        offlineTransport(),
        createHttp(),
        null,
        store.callbacks as never
      )
    );
    expect(error).toBeInstanceOf(Error);
    expect(error.name).toBe("WhatsAppError");
    expect(error.kind).toBe("storage");
    expect(error.message).toContain("device");
  });

  test("a store missing get still rejects as internal, not storage", async () => {
    const error = await rejection(
      createWhatsAppClient(offlineTransport(), createHttp(), null, {
        set() {},
        delete() {},
      } as never)
    );
    expect(error).toBeInstanceOf(Error);
    expect(error.name).toBe("WhatsAppError");
    expect(error.kind).toBe("internal");
  });
});
