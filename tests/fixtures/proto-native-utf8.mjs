import assert from "node:assert/strict";
import { decodeProto, proto } from "../../dist/index.js";

const input = new Uint8Array([
  0x0a, 6, 0xef, 0xbb, 0xbf, 0xc3, 0x28, 0x41,
  0xfa, 0xff, 0x03, 0x80, 0x01,
  ...new Uint8Array(128),
]);
const expected = "\ufeff�(A";
const backing = Buffer.alloc(input.length + 14);
backing.set(input, 5);
const view = backing.subarray(5, 5 + input.length);
const plain = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
for (const bytes of [view, plain]) {
  assert.equal(decodeProto("Message", bytes).conversation, expected);
  assert.equal(proto.Message.decode(bytes).conversation, expected);
  const report = { invalidUtf8Fields: 0 };
  assert.equal(decodeProto("Message", bytes, report).conversation, expected);
  assert.equal(report.invalidUtf8Fields, 1);
  const incomplete = bytes.subarray(0, bytes.length - 1);
  assert.throws(() => decodeProto("Message", incomplete), /premature EOF/);
}

const dataView = new DataView(view.buffer, view.byteOffset, view.byteLength);
assert.equal(proto.Message.decode(dataView).conversation, expected);

const customized = new Uint8Array(input);
Object.defineProperty(customized, "subarray", {
  value(start, end) {
    if (start === 2 && end === 8) return new Uint8Array([0x7a]);
    return Uint8Array.prototype.subarray.call(this, start, end);
  },
});
assert.equal(decodeProto("Message", customized).conversation, "z");
assert.equal(proto.Message.decode(customized).conversation, "z");

const customSpecies = new Uint8Array(input);
customSpecies.constructor = {
  [Symbol.species]: class extends Uint8Array {
    constructor(buffer, byteOffset, length) {
      super(buffer, byteOffset, length);
      if (length === 6) return new Uint8Array([0x7a]);
    }
  },
};
assert.equal(decodeProto("Message", customSpecies).conversation, "z");
assert.equal(proto.Message.decode(customSpecies).conversation, "z");

const customBuffer = Buffer.from(input);
const spoofed = Buffer.alloc(customBuffer.length);
Object.defineProperty(customBuffer, "buffer", { value: spoofed.buffer });
Object.defineProperty(customBuffer, "byteOffset", { value: spoofed.byteOffset });
assert.equal(decodeProto("Message", customBuffer).conversation, "\0".repeat(6));

const storage = new ArrayBuffer(input.length, { maxByteLength: input.length + 19 });
const resizable = new Uint8Array(storage);
resizable.set(input);
assert.equal(decodeProto("Message", resizable).conversation, expected);
storage.resize(input.length - 1);
assert.throws(() => decodeProto("Message", resizable), /premature EOF/);

const shared = new Uint8Array(new SharedArrayBuffer(input.length));
shared.set(input);
assert.equal(decodeProto("Message", shared).conversation, expected);
