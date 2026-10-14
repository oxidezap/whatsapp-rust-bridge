import type { JsStoreCallbacks } from "../../dist/index.js";

// These construction/teardown fixtures never commit patches or merge tokens.
// Reject unexpected use instead of pretending that a write was persisted.
export const unexercisedAtomicStorage = {
  async commitPatch() { throw new Error("atomic patch not exercised by this fixture"); },
  async touchTcTokenSenderTimestamp() { throw new Error("atomic token merge not exercised by this fixture"); },
  async storeReceivedTcToken() { throw new Error("atomic token merge not exercised by this fixture"); },
} satisfies Pick<JsStoreCallbacks, "commitPatch" | "touchTcTokenSenderTimestamp" | "storeReceivedTcToken">;
