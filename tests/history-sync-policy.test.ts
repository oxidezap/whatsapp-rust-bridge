import { describe, test, expect, beforeAll } from "bun:test";
import { createWhatsAppClient, initWasmEngine } from "../dist/index.js";
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

describe("the history-sync policy construction argument", () => {
  test.each([1, "policy", [], true])(
    "a malformed policies value %p rejects before storage",
    async (value) => {
      const storageCalls: string[] = [];
      const store = {
        get: async () => {
          storageCalls.push("get");
          return null;
        },
        set: async () => {
          storageCalls.push("set");
        },
        delete: async () => {
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
          null,
          value as never
        )
      );
      expect(error.kind).toBe("invalid-argument");
      expect(error.field).toBe("policies");
      expect(storageCalls).toEqual([]);
    }
  );

  test("a malformed callback reports the policies argument", async () => {
    const error = await rejection(
      createWhatsAppClient(
        offlineTransport(),
        createHttp(),
        null,
        null,
        null,
        null,
        null,
        null,
        { historySyncAdmission: 1 } as never
      )
    );
    expect(error.kind).toBe("invalid-argument");
    expect(error.field).toBe("policies");
    expect(error.message).toContain("historySyncAdmission");
  });

  test("a throwing callback getter reports the policies argument", async () => {
    const error = await rejection(
      createWhatsAppClient(
        offlineTransport(),
        createHttp(),
        null,
        null,
        null,
        null,
        null,
        null,
        {
          get historySyncAdmission(): never {
            throw new Error("getter failure");
          },
        } as never
      )
    );
    expect(error.kind).toBe("invalid-argument");
    expect(error.field).toBe("policies");
    expect(error.message).toContain("historySyncAdmission");
  });

  test.each([undefined, null, {}])("accepted %p policies construct", async (policies) => {
    const client = await createWhatsAppClient(
      offlineTransport(),
      createHttp(),
      null,
      null,
      null,
      null,
      null,
      null,
      policies
    );
    try {
      expect(client.isConnected()).toBe(false);
    } finally {
      client.free();
    }
  });

  test("the existing eight-argument call remains valid", async () => {
    const client = await createWhatsAppClient(
      offlineTransport(),
      createHttp(),
      null,
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
  });
});
