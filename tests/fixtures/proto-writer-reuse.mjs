import assert from "node:assert/strict";
import { decodeProto, encodeProto, UnpairedSurrogateError } from "../../dist/index.js";

let reads = 0;
const image = {
  width: 1920,
  get caption() {
    // Normalization reads the same value twice. The next read lands inside
    // the encoder's open image fork.
    if (++reads > 2) {
      const nested = encodeProto("Message", { conversation: "inner" });
      assert.equal(decodeProto("Message", nested).conversation, "inner");
    }
    return "outer";
  },
};

encodeProto("Message", { conversation: "prime the spare writer" });
const retained = encodeProto("Message", { imageMessage: image });
assert.ok(reads > 2);

assert.throws(
  () => encodeProto("Message", { imageMessage: { width: 1920, caption: "\ud800" } }),
  UnpairedSurrogateError,
);
encodeProto("Message", { conversation: "x".repeat(8192) });
const next = encodeProto("Message", { conversation: "ok" });
assert.equal(Buffer.from(next).toString("hex"), "0a026f6b");
assert.deepEqual(decodeProto("Message", retained).imageMessage, { width: 1920, caption: "outer" });
