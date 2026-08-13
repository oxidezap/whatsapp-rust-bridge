/**
 * What a message batch costs the host, before and after the table started
 * outliving the batch.
 *
 * `baseline` is `ts/wire-info.ts` as of the commit named by `BASELINE_REV`
 * (default `origin/main`), where every batch carried its own string table and
 * the host rebuilt that table from bytes each time. `current` is the working
 * tree. Both arms drive the same traffic through their own encoder and their
 * own decoder, so the numbers compare like for like: the encoders are
 * byte-exact mirrors of the Rust writer each version ships.
 *
 * A baseline identical to the working tree is an error, not a 0% result — set
 * `BASELINE_REV` to whatever the change you are measuring branched from.
 *
 * Reported per traffic shape:
 *   bytes/batch  what crosses the boundary
 *   decoded/msg  UTF-8 bytes the host turns into strings — the work the table
 *                is there to remove
 *   ns/msg       decode wall time, interleaved across arms so drift is shared
 *
 * This measures work and wire size. It says nothing about RSS: V8's heap growth
 * is a step function that saturates, so a transient-allocation cut does not
 * move resident memory in any way this harness could honestly report.
 *
 * Run: bun run bench:wire
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const BASELINE_REV = process.env.BASELINE_REV ?? "origin/main";
const WARMUP_ROUNDS = 3;
const ROUNDS = 21;
const BATCHES = 2_000;

// A batch is what the dispatch loop coalesces, capped at EVENT_BATCH_CAPACITY.
const TRAFFIC = [
  { name: "live singleton, one chat", perBatch: 1, chats: 1, senders: 1 },
  { name: "live singleton, 8 chats", perBatch: 1, chats: 8, senders: 8 },
  { name: "group burst, 32/batch", perBatch: 32, chats: 1, senders: 24 },
  { name: "backlog drain, 32/batch", perBatch: 32, chats: 12, senders: 12 },
] as const;

interface WireModule {
  decodeMessageWireBatch(batch: Uint8Array): { infos: unknown[] };
  encodeMessageWireBatch(entries: readonly Entry[]): Uint8Array;
  MessageWireBatchEncoder?: new () => { encode(entries: readonly Entry[]): Uint8Array };
}

interface Entry {
  payload: Uint8Array;
  info: Record<string, unknown>;
}

/** The baseline module, checked out beside a copy of what it imports. */
async function loadBaseline(): Promise<WireModule> {
  const source = Bun.spawnSync({
    cmd: ["git", "show", `${BASELINE_REV}:ts/wire-info.ts`],
    stdout: "pipe",
  });
  if (source.exitCode !== 0) {
    throw new Error(
      `cannot read ${BASELINE_REV}:ts/wire-info.ts — set BASELINE_REV to a revision that has it`,
    );
  }
  // Two arms running the same code would report a clean 0% rather than fail,
  // which is the one result this harness must never produce quietly.
  const decoded = new TextDecoder().decode(source.stdout);
  if (decoded === (await Bun.file(new URL("../ts/wire-info.ts", import.meta.url)).text())) {
    throw new Error(
      `${BASELINE_REV}:ts/wire-info.ts is identical to the working tree, so there is nothing to ` +
        `compare — point BASELINE_REV at the revision before the change you are measuring`,
    );
  }
  const dir = join(tmpdir(), "wire-info-baseline");
  mkdirSync(dir, { recursive: true });
  const file = join(dir, "wire-info.ts");
  writeFileSync(file, source.stdout);
  return (await import(file)) as WireModule;
}

/** An encoder for one arm: the stateful one where there is one, else one-shot. */
function encoderOf(module: WireModule): (entries: readonly Entry[]) => Uint8Array {
  if (module.MessageWireBatchEncoder) {
    const encoder = new module.MessageWireBatchEncoder();
    return entries => encoder.encode(entries);
  }
  return entries => module.encodeMessageWireBatch(entries);
}

const payload = new Uint8Array(96).fill(7);

function traffic(shape: (typeof TRAFFIC)[number]): Entry[][] {
  const chats = Array.from({ length: shape.chats }, (_, i) => `12036300000000${i}@g.us`);
  const senders = Array.from({ length: shape.senders }, (_, i) => `5511999${i}000@s.whatsapp.net`);
  const names = Array.from({ length: shape.senders }, (_, i) => `Contact Number ${i}`);
  const batches: Entry[][] = [];
  let n = 0;
  for (let b = 0; b < BATCHES; b++) {
    const entries: Entry[] = [];
    for (let m = 0; m < shape.perBatch; m++, n++) {
      const who = n % shape.senders;
      entries.push({
        payload,
        info: {
          chat: chats[n % shape.chats]!,
          sender: senders[who]!,
          isFromMe: false,
          isGroup: shape.chats > 0,
          id: `3EB0${n.toString(16).padStart(16, "0")}`,
          timestamp: 1700000000 + n,
          pushName: names[who]!,
          isViewOnce: false,
          isOffline: false,
        },
      });
    }
    batches.push(entries);
  }
  return batches;
}

/** UTF-8 bytes this batch asks the host to turn into strings. */
function decodedBytes(batch: Uint8Array): number {
  const view = new DataView(batch.buffer, batch.byteOffset, batch.byteLength);
  // Both layouts carry the string-region byte count in the same header slot;
  // in both, that region is exactly what the decoder feeds to TextDecoder.
  return view.getUint32(12, true);
}

/**
 * A decode pass over the whole run. The current decoder is stateful, so a pass
 * has to start at the first batch and walk every one in order — which is also
 * the only order the bridge ever delivers them in.
 */
function decodeRun(module: WireModule, encoded: readonly Uint8Array[]): number {
  const started = Bun.nanoseconds();
  for (const batch of encoded) module.decodeMessageWireBatch(batch);
  return Bun.nanoseconds() - started;
}

const median = (samples: number[]): number => {
  const sorted = [...samples].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)]!;
};

const pct = (before: number, after: number): string => {
  if (before === 0) return "n/a";
  const delta = ((after - before) / before) * 100;
  return `${delta >= 0 ? "+" : ""}${delta.toFixed(1)}%`;
};

const baseline = await loadBaseline();
const current = (await import("../ts/wire-info")) as unknown as WireModule;

console.log(`baseline: ${BASELINE_REV}:ts/wire-info.ts`);
console.log(`${BATCHES} batches per shape, ${ROUNDS} interleaved rounds, median (spread: min..max)\n`);

for (const shape of TRAFFIC) {
  const batches = traffic(shape);
  const messages = BATCHES * shape.perBatch;
  const arms = [
    { name: "before", module: baseline, encoded: batches.map(encoderOf(baseline)) },
    { name: "after", module: current, encoded: batches.map(encoderOf(current)) },
  ];

  for (let round = 0; round < WARMUP_ROUNDS; round++) {
    for (const arm of arms) decodeRun(arm.module, arm.encoded);
  }
  // One round of each per iteration, so machine drift lands on both arms.
  const samples: Record<string, number[]> = { before: [], after: [] };
  for (let round = 0; round < ROUNDS; round++) {
    for (const arm of arms) samples[arm.name]!.push(decodeRun(arm.module, arm.encoded) / messages);
  }

  const [before, after] = arms;
  const bytes = (arm: (typeof arms)[number]): number =>
    arm.encoded.reduce((sum, b) => sum + b.byteLength, 0) / arm.encoded.length;
  const decoded = (arm: (typeof arms)[number]): number =>
    arm.encoded.reduce((sum, b) => sum + decodedBytes(b), 0) / messages;
  const spread = (name: string): string =>
    `${Math.min(...samples[name]!).toFixed(0)}..${Math.max(...samples[name]!).toFixed(0)}`;

  console.log(shape.name);
  console.log(
    `  bytes/batch  ${bytes(before!).toFixed(1)} -> ${bytes(after!).toFixed(1)}` +
      `  (${(bytes(after!) - bytes(before!)).toFixed(1)} B, ${pct(bytes(before!), bytes(after!))})`,
  );
  console.log(
    `  decoded/msg  ${decoded(before!).toFixed(1)} -> ${decoded(after!).toFixed(1)} B` +
      `  (${pct(decoded(before!), decoded(after!))})`,
  );
  console.log(
    `  ns/msg       ${median(samples.before!).toFixed(1)} -> ${median(samples.after!).toFixed(1)}` +
      `  (${pct(median(samples.before!), median(samples.after!))})` +
      `  [${spread("before")} vs ${spread("after")}]\n`,
  );
}
