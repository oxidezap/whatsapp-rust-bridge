// Installed call-control WASM -> OZVP -> real media WASM -> synthetic opaque relay.
// Signaling and encryption callbacks are deterministic fixtures, NOT a Signal/auth/NAT test.
import assert from "node:assert/strict";
import { createCallControl } from "@oxidezap/whatsapp-rust-bridge";
import { loadVoip } from "@oxidezap/whatsapp-rust-bridge/voip";
import { createLocalRelay } from "./voip-local-relay.mjs";
import { createCallSignalingFixture } from "./voip-call-signaling.mjs";

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(predicate, label, timeout = 5_000) {
  const deadline = Date.now() + timeout;
  while (!predicate()) {
    if (Date.now() > deadline) throw Error(`timed out: ${label}`);
    await sleep(5);
  }
}
const peers = ["111111111111111@lid", "222222222222222@lid"];
const controls = [], events = [[], []], pcm = [[], []], pending = [new Map(), new Map()];
const counts = [{ packets: 0 }, { packets: 0 }];
const relay = createLocalRelay(index => counts[index].packets++);
const handles = [];
const signaling = createCallSignalingFixture({
  peers,
  deliver: (index, message) => controls[index].receiveNode(message),
  hasAck: (index, id) => pending[index].has(id),
  resolveAck: (index, ack) => pending[index].get(ack.attrs.id)?.(ack),
});
const { sent, errors: ingressErrors } = signaling;
let sequence = 0;

try {
  for (const index of [0, 1]) {
    const host = {
      ownLid: () => peers[index],
      ownPn: () => undefined,
      namespaceAlias: async () => undefined,
      learnIdentity: async () => { throw Error("fixture does not announce caller_pn"); },
      isConnected: () => true,
      generateRequestId: () => `fixture-${index}-${++sequence}`,
      resolvePeer: async peer => peer,
      // Snapshot because the bridge wipes its temporary plaintext after this callback.
      prepareKeys: async (peer, plaintext) => ({
        deviceKeys: [{ deviceJid: peer, ciphertext: plaintext.slice(), encType: "msg" }],
        ringDevices: [peer],
      }),
      decrypt: async (_peer, kind, ciphertext) => {
        assert.equal(kind, "msg");
        return ciphertext.slice();
      },
      readToken: async () => undefined,
      issueToken: async () => {},
      watchAck(id) {
        assert.equal(pending[index].has(id), false, "one observer per correlated request");
        let resolve;
        const promise = new Promise(ready => { resolve = ready; });
        pending[index].set(id, resolve);
        return { promise, cancel() { pending[index].delete(id); } };
      },
      sendNode: outgoing => signaling.send(index, outgoing),
    };
    const engine = loadVoip(relay.transport(index)).voipBackend;
    const control = await createCallControl(host, {
      onEvent(event) { events[index].push(event); },
      onCallEvent(event) { events[index].push({ type: "media", data: event }); },
      onCallPcm(frame) { pcm[index].push(frame.data.slice()); },
    }, { voipBackend: engine });
    controls.push(control);
  }

  const caller = await controls[0].dialCallPcm(peers[1]);
  handles.push(caller);
  await until(() => events[1].some(event => event.type === "incoming_call"), "incoming offer admitted");
  assert.deepEqual(ingressErrors, []);
  const callee = await controls[1].acceptCallPcm(caller.callId);
  handles.push(callee);
  assert.equal(callee.callId, caller.callId);
  await until(() => events.every(list => list.some(event => event.type === "media" && event.data.kind === "relay-allocated")), "both actual engines allocated");
  assert.equal(relay.activeConnections, 2);
  assert.equal(sent[1].filter(item => item.tag === "receipt" && item.content?.[0]?.tag === "offer").length, 1, "one distinct offer receipt");
  assert.equal(sent.flat().filter(item => item.tag === "ack").length, 0, "shared call control leaves generic ACKs to its host");
  await assert.rejects(caller.rejectVideo(), error =>
    error.kind === "invalid-argument" && /not supported by the pinned core/.test(error.message));
  assert.equal(sent.flat().filter(item => item.content?.some(child => child.tag === "video")).length, 0,
    "unsupported peer rejection must not send a local-stop video stanza");

  const samples = Int16Array.from({ length: 960 }, (_, i) => Math.round(6000 * Math.sin(i * Math.PI / 20)));
  for (let frame = 0; frame < 6; frame++) {
    for (const control of controls) assert.equal(control.callPushPcm16(caller.callId, samples), true);
    await sleep(60);
  }
  await until(() => pcm.every(frames => frames.some(frame => frame.some(sample => Math.abs(sample) > 100))), "two-way non-silent PCM");
  // Do not wait for a periodic stats push: close must flush the short call.
  assert.ok(counts.every(count => count.packets > 0));
  assert.deepEqual(ingressErrors, []);

  // Ending one side must deliver the terminal signal, close both relays, and keep final stats.
  const outcome = await controls[0].endCall(caller.callId);
  assert.equal(outcome.outcome, "peer-notified");
  await Promise.all(handles.map(handle => handle.waitEnded()));
  await until(() => relay.activeConnections === 0, "both relay handles closed");
  await until(() => events.every(list => list.some(event => event.type === "media" && event.data.kind === "ended")), "one terminal event per control");
  for (let index = 0; index < controls.length; index++) {
    const stats = controls[index].getCallMediaStats(caller.callId);
    assert.ok(stats.rtpReceived > 0, "final stats survive record teardown");
    const ended = events[index].filter(event => event.type === "media" && event.data.kind === "ended");
    assert.equal(ended.length, 1);
    assert.equal(ended[0].data.stats.rtpReceived, stats.rtpReceived, "ended carries the final snapshot");
    assert.equal(pending[index].size, 0, "ACK observers released");
  }

  // Disconnect while another actual media session is active, rather than only
  // proving disposal after an already-ended call.
  const priorOffers = events[1].filter(event => event.type === "incoming_call").length;
  const secondCaller = await controls[0].dialCallPcm(peers[1]);
  handles.push(secondCaller);
  await until(() => events[1].filter(event => event.type === "incoming_call").length > priorOffers, "second offer admitted");
  const secondCallee = await controls[1].acceptCallPcm(secondCaller.callId);
  handles.push(secondCallee);
  await until(() => relay.activeConnections === 2, "second actual session active");
  for (const control of controls) control.disconnect();
  await Promise.all([secondCaller, secondCallee].map(handle => handle.waitEnded()));
  await until(() => relay.activeConnections === 0 && pending.every(entries => entries.size === 0), "disconnect releases media and ACK observers");
  const receivedAtClose = pcm.map(frames => frames.length);
  await sleep(80);
  assert.deepEqual(pcm.map(frames => frames.length), receivedAtClose, "no media callback after disconnect");
} finally {
  for (const control of controls) control.disconnect();
  for (const handle of handles) handle.free();
  for (const control of controls) control.free();
}
assert.equal(relay.activeConnections, 0);
assert.deepEqual(ingressErrors, []);
console.log("standalone control delivered duplex PCM through both WASMs and released its hosts");
