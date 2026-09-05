/**
 * Same-artifact Noise cert policy proof against the mock server.
 *
 * The mock signs with its own root, so its chain is not rooted in
 * WhatsApp's issuer: a default (strict) client must reject the handshake
 * at XEdDSA verification, while a client constructed with the explicit
 * testing opt-in completes it. TLS to the self-signed mock endpoint is
 * accepted only by the test transport (`rejectUnauthorized: false` plus
 * `NODE_TLS_REJECT_UNAUTHORIZED=0`); the Noise checks under test remain
 * production code.
 *
 * Needs the mock server (skipped in CI like the other E2E files):
 *   MOCK_SERVER_URL=wss://127.0.0.1:32768/ws/chat bun test tests/mock-noise-cert-policy.test.ts
 */

import { describe, test, expect, beforeAll } from "bun:test";
import WebSocket from "ws";
import {
  initWasmEngine,
  createWhatsAppClient,
  encodeProto,
  decodeProto,
  decodeMessageWireBatch,
  type WhatsAppEvent,
} from "../dist/index.js";
import {
  createTransport,
  createHttp,
  waitForEvent,
  autoScanQr,
} from "./helpers.js";

beforeAll(() => {
  initWasmEngine();
});

// The shared `mockServerReachable` probes the admin endpoint with fetch,
// which rejects the mock's self-signed TLS here. The handshake under test
// rides WebSocket, so gate on a WS probe with the same accept-self-signed
// transport option the tests themselves use.
async function mockWsReachable(timeoutMs = 2000): Promise<boolean> {
  const url = process.env.MOCK_SERVER_URL ?? "wss://127.0.0.1:8080/ws/chat";
  return new Promise((resolve) => {
    const done = (v: boolean) => resolve(v);
    const timer = setTimeout(() => done(false), timeoutMs);
    try {
      const ws = new WebSocket(url, { rejectUnauthorized: false });
      ws.on("open", () => {
        clearTimeout(timer);
        ws.close();
        done(true);
      });
      ws.on("error", () => {
        clearTimeout(timer);
        done(false);
      });
    } catch {
      clearTimeout(timer);
      done(false);
    }
  });
}

const hasMockServer = await mockWsReachable();

describe("Noise cert policy against the mock server", () => {
  test("a default client rejects the mock chain at XEdDSA verify", async () => {
    if (!hasMockServer) return;
    const client = await createWhatsAppClient(createTransport(), createHttp(), null);
    try {
      const error = await client.connect().then(
        () => {
          throw new Error("strict connect must reject the mock chain");
        },
        (e: Error) => e
      );
      expect(String(error.message)).toContain("intermediate signature failed XEdDSA verify");
      expect(client.isConnected()).toBe(false);
    } finally {
      await client.disconnect().catch(() => {});
      client.free();
    }
  }, 30000);

  test("an opted-in client pairs and exchanges a message", async () => {
    if (!hasMockServer) return;

    async function pairOne(name: string, withInbox: boolean) {
      const events: WhatsAppEvent[] = [];
      const inbox: string[] = [];
      // Message events cross only through onMessageBatch as wire bytes;
      // decode synchronously inside the call per the batch contract.
      const onEvent = withInbox
        ? {
            onEvent: (event: WhatsAppEvent) => {
              events.push(event);
            },
            onMessageBatch: (batch: unknown) => {
              const view = decodeMessageWireBatch(
                batch as Parameters<typeof decodeMessageWireBatch>[0]
              );
              for (let i = 0; i < view.infos.length; i++) {
                const payload = view.messageData.slice(
                  view.messageOffsets[i],
                  view.messageOffsets[i + 1]
                );
                const message = decodeProto("Message", payload) as {
                  conversation?: string;
                };
                if (typeof message.conversation === "string") {
                  inbox.push(message.conversation);
                }
              }
            },
          }
        : (event: WhatsAppEvent) => {
            events.push(event);
          };
      const client = await createWhatsAppClient(
        createTransport(name),
        createHttp(),
        onEvent as never,
        null,
        null,
        null,
        null,
        true
      );
      client.run();
      await Promise.all([autoScanQr(events), waitForEvent(events, "pair_success", 20000)]);
      await waitForEvent(events, "connected", 45000);
      const jid = (await client.getJid()) as string;
      expect(jid).toBeTruthy();
      return { client, events, jid, inbox };
    }

    const alice = await pairOne("alice", false);
    try {
      const bob = await pairOne("bob", true);
      try {
        const text = `Noise policy proof ${Date.now()}`;
        const bytes = encodeProto("Message", { conversation: text });
        const msgId = await alice.client.sendMessageBytes(bob.jid, bytes);
        expect(msgId).toBeTruthy();

        await new Promise<void>((resolve, reject) => {
          const deadline = Date.now() + 15000;
          const interval = setInterval(() => {
            if (bob.inbox.includes(text)) {
              clearInterval(interval);
              resolve();
            } else if (Date.now() > deadline) {
              clearInterval(interval);
              reject(new Error(`timed out waiting for message text; inbox: ${JSON.stringify(bob.inbox)}`));
            }
          }, 100);
        });
        expect(bob.inbox).toContain(text);
      } finally {
        await bob.client.disconnect().catch(() => {});
        bob.client.free();
      }
    } finally {
      await alice.client.disconnect().catch(() => {});
      alice.client.free();
    }
  }, 120000);
});
