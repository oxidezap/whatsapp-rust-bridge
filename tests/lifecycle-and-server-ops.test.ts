/**
 * Lifecycle waits and server-side queries the core implements.
 *
 * No mock server runs in CI, so these cover the two things that do not need a
 * peer: the timeout contract of the waits, and that every new call crosses the
 * boundary and comes back as a typed `BridgeError` rather than `undefined`.
 * The declared shape of the results is asserted against the emitted `.d.ts`.
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { initWasmEngine, createWhatsAppClient } from "../dist/index.js";
import { createHttp } from "./helpers.js";

beforeAll(() => {
  initWasmEngine();
});

/** A client with a transport that never connects: every call below runs offline. */
async function offlineClient() {
  return createWhatsAppClient(
    { connect() {}, send() {}, disconnect() {} },
    createHttp()
  );
}

async function rejection(promise: Promise<unknown>): Promise<Error & { kind?: string }> {
  try {
    await promise;
  } catch (error) {
    return error as Error & { kind?: string };
  }
  throw new Error("expected the call to reject");
}

describe("lifecycle waits", () => {
  test("waitForSocket rejects with a timeout once the window elapses", async () => {
    const client = await offlineClient();
    try {
      const started = Date.now();
      const error = await rejection(client.waitForSocket(120));

      // The core turns expiry into ConnectError::Timeout, which the bridge maps
      // to the `timeout` kind. A boolean return would have lost this.
      expect(error.kind).toBe("timeout");
      expect(error.message).toBeTruthy();
      // It waited rather than failing fast on "not connected".
      expect(Date.now() - started).toBeGreaterThanOrEqual(100);
    } finally {
      client.free();
    }
  }, 20000);

  test("waitForConnected rejects with a timeout once the window elapses", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(client.waitForConnected(120));
      expect(error.kind).toBe("timeout");
    } finally {
      client.free();
    }
  }, 20000);

  test("the waits reject a timeout that is not a finite non-negative number", async () => {
    const client = await offlineClient();
    try {
      for (const bad of [-1, Number.NaN, Number.POSITIVE_INFINITY]) {
        const error = await rejection(client.waitForSocket(bad));
        expect(error.kind).toBe("invalid-argument");
      }
    } finally {
      client.free();
    }
  }, 20000);
});

describe("server-side operations", () => {
  test("cleanDirtyBits reaches the core and reports there is no connection", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(client.cleanDirtyBits("account_sync"));
      expect(error.kind).toBe("not-connected");
    } finally {
      client.free();
    }
  }, 20000);

  test("cleanDirtyBits carries an optional timestamp and validates both inputs", async () => {
    const client = await offlineClient();
    try {
      // A timestamp is accepted and still reaches the core.
      const withTimestamp = await rejection(
        client.cleanDirtyBits("groups", 1_700_000_000)
      );
      expect(withTimestamp.kind).toBe("not-connected");

      const emptyType = await rejection(client.cleanDirtyBits(""));
      expect(emptyType.kind).toBe("invalid-argument");

      const badTimestamp = await rejection(client.cleanDirtyBits("groups", -5));
      expect(badTimestamp.kind).toBe("invalid-argument");
    } finally {
      client.free();
    }
  }, 20000);

  test("an unknown dirty type is forwarded rather than rejected here", async () => {
    const client = await offlineClient();
    try {
      // The core's DirtyType has a wire fallback, so the bridge must not
      // second-guess the string: this has to fail at the connection, not on
      // validation.
      const error = await rejection(client.cleanDirtyBits("something_new"));
      expect(error.kind).toBe("not-connected");
    } finally {
      client.free();
    }
  }, 20000);

  test("getBotList reaches the core and reports there is no connection", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(client.getBotList());
      expect(error.kind).toBe("not-connected");
    } finally {
      client.free();
    }
  }, 20000);

  test("fetchNewChatMessageCappingInfo reaches the core and reports there is no connection", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(client.fetchNewChatMessageCappingInfo());
      expect(error.kind).toBe("not-connected");
    } finally {
      client.free();
    }
  }, 20000);
});

describe("emitted types", () => {
  const declarations = readFileSync(
    join(import.meta.dir, "..", "pkg", "whatsapp_rust_bridge.d.ts"),
    "utf8"
  );

  test("every new method is declared", () => {
    expect(declarations).toContain("waitForSocket(timeout_ms: number): Promise<void>");
    expect(declarations).toContain("waitForConnected(timeout_ms: number): Promise<void>");
    expect(declarations).toContain("getBotList(): Promise<BotListResult>");
    expect(declarations).toContain(
      "fetchNewChatMessageCappingInfo(): Promise<NewChatMessageCappingResult>"
    );
    expect(declarations).toContain("cleanDirtyBits(dirty_type: string");
  });

  test("the bot directory keeps its sections instead of being flattened", () => {
    const botList = declarations.match(/export interface BotListResult \{[^}]*\}/)?.[0];
    expect(botList).toBeTruthy();
    expect(botList).toContain("sections: BotListSectionResult[]");
    expect(botList).toContain("version: string");

    const section = declarations.match(/export interface BotListSectionResult \{[^}]*\}/)?.[0];
    expect(section).toBeTruthy();
    // Presentation metadata survives the crossing; a flattened list would not
    // carry it.
    expect(section).toContain("sectionType: string");
    expect(section).toContain("bots: BotListEntryResult[]");
  });

  test("the capping result carries the core's own remaining-quota derivation", () => {
    const capping = declarations.match(
      /export interface NewChatMessageCappingResult \{[^}]*\}/
    )?.[0];
    expect(capping).toBeTruthy();
    expect(capping).toContain("remainingQuota?");
    expect(capping).toContain("totalQuota?");
    expect(capping).toContain("usedQuota?");
  });
});
