/**
 * App-state actions the core implements: labels, quick replies, the link
 * preview preference, contact removal and status muting.
 *
 * Every one is a syncd mutation, so completing it needs a live connection and
 * no mock server runs in CI. What these cover is what does not need a peer: the
 * call reaches the core's app-state layer, the arguments arrive there intact
 * (proved by the core's own validation naming the offending one), and failures
 * come back as legible errors rather than `undefined`.
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
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

async function rejection(promise: Promise<unknown>): Promise<Error & { kind?: string }> {
  try {
    await promise;
  } catch (error) {
    return error as Error & { kind?: string };
  }
  throw new Error("expected the call to reject");
}

/**
 * How far an app-state mutation gets with no session: the core stops at the
 * sync key, after the action has been built. Reaching this proves the call
 * crossed and its arguments passed the core's validation.
 */
const REACHED_APP_STATE = "no app state sync key available";

const CHAT = "5511999999999@s.whatsapp.net";

describe("labels", () => {
  test("every label action reaches the core's app-state layer", async () => {
    const client = await offlineClient();
    try {
      const calls: Array<[string, Promise<unknown>]> = [
        ["createLabel", client.createLabel("1", "Leads", 3)],
        ["deleteLabel", client.deleteLabel("1")],
        ["addChatLabel", client.addChatLabel("1", CHAT)],
        ["removeChatLabel", client.removeChatLabel("1", CHAT)],
        ["addMessageLabel", client.addMessageLabel("1", CHAT, "MSG1")],
        ["removeMessageLabel", client.removeMessageLabel("1", CHAT, "MSG1")],
      ];

      for (const [name, call] of calls) {
        const error = await rejection(call);
        expect(error.message, name).toContain(REACHED_APP_STATE);
      }
    } finally {
      client.free();
    }
  }, 30000);

  test("the label id crosses: the core names it when it is empty", async () => {
    const client = await offlineClient();
    try {
      const emptyId = await rejection(client.createLabel("", "Leads", 3));
      expect(emptyId.message).toContain("label_id cannot be empty");

      const emptyName = await rejection(client.createLabel("1", "", 3));
      expect(emptyName.message).toContain("label name cannot be empty");

      const deleteEmpty = await rejection(client.deleteLabel(""));
      expect(deleteEmpty.message).toContain("label_id cannot be empty");
    } finally {
      client.free();
    }
  }, 20000);

  test("a malformed chat JID is rejected before the core is reached", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(client.addChatLabel("1", "not a jid"));
      expect(error.kind).toBe("invalid-argument");
      expect(error.message).not.toContain(REACHED_APP_STATE);
    } finally {
      client.free();
    }
  }, 20000);
});

describe("quick replies", () => {
  test("setQuickReply reaches the core with its keywords and count", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(
        client.setQuickReply("qr1", "/hi", "Hello there", ["greeting", "hi"], 0)
      );
      expect(error.message).toContain(REACHED_APP_STATE);
    } finally {
      client.free();
    }
  }, 20000);

  test("deleteQuickReply reaches the core", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(client.deleteQuickReply("qr1"));
      expect(error.message).toContain(REACHED_APP_STATE);
    } finally {
      client.free();
    }
  }, 20000);

  test("each quick-reply argument crosses, named back by the core's validation", async () => {
    const client = await offlineClient();
    try {
      const emptyId = await rejection(client.setQuickReply("", "/hi", "Hello", [], 0));
      expect(emptyId.message).toContain("quick reply id cannot be empty");

      const emptyShortcut = await rejection(
        client.setQuickReply("qr1", "", "Hello", [], 0)
      );
      expect(emptyShortcut.message).toContain("quick reply shortcut cannot be empty");

      const emptyMessage = await rejection(client.setQuickReply("qr1", "/hi", "", [], 0));
      expect(emptyMessage.message).toContain("quick reply message cannot be empty");

      // Proves `count` is carried rather than dropped on the way in.
      const negativeCount = await rejection(
        client.setQuickReply("qr1", "/hi", "Hello", [], -1)
      );
      expect(negativeCount.message).toContain("quick reply count cannot be negative");
    } finally {
      client.free();
    }
  }, 20000);
});

describe("account settings and contacts", () => {
  test("setLinkPreviewsDisabled reaches the core in both directions", async () => {
    const client = await offlineClient();
    try {
      for (const disabled of [true, false]) {
        const error = await rejection(client.setLinkPreviewsDisabled(disabled));
        expect(error.message).toContain(REACHED_APP_STATE);
      }
    } finally {
      client.free();
    }
  }, 20000);

  test("removeContact routes to the core's Remove, not to saveContact", async () => {
    const client = await offlineClient();
    try {
      const ok = await rejection(client.removeContact(CHAT));
      expect(ok.message).toContain(REACHED_APP_STATE);

      // The message names `remove_contact`, so this reached the syncd `Remove`
      // path and not the `Set` that `saveContact` builds.
      const lid = await rejection(client.removeContact("123456789@lid"));
      expect(lid.message).toContain("remove_contact");
      expect(lid.message).toContain("bare phone-number JID");
    } finally {
      client.free();
    }
  }, 20000);

  test("setUserStatusMute reaches the core in both directions", async () => {
    const client = await offlineClient();
    try {
      for (const muted of [true, false]) {
        const error = await rejection(client.setUserStatusMute(CHAT, muted));
        expect(error.message).toContain(REACHED_APP_STATE);
      }
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

  test("every new action is declared with the core's own shape", () => {
    expect(declarations).toContain(
      "createLabel(label_id: string, name: string, color: number): Promise<void>"
    );
    expect(declarations).toContain("deleteLabel(label_id: string): Promise<void>");
    expect(declarations).toContain(
      "addChatLabel(label_id: string, chat_jid: string): Promise<void>"
    );
    expect(declarations).toContain(
      "removeChatLabel(label_id: string, chat_jid: string): Promise<void>"
    );
    expect(declarations).toContain(
      "addMessageLabel(label_id: string, chat_jid: string, message_id: string): Promise<void>"
    );
    expect(declarations).toContain(
      "removeMessageLabel(label_id: string, chat_jid: string, message_id: string): Promise<void>"
    );
    expect(declarations).toContain("deleteQuickReply(id: string): Promise<void>");
    expect(declarations).toContain(
      "setLinkPreviewsDisabled(disabled: boolean): Promise<void>"
    );
    expect(declarations).toContain("removeContact(jid: string): Promise<void>");
    expect(declarations).toContain(
      "setUserStatusMute(jid: string, muted: boolean): Promise<void>"
    );
  });

  test("setQuickReply keeps its keyword list and count", () => {
    const declared = declarations.match(/setQuickReply\([^)]*\): Promise<void>/)?.[0];
    expect(declared).toBeTruthy();
    expect(declared).toContain("keywords: string[]");
    expect(declared).toContain("count: number");
  });

  test("no generic app-state escape hatch reaches JavaScript", () => {
    // Exposing `send_app_state_action` would push schema and mutation-index
    // knowledge across the boundary. Keep it out.
    expect(declarations).not.toContain("sendAppStateAction");
    expect(declarations).not.toContain("removeAppStateAction");
  });
});
