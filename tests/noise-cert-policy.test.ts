/**
 * The eighth `createWhatsAppClient` argument selects Noise cert verification.
 *
 * Absent, null or false keeps strict verification; only an explicit `true`
 * opts the built client into the mock-testing bypass, for chains not rooted
 * in WhatsApp's issuer (the local mock signs with its own root). Anything else rejects
 * the construction as `invalid-argument` naming `dangerSkipCertChainVerify`,
 * before any storage callback fires. Needs no server: construction never
 * touches the network.
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { initWasmEngine, createWhatsAppClient } from "../dist/index.js";
import { createHttp } from "./helpers.js";

beforeAll(() => {
  initWasmEngine();
});

function offlineTransport() {
  return { connect() {}, send() {}, disconnect() {} };
}

async function rejection(promise: Promise<unknown>): Promise<Error & { kind?: string; field?: string }> {
  try {
    await promise;
  } catch (error) {
    return error as Error & { kind?: string; field?: string };
  }
  throw new Error("expected the construction to reject");
}

describe("the Noise cert policy construction argument", () => {
  test.each([1, "true", {}, [], 0])(
    "a non-boolean %p rejects as invalid-argument",
    async (value) => {
      const storageCalls: string[] = [];
      const store = {
        get: async (...args: unknown[]) => {
          storageCalls.push("get");
          return null;
        },
        set: async (...args: unknown[]) => {
          storageCalls.push("set");
        },
        delete: async (...args: unknown[]) => {
          storageCalls.push("delete");
        },
      };
      const error = await rejection(
        createWhatsAppClient(
          offlineTransport(),
          createHttp(),
          null,
          store,
          null,
          null,
          null,
          value as boolean
        )
      );
      expect(error.kind).toBe("invalid-argument");
      expect(error.field).toBe("dangerSkipCertChainVerify");
      expect(storageCalls).toEqual([]);
    },
    20000
  );

  test.each([undefined, null, false, true])(
    "an accepted %p policy value constructs successfully",
    async (value) => {
      const store = {
        get: async () => {
          return null;
        },
        set: async () => {},
        delete: async () => {},
      };
      const client = await createWhatsAppClient(
        offlineTransport(),
        createHttp(),
        null,
        store,
        null,
        null,
        null,
        value as boolean | null | undefined
      );
      try {
        expect(client.isConnected()).toBe(false);
      } finally {
        client.free();
      }
    },
    20000
  );

  test("the existing seven-argument form still constructs", async () => {
    // Full seven-argument call: the eighth parameter stays optional, and
    // the strict default is proven by the parser unit plus the mock
    // rejection test, not by this construction succeeding.
    const client = await createWhatsAppClient(
      offlineTransport(),
      createHttp(),
      null,
      null,
      null,
      null,
      null
    );
    try {
      expect(client.isConnected()).toBe(false);
    } finally {
      client.free();
    }
  }, 20000);
});
