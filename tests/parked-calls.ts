/**
 * Proving that a call is parked, without waiting on a clock.
 *
 * Three files ask the same question of a client mid-reconnect: is this call
 * being held at the gate? The answer used to be a timer: watch the promise for
 * 500ms and call it parked if it had not settled, a negative proved by
 * exhaustion. It costs wall clock on every run, it never rules out a call
 * that was merely slow, and shortening it weakens what it proves without
 * turning anything red.
 *
 * The proof here is an ordering instead. `aCallThatDoesNotPark` issues a call
 * the bridge deliberately does not hold and returns once the core has refused
 * it: a full trip to the gate and back, on the same single-threaded executor,
 * after the call under test was issued. A call still unsettled at that point is
 * unsettled because it parked. Every use pairs it with the other side of the
 * order, either `withdrawParkedCalls()` counting the call or a teardown letting
 * it into the core, so the claim is made twice from two directions.
 */

import { expect } from "bun:test";
import type { WasmWhatsAppClient } from "../pkg/whatsapp_rust_bridge.js";

/**
 * A call's outcome, readable without waiting on it. The handlers go on at issue
 * time, so `settled()` answers for every turn that has already run rather than
 * for a window someone picked.
 */
export function watch<T>(promise: Promise<T>) {
  let done = false;
  const watched = promise.then(
    (value) => {
      done = true;
      return value;
    },
    (error) => {
      done = true;
      throw error;
    }
  );
  // Several of these are read later, or deliberately not at all.
  watched.catch(() => {});
  return { promise: watched, settled: () => done };
}

/**
 * Returns once a call that is *not* held has been to the gate and back.
 *
 * A typing indicator is about the instant it was sent, so the core discards it
 * on a lost connection and the bridge lets it through `unwaited`, pinned by
 * "sendChatState still fails at once" in `reconnect-wait.test.ts`, which is
 * where this goes red first if that ever changes. It reaches no store, so a
 * counting store beside it stays at zero.
 */
export async function aCallThatDoesNotPark(client: WasmWhatsAppClient, chat: string) {
  let kind: string | undefined = "(resolved)";
  try {
    await client.sendChatState(chat, "composing");
  } catch (error) {
    kind = (error as Error & { kind?: string }).kind;
  }
  expect(kind).toBe("not-connected");
}
