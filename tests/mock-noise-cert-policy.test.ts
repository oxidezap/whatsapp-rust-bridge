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
 *   NODE_TLS_REJECT_UNAUTHORIZED=0 MOCK_SERVER_URL=wss://127.0.0.1:32768/ws/chat \
 *     bun test tests/mock-noise-cert-policy.test.ts
 *
 * `NODE_TLS_REJECT_UNAUTHORIZED=0` covers only the test transport's TLS to
 * the self-signed mock endpoint (same as the existing E2E harness); the
 * Noise cert checks under test stay untouched.
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
// transport option the tests themselves use. The probe owns exactly one
// socket and one deadline: every path closes the socket and clears the
// deadline exactly once.
async function mockWsReachable(timeoutMs = 2000): Promise<boolean> {
  const url = process.env.MOCK_SERVER_URL ?? "wss://127.0.0.1:8080/ws/chat";
  return new Promise((resolve) => {
    let done = false;
    let ws: WebSocket | null = null;
    const finish = (v: boolean) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      try {
        ws?.terminate();
      } catch {
        // Terminating a half-open probe socket must not fail the gate.
      }
      resolve(v);
    };
    const timer = setTimeout(() => finish(false), timeoutMs);
    try {
      ws = new WebSocket(url, { rejectUnauthorized: false });
      ws.on("open", () => finish(true));
      ws.on("error", () => finish(false));
    } catch {
      finish(false);
    }
  });
}

// An explicitly supplied MOCK_SERVER_URL means the run intends to validate
// against the mock: an unreachable server is a visible failure, not a
// silent skip. Without it (CI), absence skips like the other E2E files.
const hasMockServer = await mockWsReachable();
if (process.env.MOCK_SERVER_URL && !hasMockServer) {
  throw new Error(
    `MOCK_SERVER_URL is set but the mock is unreachable: ${process.env.MOCK_SERVER_URL}`
  );
}

describe.skipIf(!hasMockServer)("Noise cert policy against the mock server", () => {
  test("a default client rejects the mock chain at XEdDSA verify", async () => {
    const client = await createWhatsAppClient(createTransport(), createHttp(), null);
    try {
      const error = (await client.connect().then(
        () => {
          throw new Error("strict connect must reject the mock chain");
        },
        (e: Error & { kind?: string }) => e
      )) as Error & { kind?: string };
      expect(error.kind).toBe("crypto");
      expect(String(error.message)).toContain("verify server Noise cert chain");
      expect(client.isConnected()).toBe(false);
    } finally {
      await client.disconnect().catch(() => {});
      client.free();
    }
  }, 30000);

  test("an opted-in client pairs and exchanges a message", async () => {
    // Every created client is owned by a finally from the moment it
    // exists: a pairing failure below must still free it, and a
    // disconnect failure must not mask the original error.
    async function pairOne(name: string) {
      const events: WhatsAppEvent[] = [];
      const inbox: string[] = [];
      // At most one outstanding waiter: set when the test starts awaiting
      // an incoming text, cleared on arrival or on timeout. No deadline
      // exists before that, so pairing/creation failures leave nothing
      // behind to fire.
      let pending: {
        text: string;
        done: () => void;
        timer: ReturnType<typeof setTimeout>;
      } | null = null;
      const waitForText = (text: string, timeoutMs = 15000) =>
        new Promise<void>((resolve, reject) => {
          if (inbox.includes(text)) {
            resolve();
            return;
          }
          const timer = setTimeout(() => {
            pending = null;
            reject(new Error(`timed out waiting for message text ${text}`));
          }, timeoutMs);
          const done = () => {
            clearTimeout(timer);
            pending = null;
            resolve();
          };
          pending = { text, done, timer };
        });
      // Message events cross only through onMessageBatch as wire bytes;
      // decode synchronously inside the call per the batch contract, and
      // settle a pending waiter on an exact match.
      const callbacks = {
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
              if (pending && message.conversation === pending.text) {
                pending.done();
              }
            }
          }
        },
      };
      const client = await createWhatsAppClient(
        createTransport(name),
        createHttp(),
        callbacks as never,
        null,
        null,
        null,
        null,
        true
      );
      try {
        client.run();
        await Promise.all([
          autoScanQr(events),
          waitForEvent(events, "pair_success", 20000),
        ]);
        await waitForEvent(events, "connected", 45000);
        const jid = (await client.getJid()) as string;
        expect(jid).toBeTruthy();
        return { client, events, jid, inbox, waitForText };
      } catch (error) {
        await client.disconnect().catch(() => {});
        client.free();
        throw error;
      }
    }

    const alice = await pairOne("alice");
    try {
      const bob = await pairOne("bob");
      try {
        const text = `Noise policy proof ${Date.now()}`;
        const bytes = encodeProto("Message", { conversation: text });
        const msgId = await alice.client.sendMessageBytes(bob.jid, bytes);
        expect(msgId).toBeTruthy();
        await bob.waitForText(text);
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
