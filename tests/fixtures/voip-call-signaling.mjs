// Synthetic call routing only: no sockets, authentication, Signal, or server proof.
import assert from "node:assert/strict";
const text = value => new TextEncoder().encode(value);
const node = (tag, attrs = {}, content) => ({ tag, attrs, ...(content === undefined ? {} : { content }) });
// Same synthetic relay grammar as core direct_tests::relay. Address is never dialed.
const relayNode = () => node("relay", {}, [
  node("key", {}, text("cmVsYXkta2V5")),
  node("warp_mi_tag_len", {}, text("4")),
  node("token", { id: "0" }, new Uint8Array(16).fill(0x41)),
  node("auth_token", { id: "1" }, new Uint8Array(16).fill(0x42)),
  node("te2", { relay_id: "1", relay_name: "test-relay", token_id: "0", auth_token_id: "1" },
    Uint8Array.of(203, 0, 113, 7, 0x0d, 0x96)),
]);

export function createCallSignalingFixture({ peers, deliver, hasAck, resolveAck }) {
  const sent = peers.map(() => []), errors = [];
  return {
    sent, errors,
    async send(index, outgoing) {
      sent[index].push(structuredClone(outgoing));
      if (outgoing.tag === "receipt") return;
      assert.equal(outgoing.tag, "call");
      const message = structuredClone(outgoing);
      message.attrs.from = peers[index];
      message.attrs.t = "1700000000";
      const offer = message.content.find(child => child.tag === "offer");
      if (offer) {
        assert.ok(hasAck(index, outgoing.attrs.id), "register ACK before transmitting the offer");
        message.content.push(relayNode());
      }
      // A socket write does not await the remote handler's transition lock.
      queueMicrotask(() => {
        Promise.resolve().then(() => deliver(1 - index, message)).catch(error => errors.push(error));
      });
      if (hasAck(index, outgoing.attrs.id)) resolveAck(index, node("ack", {
        id: outgoing.attrs.id, from: peers[1 - index], class: "call", type: message.content[0].tag,
      }, offer ? [relayNode()] : undefined));
    },
  };
}
