/**
 * The newsletter operations the core implements.
 *
 * CI has no mock server, so what these cover is what needs no peer: each call
 * reaches the core, the arguments arrive intact (proved wherever the core's own
 * guards name the one they rejected), and failures come back typed rather than
 * as `undefined`.
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

const CHANNEL = "123456789@newsletter";
const USER = "5511999999999@s.whatsapp.net";

describe("newsletter reads", () => {
  test("every read reaches the core", async () => {
    const client = await offlineClient();
    try {
      const calls: Array<[string, Promise<unknown>]> = [
        ["newsletterList", client.newsletterList()],
        ["newsletterMetadataByInvite", client.newsletterMetadataByInvite("abc123")],
        ["newsletterMessages", client.newsletterMessages(CHANNEL, 10)],
        ["newsletterFollowers", client.newsletterFollowers(CHANNEL, 50)],
        ["newsletterAdminInfo", client.newsletterAdminInfo(CHANNEL)],
      ];

      for (const [name, call] of calls) {
        const error = await rejection(call);
        expect(error.kind, name).toBe("not-connected");
      }
    } finally {
      client.free();
    }
  }, 30000);

  test("the message cursor is a serverId and is validated as one", async () => {
    const client = await offlineClient();
    try {
      // `before` crosses as a string because a serverId is a u64; a value that
      // is not one must be rejected here rather than silently truncated.
      const bad = await rejection(client.newsletterMessages(CHANNEL, 10, "not-a-number"));
      expect(bad.kind).toBe("invalid-argument");
      expect(bad.message).toContain("before");

      // A well-formed cursor gets past validation and on to the core.
      const ok = await rejection(
        client.newsletterMessages(CHANNEL, 10, "18446744073709551615")
      );
      expect(ok.kind).toBe("not-connected");
    } finally {
      client.free();
    }
  }, 20000);
});

describe("newsletter lifecycle and administration", () => {
  test("every mutation reaches the core", async () => {
    const client = await offlineClient();
    try {
      const calls: Array<[string, Promise<unknown>]> = [
        ["newsletterUpdate", client.newsletterUpdate(CHANNEL, "New name", null)],
        ["newsletterDelete", client.newsletterDelete(CHANNEL)],
        ["newsletterSetPicture", client.newsletterSetPicture(CHANNEL, new Uint8Array([1, 2, 3]))],
        ["newsletterRemovePicture", client.newsletterRemovePicture(CHANNEL)],
        ["newsletterSubscribeLiveUpdates", client.newsletterSubscribeLiveUpdates(CHANNEL)],
      ];

      for (const [name, call] of calls) {
        const error = await rejection(call);
        expect(error.kind, name).toBe("not-connected");
      }
    } finally {
      client.free();
    }
  }, 30000);

  test("admin targets go through the core's LID resolution", async () => {
    const client = await offlineClient();
    try {
      // The server addresses newsletter admins only by LID, so the core
      // resolves a phone-number JID first and fails when it cannot. Reaching
      // this message proves the target crossed and that resolution ran.
      const owner = await rejection(client.newsletterChangeOwner(CHANNEL, USER));
      expect(owner.message).toContain("no known LID for newsletter admin");

      const demote = await rejection(client.newsletterDemoteAdmin(CHANNEL, USER));
      expect(demote.message).toContain("no known LID for newsletter admin");
    } finally {
      client.free();
    }
  }, 20000);

  test("a malformed JID is rejected before the core is reached", async () => {
    const client = await offlineClient();
    try {
      const error = await rejection(client.newsletterAdminInfo("not a jid"));
      expect(error.kind).toBe("invalid-argument");
    } finally {
      client.free();
    }
  }, 20000);
});

describe("newsletter muting", () => {
  test("follower and admin mutes are separate entry points", async () => {
    const client = await offlineClient();
    try {
      for (const call of [
        client.newsletterFollowerMute(CHANNEL, true),
        client.newsletterAdminMute(CHANNEL, true),
        client.newsletterMute(CHANNEL, true),
      ]) {
        const error = await rejection(call);
        expect(error.kind).toBe("not-connected");
      }
    } finally {
      client.free();
    }
  }, 20000);
});

describe("newsletter messages", () => {
  test("edit and revoke refuse a JID that is not a channel", async () => {
    const client = await offlineClient();
    try {
      // Proves these route to the core's newsletter methods and not to the
      // DM/group edit and revoke, which accept this JID.
      const edit = await rejection(
        client.newsletterEditMessage(USER, "MSG1", new Uint8Array([]))
      );
      expect(edit.message).toContain("only valid for newsletter (channel) JIDs");

      const revoke = await rejection(client.newsletterRevokeMessage(USER, "MSG1"));
      expect(revoke.message).toContain("only valid for newsletter (channel) JIDs");
    } finally {
      client.free();
    }
  }, 20000);

  test("revoke requires a message id, and edit requires decodable bytes", async () => {
    const client = await offlineClient();
    try {
      const emptyId = await rejection(client.newsletterRevokeMessage(CHANNEL, ""));
      expect(emptyId.message).toContain("needs a target message_id");

      const badBytes = await rejection(
        client.newsletterEditMessage(CHANNEL, "MSG1", new Uint8Array([255, 255, 255]))
      );
      expect(badBytes.message).toContain("invalid message bytes");
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

  test("the mute ambiguity is resolved by name", () => {
    // The old `newsletterMute` said nothing about which of the core's two
    // controls it drove. Both are now spelled out; the old name stays.
    expect(declarations).toContain(
      "newsletterFollowerMute(jid: string, muted: boolean): Promise<void>"
    );
    expect(declarations).toContain(
      "newsletterAdminMute(jid: string, muted: boolean): Promise<void>"
    );
    expect(declarations).toContain(
      "newsletterMute(jid: string, muted: boolean): Promise<void>"
    );
  });

  test("every new operation is declared", () => {
    for (const declared of [
      "newsletterList(): Promise<NewsletterMetadataResult[]>",
      "newsletterMetadataByInvite(invite_code: string): Promise<NewsletterMetadataResult>",
      "newsletterDelete(jid: string): Promise<void>",
      "newsletterChangeOwner(jid: string, user: string): Promise<void>",
      "newsletterDemoteAdmin(jid: string, user: string): Promise<void>",
      "newsletterRemovePicture(jid: string): Promise<NewsletterMetadataResult>",
      "newsletterAdminInfo(jid: string): Promise<NewsletterAdminInfoResult>",
      "newsletterFollowers(jid: string, count: number): Promise<NewsletterFollowerResult[]>",
      "newsletterRevokeMessage(jid: string, message_id: string): Promise<void>",
    ]) {
      expect(declarations).toContain(declared);
    }
  });

  test("followers take a count and no cursor", () => {
    const followers = declarations.match(/newsletterFollowers\([^)]*\)/)?.[0];
    expect(followers).toBeTruthy();
    // The core's query has no cursor and the real client does not paginate
    // this list, so inventing one here would send it nowhere.
    expect(followers).not.toContain("before");
    expect(followers).not.toContain("after");
  });

  test("live updates carry the server's duration back", () => {
    expect(declarations).toContain(
      "newsletterSubscribeLiveUpdates(jid: string): Promise<number>"
    );
  });

  test("admin count is optional, never defaulted to zero", () => {
    const info = declarations.match(
      /export interface NewsletterAdminInfoResult \{[\s\S]*?\n\}/
    )?.[0];
    expect(info).toBeTruthy();
    expect(info).toContain("adminCount?");
  });

  test("a message keeps both of its ids, as strings", () => {
    const message = declarations.match(
      /export interface NewsletterMessageResult \{[\s\S]*?\n\}/
    )?.[0];
    expect(message).toBeTruthy();
    // serverId is a u64 — a number would lose it.
    expect(message).toContain("serverId: string");
    expect(message).toContain("messageId: string");
  });
});
