// Public client -> both core queues -> OZVP -> isolated WASM -> local relay -> peer callback.
// Requires an explicitly selected loopback mock; never connects to WhatsApp.
import assert from "node:assert/strict";
import dgram from "node:dgram";
import { readFileSync } from "node:fs";
import { initWasmEngine, createWhatsAppClient } from "@oxidezap/whatsapp-rust-bridge";
import { loadVoip } from "@oxidezap/whatsapp-rust-bridge/voip";
const endpoint = new URL(process.env.MOCK_SERVER_URL);
assert.equal(endpoint.hostname, "127.0.0.1", "this fixture only uses the local mock");
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(predicate, label, timeout = 10_000) {
  const deadline = Date.now() + timeout;
  while (!predicate()) {
    if (Date.now() > deadline) throw Error(`timed out: ${label}`);
    await sleep(10);
  }
}
const annex = new Uint8Array(readFileSync(new URL("./voip-testsrc-idr.h264", import.meta.url)));
function idrPayload(annex) {
  const starts = [];
  for (let i = 0; i + 3 < annex.length; i++) {
    if (annex[i] || annex[i + 1]) continue;
    const n = annex[i + 2] === 1 ? 3 : annex[i + 2] === 0 && annex[i + 3] === 1 ? 4 : 0;
    if (n) { starts.push({ start: i, payload: i + n }); i += n - 1; }
  }
  const index = starts.findIndex(n => (annex[n.payload] & 31) === 5);
  return index < 0 ? undefined : annex.slice(starts[index].payload, starts[index + 1]?.start ?? annex.length);
}
const expectedIdr = idrPayload(annex);
assert.ok(expectedIdr);
const matchesIdr = frame => {
  const actual = idrPayload(frame.data);
  return actual && Buffer.from(actual).equals(Buffer.from(expectedIdr));
};
initWasmEngine();
const clients = [], sockets = new Set(), counts = [];
async function pair(index) {
  const events = [], received = [], count = { accepted: 0, forwarded: 0, rtpSent: 0, rtpReceived: 0 };
  counts.push(count);
  let active, old, scan, run;
  const engine = loadVoip({
    async connect(params, inbound) {
      assert.equal(params.address, "127.0.0.1", "mock relay must remain loopback");
      const socket = dgram.createSocket("udp4");
      sockets.add(socket);
      await new Promise(resolve => socket.bind(0, "127.0.0.1", resolve));
      socket.on("message", packet => { count.rtpReceived += Number((packet[0] & 0xc0) === 0x80 && packet[1] < 192); inbound.onPacket(packet); });
      socket.on("close", () => { sockets.delete(socket); inbound.onClose(); });
      queueMicrotask(() => inbound.onOpen());
      return { send(packet) { count.rtpSent += Number((packet[0] & 0xc0) === 0x80 && packet[1] < 192); socket.send(packet, Number(process.env.MOCK_RELAY_PORT ?? params.port), params.address); },
        async reconnect() { throw Error("unexpected reconnect"); }, close() { try { socket.close(); } catch {} } };
    },
  }).voipBackend;
  const client = await createWhatsAppClient({
    connect(handle) {
      old = active;
      const ws = active = new WebSocket(endpoint);
      ws.binaryType = "arraybuffer";
      ws.onmessage = message => { if (active === ws) handle.onData(new Uint8Array(message.data)); };
      ws.onclose = () => { if (active === ws) handle.onDisconnected(); };
      return new Promise((resolve, reject) => {
        ws.onopen = () => { handle.onConnected(); resolve(); };
        ws.onerror = reject;
      });
    },
    send(bytes) { active?.send(bytes); },
    disconnect() { (old ?? active)?.close(); old = undefined; },
  }, { async execute(url, method, headers, body) {
    assert.equal(new URL(url).hostname, "127.0.0.1", "no external HTTP in this fixture");
    const response = await fetch(url, { method, headers, body: body ?? undefined });
    return { statusCode: response.status, body: new Uint8Array(await response.arrayBuffer()) };
  } }, {
    onEvent(event) {
      events.push(event);
      if (event.type === "qr" && !scan) scan = fetch(new URL("/admin/mock-phone/scan-qr", endpoint.href.replace(/^ws/, "http")), { method: "POST", body: event.data.code });
    },
    onMessageBatch() {}, onHistorySyncBatch() {},
    onCallVideo(frame) { received.push(frame); },
    onCallEvent(event) { events.push({type: "media", data: event});  },
  }, null, null, null, null, true, null, { voipBackend: {
    setPushHandler: handler => engine.setPushHandler(handler),
    sendFrame(frame) { if (frame[6] === 0x14) count.forwarded++; return engine.sendFrame(frame); },
  } });
  const item = { client, received, events, count, close: async () => {
    client.setAutoReconnect(false);
    await client.disconnect();
    await run;
    active?.close();
    client.free();
  } };
  clients.push(item);
  run = client.run();
  await until(() => events.some(e => e.type === "connected"), `pair ${index}`);
  if (scan) assert.equal((await scan).ok, true);
  await client.waitForConnected(10000);
  await client.setPushName(`public-video-${index}`);
  return item;
}
let caller, callee;
try {
  const alice = await pair(0), bob = await pair(1);
  caller = await alice.client.dialCall(await bob.client.getLid() ?? await bob.client.getJid(), "mlow", true);
  await until(() => bob.events.some(e => e.type === "incoming_call" && e.data.action.type === "offer"), "incoming video offer");
  const offer = bob.events.find(e => e.type === "incoming_call" && e.data.action.type === "offer").data;
  assert.equal(offer.action.is_video, true);
  callee = await bob.client.acceptCall(offer.action.call_id, "mlow", true, offer.offerHandle);
  await until(() => clients.every(item => item.events.some(e => e.type === "media" && e.data.kind === "relay-allocated")), "both relays allocated");
  for (let i = 0; i < 30 && !(alice.received.some(matchesIdr) && bob.received.some(matchesIdr)); i++) {
    for (const item of clients) item.count.accepted += Number(item.client.callPushVideo(caller.callId, annex));
    await sleep(80);
  }
  for (const item of clients) {
    const matching = item.received.find(matchesIdr);
    assert.ok(item.count.forwarded > 0, `public input crossed the core pump: ${JSON.stringify(item.count)}`);
    assert.ok(item.count.rtpSent > 0 && item.count.rtpReceived > 0, "video RTP travels through the relay");
    assert.ok(matching, `peer callback contains the submitted IDR; stats=${JSON.stringify(item.client.getCallMediaStats(caller.callId))}`);
    assert.equal(matching.callId, caller.callId);
    assert.equal(matching.keyframe, true);
  }
  console.log("public callPushVideo delivered matching H.264 through both WASMs", JSON.stringify(counts));
} finally {
  await caller?.terminate().catch(() => {});
  await callee?.terminate().catch(() => {});
  for (const item of clients) await item.close();
  for (const socket of sockets) { try { socket.close(); } catch {} }
  caller?.free();
  callee?.free();
}
