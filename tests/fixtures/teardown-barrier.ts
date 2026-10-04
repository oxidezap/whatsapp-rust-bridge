import { createWhatsAppClient, initWasmEngine } from "../../dist/index.js";

process.on("uncaughtException", (error) => {
  console.error(error);
  process.exit(1);
});

initWasmEngine();
let disposed = false;
let postDisposeOperations = 0;
function observeOperation() {
  if (disposed) postDisposeOperations++;
}
const store = {
  async get() {
    observeOperation();
    return null;
  },
  async set() {
    observeOperation();
  },
  async delete() {
    observeOperation();
  },
};
const client = await createWhatsAppClient(
  { connect() {}, send() {}, disconnect() {} },
  { execute: async () => ({ statusCode: 0, body: new Uint8Array() }) },
  () => observeOperation(),
  store as never
);

await client.setInitialPushName("barrier");
await Promise.all([client.disconnect(), client.disconnect()]);
client.free();
disposed = true;

// Construction of an unrelated client must not be needed to drain any
// post-dispose teardown task from the first client.
const next = await createWhatsAppClient(
  { connect() {}, send() {}, disconnect() {} },
  { execute: async () => ({ statusCode: 0, body: new Uint8Array() }) }
);
await next.disconnect();
next.free();
if (postDisposeOperations !== 0) {
  throw new Error(`client activity after free: ${postDisposeOperations}`);
}
