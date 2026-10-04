import { createWhatsAppClient, initWasmEngine } from "../../dist/index.js";

process.on("uncaughtException", (error) => {
  console.error(error);
  process.exit(1);
});

initWasmEngine();
let connectEntered!: () => void;
let releaseConnect!: () => void;
const connectCheckpoint = new Promise<void>((resolve) => {
  connectEntered = resolve;
});
const connectRelease = new Promise<void>((resolve) => {
  releaseConnect = resolve;
});
const client = await createWhatsAppClient(
  {
    connect: () => {
      connectEntered();
      return connectRelease;
    },
    send() {},
    disconnect() {},
  },
  { execute: async () => ({ statusCode: 0, body: new Uint8Array() }) }
);

const pending = client.connect();
await connectCheckpoint;
client.free();
releaseConnect();
await pending.catch(() => {});
