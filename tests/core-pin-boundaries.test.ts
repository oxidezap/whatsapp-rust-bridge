import { beforeAll, expect, test } from "bun:test";
import { createWhatsAppClient, initWasmEngine } from "../dist/index.js";
import { createHttp } from "./helpers.js";

beforeAll(() => initWasmEngine());

async function client() {
  return createWhatsAppClient({ connect() {}, send() {}, disconnect() {} }, createHttp());
}

async function rejects(call: () => unknown, field: string) {
  try {
    await call();
  } catch (error) {
    expect(error).toBeInstanceOf(Error);
    expect(error).toMatchObject({ name: "WhatsAppError", kind: "invalid-argument", field });
    return;
  }
  throw new Error("expected a typed input rejection");
}

test("typed message IDs reject empty IDs before entering the offline core", async () => {
  const value = await client();
  try {
    const chat = "100@s.whatsapp.net";
    await rejects(() => value.addMessageLabel("1", chat, ""), "messageId");
    await rejects(() => value.removeMessageLabel("1", chat, ""), "messageId");
    await rejects(() => value.starMessage(chat, "", true), "messageId");
    await rejects(() => value.deleteMessageForMe(chat, "", true), "messageId");
  } finally {
    value.free();
  }
});

test("media reupload validates its ID and key without sending a request", async () => {
  const value = await client();
  try {
    await rejects(() => value.requestMediaReupload("", "100@s.whatsapp.net", new Uint8Array(32), true), "msgId");
    await rejects(() => value.requestMediaReupload("ID", "100@s.whatsapp.net", new Uint8Array(31), true), "mediaKey");
  } finally {
    value.free();
  }
});

test("owned download preparation names the malformed metadata for both download APIs", async () => {
  const value = await client();
  try {
    for (const [keyLength, plainLength, encryptedLength, field] of [
      [31, 32, 32, "media_key"],
      [32, 31, 32, "file_sha256"],
      [32, 32, 31, "file_enc_sha256"],
    ] as const) {
      const args = ["/path", new Uint8Array(keyLength), new Uint8Array(plainLength), new Uint8Array(encryptedLength), 1, "image"] as const;
      await rejects(() => value.downloadMedia(...args), field);
      await rejects(() => value.downloadMediaStream(...args), field);
    }
  } finally {
    value.free();
  }
});
