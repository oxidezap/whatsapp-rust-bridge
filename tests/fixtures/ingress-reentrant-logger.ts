import assert from "node:assert/strict";
import { createWhatsAppClient, initWasmEngine } from "../../dist/index.js";
import type { JsTransportHandle } from "../../pkg/whatsapp_rust_bridge.js";

const entered = Promise.withResolvers<JsTransportHandle>();
let handle: JsTransportHandle | undefined;
let overflowCount = 0;
initWasmEngine({
  level: "error",
  trace() {},
  debug() {},
  info() {},
  warn() {},
  error(_, message) {
    if (message?.includes("Transport ingress budget exceeded")) {
      assert.ok(handle);
      handle.onDisconnected();
      overflowCount++;
    }
  },
});

const client = await createWhatsAppClient({
  connect(handle) { entered.resolve(handle); },
  send() {},
  disconnect() {},
}, {
  async execute() { return { statusCode: 0, body: new Uint8Array() }; },
});
try {
  client.run();
  handle = await entered.promise;
  const empty = new Uint8Array();
  for (let index = 0; index <= 32_768; index++) handle.onData(empty);
  assert.equal(overflowCount, 1);
} finally {
  await client.disconnect();
  client.free();
}
