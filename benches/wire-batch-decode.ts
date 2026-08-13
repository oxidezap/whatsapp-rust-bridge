/**
 * What reading the packed receipt/ack string region costs.
 *
 * The cases pair ASCII against non-ASCII on both sides of `TINY_STRING_LIMIT`,
 * because those are the four paths `ts/wire-info.ts` can take through a region.
 *
 * One case per process: the decoder is a singleton whose region path would
 * otherwise go polymorphic across cases, which is not what a host decoding a
 * stream of one shape sees. Rounds interleave so drift hits the cases equally.
 *
 * Run: bun run benches/wire-batch-decode.ts
 */
import {
  decodeReceiptWireBatch,
  decodeServerAckWireBatch,
  encodeReceiptWireBatch,
  encodeServerAckWireBatch,
  type PackedWireBatch,
  type ReceiptWireData,
  type WireJid,
} from "../ts/wire-info";

const ITERATIONS = 200_000;
const WARMUP = 300_000;
const REPS = 7;
const ROUNDS = 5;

const jid = (user: string, server: string): WireJid => ({
  user,
  server,
  agent: 0,
  device: 0,
  integrator: 0,
});

/** A receipt run shaped like a live batch: a few chats, two ids per receipt. */
function receipts(count: number, id: (i: number) => string, type: string): ReceiptWireData[] {
  return Array.from({ length: count }, (_, i) => {
    const peer = jid(`551199900${i % 4}`, "s.whatsapp.net");
    return {
      source: { chat: peer, sender: peer, is_from_me: false, is_group: false },
      message_ids: [id(i * 2), id(i * 2 + 1)],
      timestamp: 1700000000 + i,
      type: i === 0 ? { Other: type } : "Delivered",
      offline: false,
    };
  });
}

const ascii = (i: number): string => `3EB0C767D${i.toString().padStart(13, "0")}`;
const mixed = (i: number): string => `Ação-${i.toString().padStart(13, "0")}`;

interface Case {
  name: string;
  batch: PackedWireBatch;
  decode: (batch: PackedWireBatch) => unknown;
}

const CASES: Case[] = [
  {
    name: "ack x1, ASCII id (short region)",
    batch: encodeServerAckWireBatch([{ id: ascii(1), class: "message", timestamp: 1700000000 }]),
    decode: decodeServerAckWireBatch,
  },
  {
    name: "ack x1, non-ASCII id (short region)",
    batch: encodeServerAckWireBatch([{ id: mixed(1), class: "message", timestamp: 1700000000 }]),
    decode: decodeServerAckWireBatch,
  },
  {
    name: "receipt x8, ASCII ids (long region)",
    batch: encodeReceiptWireBatch(receipts(8, ascii, "delivery")),
    decode: decodeReceiptWireBatch,
  },
  {
    name: "receipt x8, non-ASCII ids (long region)",
    batch: encodeReceiptWireBatch(receipts(8, mixed, "leído")),
    decode: decodeReceiptWireBatch,
  },
];

const median = (xs: number[]): number => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)]!;

/** `u32 str_len` at header offset 12: the region the case actually reads. */
const regionBytes = (batch: PackedWireBatch): number =>
  new DataView(batch.buffer, batch.byteOffset, batch.byteLength).getUint32(12, true);

interface Result {
  regionBytes: number;
  opsPerSec: number;
  cpuUsPerOp: number;
}

function measure(index: number): Result {
  const testCase = CASES[index]!;
  const decodeAll = (n: number): void => {
    for (let i = 0; i < n; i++) testCase.decode(testCase.batch);
  };

  decodeAll(WARMUP);
  const ops: number[] = [];
  const cpu: number[] = [];
  for (let rep = 0; rep < REPS; rep++) {
    const cpuBefore = process.cpuUsage();
    const startedAt = Bun.nanoseconds();
    decodeAll(ITERATIONS);
    const elapsedNs = Bun.nanoseconds() - startedAt;
    const cpuUsed = process.cpuUsage(cpuBefore);
    ops.push((ITERATIONS * 1e9) / elapsedNs);
    cpu.push((cpuUsed.user + cpuUsed.system) / ITERATIONS);
  }
  return {
    regionBytes: regionBytes(testCase.batch),
    opsPerSec: median(ops),
    cpuUsPerOp: median(cpu),
  };
}

const requested = process.argv[2];
if (requested !== undefined) {
  console.log(JSON.stringify(measure(Number(requested))));
} else {
  const rounds: Result[][] = CASES.map(() => []);
  for (let round = 0; round < ROUNDS; round++) {
    // Rotate the order so no case always runs first: thermal and background
    // drift within a round would otherwise land on the same one every time.
    for (let i = 0; i < CASES.length; i++) {
      const index = (i + round) % CASES.length;
      const child = Bun.spawnSync({
        cmd: ["bun", "run", import.meta.path, String(index)],
        stdout: "pipe",
        stderr: "inherit",
      });
      if (child.exitCode !== 0) throw new Error(`case ${index} exited with ${child.exitCode}`);
      rounds[index]!.push(JSON.parse(new TextDecoder().decode(child.stdout)) as Result);
    }
  }

  const pad = Math.max(...CASES.map(c => c.name.length));
  console.log(
    `bun ${Bun.version} — ${ROUNDS} interleaved rounds x ${REPS} reps x ${ITERATIONS} decodes, median of medians\n`,
  );
  CASES.forEach((testCase, index) => {
    const runs = rounds[index]!;
    const ops = median(runs.map(run => run.opsPerSec));
    const lo = Math.min(...runs.map(run => run.opsPerSec));
    const hi = Math.max(...runs.map(run => run.opsPerSec));
    console.log(
      `${testCase.name.padEnd(pad)}  ${String(runs[0]!.regionBytes).padStart(4)} B region  ` +
        `${ops.toFixed(0).padStart(9)} ops/s  [${lo.toFixed(0)}..${hi.toFixed(0)}]  ` +
        `${(1e9 / ops).toFixed(0).padStart(5)} ns/op  ` +
        `cpu ${median(runs.map(run => run.cpuUsPerOp)).toFixed(3)} us/op`,
    );
  });
}
