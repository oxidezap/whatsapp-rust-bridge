/**
 * E2E test: a paired client sends call-control stanzas to the mock server.
 *
 * `rejectCall` and `terminateCall` are fire-and-forget: resolving means the
 * stanza went out, not that a peer answered. The mock server implements no
 * call peer, so this proves the plumbing (pair, send, no error) rather than a
 * call outcome. An actual dial needs the media builders of a later slice and
 * a peer that rings, neither of which exists here.
 *
 * Prerequisites:
 *   - Mock server running on wss://127.0.0.1:8080/ws/chat
 *   - Bridge built: bun run build:dev
 *
 * Run: NODE_TLS_REJECT_UNAUTHORIZED=0 bun test tests/e2e-calls.test.ts
 */

import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import {
  initWasmEngine,
  createWhatsAppClient,
} from "../dist/index.js";
import type {
  WhatsAppEvent,
  WasmWhatsAppClient,
} from "../pkg/whatsapp_rust_bridge.js";
import {
  createTransport,
  createHttp,
  waitForEvent,
  autoScanQr,
  mockServerReachable,
} from "./helpers.js";

process.env.NODE_TLS_REJECT_UNAUTHORIZED = "0";

beforeAll(() => {
  initWasmEngine();
});

// Pairs against the mock server on MOCK_SERVER_URL, which CI does not start.
// Skipping keeps a missing server out of the build result; a real regression
// still shows up wherever it is running.
const hasMockServer = await mockServerReachable();

describe.skipIf(!hasMockServer)("E2E call signaling", () => {
  let client: WasmWhatsAppClient;
  let events: WhatsAppEvent[];

  beforeAll(async () => {
    events = [];
    // Mock opt-in, per client: the mock server cannot sign a chain rooted in
    // WhatsApp's issuer, so every client in this file names the testing
    // bypass at construction. Production callers keep the default.
    client = await createWhatsAppClient(
      createTransport("calls"),
      createHttp(),
      (event: WhatsAppEvent) => {
        events.push(event);
      },
      null,
      null,
      null,
      null,
      true
    );
    client.run();
    await Promise.all([
      autoScanQr(events),
      waitForEvent(events, "pair_success", 20000),
    ]);
    await waitForEvent(events, "connected", 45000);
  }, 90000);

  afterAll(async () => {
    await client.disconnect();
    client.free();
  });

  test("rejectCall sends without error", async () => {
    const peer = (await client.getJid())!;
    await client.rejectCall("CALLID-E2E-1", peer, peer);
  }, 30000);

  test("terminateCall sends without error", async () => {
    const peer = (await client.getJid())!;
    await client.terminateCall("CALLID-E2E-2", peer, peer);
  }, 30000);
});
