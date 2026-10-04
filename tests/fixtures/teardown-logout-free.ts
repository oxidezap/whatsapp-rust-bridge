import { createWhatsAppClient, initWasmEngine } from "../../dist/index.js";

process.on("uncaughtException", (error) => {
  console.error(error);
  process.exit(1);
});

initWasmEngine();
let logoutEntered!: () => void;
const logoutCheckpoint = new Promise<void>((resolve) => {
  logoutEntered = resolve;
});
const client = await createWhatsAppClient(
  { connect() {}, send() {}, disconnect() {} },
  { execute: async () => ({ statusCode: 0, body: new Uint8Array() }) },
  (event: { type?: string }) => {
    if (event.type === "logged_out") logoutEntered();
  }
);

// The event is dispatched by core logout before its final disconnect. Free
// only after logout has entered the core path, while its bridge promise is
// still pending.
const logout = client.logout();
await logoutCheckpoint;
client.free();
await logout;
