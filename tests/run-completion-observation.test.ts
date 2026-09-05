/**
 * Run supervision completion observation.
 *
 * `run()` starts the core supervision loop and returns immediately, so without
 * an observation export the host never learns why supervision ended: a
 * disconnect, a disabled-reconnect exit and a torn-down client all look the
 * same (no `Disconnected` event covers them). `waitForRunCompletion` carries
 * the core's completion reason across instead.
 *
 * No mock server runs in CI, so these cover what needs no peer: that the
 * observation export settles, that a late observer sees the same completion a
 * waiter saw, that a second `run()` changes nothing, and that freeing the
 * client settles a pending wait instead of leaving it hanging. Timeouts below
 * are test limits, never proof that an event did not happen: every positive
 * assertion awaits something that must settle first. Every teardown test
 * awaits the transport-entered gate first, so the run task provably began
 * before it is torn down.
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import WebSocket from "ws";
import {
  initWasmEngine,
  createWhatsAppClient,
  type WhatsAppEvent,
} from "../dist/index.js";
import {
  createTransport,
  createHttp,
  waitForEvent,
  autoScanQr,
} from "./helpers.js";

beforeAll(() => {
  initWasmEngine();
});

type Completion = {
  reason: string;
  generation: number;
  connection?: unknown;
  connectError?: { kind?: string; message?: string; [key: string]: unknown };
  protocolError?: unknown;
};

/** A transport that connects but never delivers bytes: the run loop stays up. */
function gatedTransport() {
  let entered!: () => void;
  const enteredGate = new Promise<void>((resolve) => {
    entered = resolve;
  });
  return {
    entered: enteredGate,
    connect() {
      entered();
    },
    send() {},
    disconnect() {},
  };
}

/** A transport whose open fails at once, plus HTTP that answers nothing. */
function failingTransport() {
  let entered!: () => void;
  const enteredGate = new Promise<void>((resolve) => {
    entered = resolve;
  });
  return {
    entered: enteredGate,
    connect() {
      entered();
      return Promise.reject(new Error("boom"));
    },
    send() {},
    disconnect() {},
  };
}

function failingHttp() {
  return {
    async execute() {
      return { statusCode: 0, body: new Uint8Array(0) };
    },
  };
}

/** Fails the test when `promise` does not settle inside the limit. */
function withLimit<T>(promise: Promise<T>, ms: number, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout>;
  const limit = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${what} did not settle in ${ms}ms`)), ms);
  });
  return Promise.race([promise, limit]).finally(() => clearTimeout(timer!));
}

describe("run completion observation", () => {
  test("waiting before run() rejects as the caller's own mistake", async () => {
    const client = await createWhatsAppClient(gatedTransport(), failingHttp());
    try {
      let error: (Error & { kind?: string; field?: string }) | null = null;
      try {
        await withLimit(client.waitForRunCompletion(), 5000, "wait before run()");
      } catch (e) {
        error = e as Error & { kind?: string; field?: string };
      }
      expect(error).not.toBeNull();
      expect(error!.kind).toBe("invalid-argument");
      expect(error!.field).toBe("waitForRunCompletion");
    } finally {
      client.free();
    }
  });

  test("a wait issued before run() keeps its before-run verdict", async () => {
    const transport = gatedTransport();
    const client = await createWhatsAppClient(transport, failingHttp());
    try {
      // No await between the two calls: both verdicts are taken in call
      // order, so the early wait must not observe the run it predates. The
      // noop handler marks the early rejection as observed while the test
      // holds it for the assertions below; the promise itself still rejects.
      const early = client.waitForRunCompletion();
      early.catch(() => {});
      client.run();
      await withLimit(transport.entered, 5000, "run task start");

      let earlyError: (Error & { kind?: string; field?: string }) | null = null;
      try {
        await withLimit(early, 5000, "early wait");
      } catch (e) {
        earlyError = e as Error & { kind?: string; field?: string };
      }
      expect(earlyError).not.toBeNull();
      expect(earlyError!.kind).toBe("invalid-argument");
      expect(earlyError!.field).toBe("waitForRunCompletion");

      // A wait issued after the run started observes the started run.
      const later = client.waitForRunCompletion();
      await withLimit(client.disconnect(), 5000, "disconnect()");
      const completion = (await withLimit(later, 5000, "later wait")) as unknown as Completion;
      expect(completion.reason).toBe("shutdown-requested");
      expect(completion.generation).toBe(0);
    } finally {
      client.free();
    }
  });

  test("starting observation does not block a concurrent disconnect", async () => {
    const transport = gatedTransport();
    const client = await createWhatsAppClient(transport, failingHttp());
    try {
      client.run();
      // The run task provably began: the core attempted its first connect.
      await withLimit(transport.entered, 5000, "run task start");
      const waiting = client.waitForRunCompletion();
      // disconnect() must settle while the observation is still pending: the
      // wait holds no borrow that stops another call from reaching the client.
      await withLimit(client.disconnect(), 5000, "disconnect()");
      const completion = (await withLimit(
        waiting,
        5000,
        "waitForRunCompletion()"
      )) as unknown as Completion;
      expect(completion.reason).toBe("shutdown-requested");
      expect(completion.generation).toBe(0);
    } finally {
      client.free();
    }
  });

  test("a late observer sees the completion the waiter saw", async () => {
    const transport = gatedTransport();
    const client = await createWhatsAppClient(transport, failingHttp());
    try {
      client.run();
      await withLimit(transport.entered, 5000, "run task start");
      const waiting = client.waitForRunCompletion();
      await withLimit(client.disconnect(), 5000, "disconnect()");
      const first = (await withLimit(waiting, 5000, "first wait")) as unknown as Completion;
      // Registered after the run already ended: the result outlives the task.
      const second = (await withLimit(
        client.waitForRunCompletion(),
        5000,
        "late wait"
      )) as unknown as Completion;
      expect(second).toEqual(first);
    } finally {
      client.free();
    }
  });

  test("simultaneous observers receive the same completion", async () => {
    const transport = gatedTransport();
    const client = await createWhatsAppClient(transport, failingHttp());
    try {
      client.run();
      await withLimit(transport.entered, 5000, "run task start");
      const first = client.waitForRunCompletion();
      const second = client.waitForRunCompletion();
      await withLimit(client.disconnect(), 5000, "disconnect()");
      const [a, b] = (await withLimit(
        Promise.all([first, second]),
        5000,
        "both waits"
      )) as unknown as [Completion, Completion];
      expect(a.reason).toBe("shutdown-requested");
      expect(b).toEqual(a);
    } finally {
      client.free();
    }
  });

  test("a second run() changes neither the call outcome nor the stored result", async () => {
    const transport = gatedTransport();
    const client = await createWhatsAppClient(transport, failingHttp());
    try {
      client.run();
      await withLimit(transport.entered, 5000, "run task start");
      const waiting = client.waitForRunCompletion();
      await withLimit(client.disconnect(), 5000, "disconnect()");
      const before = (await withLimit(waiting, 5000, "wait")) as unknown as Completion;

      let secondError: (Error & { kind?: string }) | null = null;
      try {
        client.run();
      } catch (error) {
        secondError = error as Error & { kind?: string };
      }
      expect(secondError).not.toBeNull();

      // Later client activity never rewrites the stored completion.
      client.setAutoReconnect(true);
      expect(client.reachability()).toBeDefined();
      expect(client.withdrawParkedCalls()).toBe(0);

      const after = (await withLimit(
        client.waitForRunCompletion(),
        5000,
        "wait after second run()"
      )) as unknown as Completion;
      expect(after).toEqual(before);
    } finally {
      client.free();
    }
  });

  test("a failed first connect with reconnect off ends with the exact typed cause", async () => {
    const transport = failingTransport();
    const client = await createWhatsAppClient(transport, failingHttp());
    try {
      client.setAutoReconnect(false);
      client.run();
      await withLimit(transport.entered, 5000, "run task start");
      const completion = (await withLimit(
        client.waitForRunCompletion(),
        7000,
        "waitForRunCompletion()"
      )) as unknown as Completion;
      expect(completion.reason).toBe("auto-reconnect-disabled");
      expect(completion.generation).toBe(0);
      // No connection was ever established, so there is no reader outcome
      // and no protocol cause captured: absence stays absent rather than
      // becoming a blank object.
      expect("connection" in completion ? completion.connection : undefined).toBeUndefined();
      expect("protocolError" in completion ? completion.protocolError : undefined).toBeUndefined();
      // The stubbed transport rejects its open, so that is the exact step
      // that failed; the message carries the stub's own rejection as its
      // source. Fully stubbed I/O makes the step deterministic.
      expect(completion.connectError?.kind).toBe("transport");
      expect(completion.connectError?.message).toContain("boom");
    } finally {
      client.free();
    }
  });

  test("freeing the client settles a pending wait instead of leaving it hanging", async () => {
    const transport = gatedTransport();
    const client = await createWhatsAppClient(transport, failingHttp());
    client.run();
    // The run task began and the wait is outstanding while the client still
    // exists: this tears down a live observation, not a call never started.
    await withLimit(transport.entered, 5000, "run task start");
    const waiting = client.waitForRunCompletion();
    client.free();
    let error: (Error & { kind?: string }) | null = null;
    try {
      await withLimit(waiting, 5000, "wait after free()");
    } catch (e) {
      error = e as Error & { kind?: string };
    }
    // The supervision never completed; the bridge cancelled its own waiter
    // when the host tore the client down. That is neither a supervision
    // outcome (nothing resolves) nor a hang.
    expect(error).not.toBeNull();
    expect(error!.kind).toBe("not-connected");
  });
});

// The mock signs with its own root, so this block opts one client into the
// testing bypass explicitly; production keeps the default. Gated on a WS
// probe like the other E2E files: skipped in CI, and an explicitly supplied
// but unreachable MOCK_SERVER_URL is a visible failure, not a silent skip.
async function mockWsReachable(timeoutMs = 2000): Promise<boolean> {
  const url = process.env.MOCK_SERVER_URL ?? "wss://127.0.0.1:8080/ws/chat";
  return new Promise((resolve) => {
    let done = false;
    let ws: WebSocket | null = null;
    const finish = (v: boolean) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      try {
        ws?.terminate();
      } catch {
        // Terminating a half-open probe socket must not fail the gate.
      }
      resolve(v);
    };
    const timer = setTimeout(() => finish(false), timeoutMs);
    try {
      ws = new WebSocket(url, { rejectUnauthorized: false });
      ws.on("open", () => finish(true));
      ws.on("error", () => finish(false));
    } catch {
      finish(false);
    }
  });
}

const hasMockServer = await mockWsReachable();
if (process.env.MOCK_SERVER_URL && !hasMockServer) {
  throw new Error(
    `MOCK_SERVER_URL is set but the mock is unreachable: ${process.env.MOCK_SERVER_URL}`
  );
}

describe.skipIf(!hasMockServer)("run completion against the mock server", () => {
  test("a supervised mock session ends with shutdown-requested on disconnect", async () => {
    const events: WhatsAppEvent[] = [];
    const client = await createWhatsAppClient(
      createTransport("run-completion"),
      createHttp(),
      ((event: WhatsAppEvent) => {
        events.push(event);
      }) as never,
      null,
      null,
      null,
      null,
      true
    );
    try {
      client.run();
      // Pending across the whole session: pairing, connect and traffic all
      // pass while this wait is outstanding.
      const waiting = client.waitForRunCompletion();
      await Promise.all([
        autoScanQr(events),
        waitForEvent(events, "pair_success", 20000),
      ]);
      // Supervision really is up, not merely started: the core decoded the
      // handshake and announced it.
      await waitForEvent(events, "connected", 45000);
      await withLimit(client.disconnect(), 15000, "disconnect()");
      const completion = (await withLimit(
        waiting,
        15000,
        "waitForRunCompletion()"
      )) as unknown as Completion;
      expect(completion.reason).toBe("shutdown-requested");
      expect(completion.generation).toBe(0);
      const late = (await withLimit(
        client.waitForRunCompletion(),
        15000,
        "late wait"
      )) as unknown as Completion;
      expect(late).toEqual(completion);
    } finally {
      await client.disconnect().catch(() => {});
      client.free();
    }
  }, 120000);
});

describe("run completion emitted types", () => {
  const declarations = readFileSync(
    join(import.meta.dir, "..", "dist", "whatsapp_rust_bridge.d.ts"),
    "utf8"
  );

  test("the observation export and its result union are declared", () => {
    expect(declarations).toContain(
      "waitForRunCompletion(): Promise<RunCompletionResult>"
    );
    for (const tag of [
      'reason: "shutdown-requested"',
      'reason: "auto-reconnect-disabled"',
      'reason: "stopped"',
      'reason: "already-running"',
    ]) {
      expect(declarations).toContain(tag);
    }
    for (const cause of [
      "DisconnectReasonResult",
      "ConnectErrorResult",
      "HandshakeFailureResult",
      "NoiseHandshakeFailureResult",
      "ProtocolTerminalReasonResult",
    ]) {
      expect(declarations).toContain(cause);
    }
  });
});
