/**
 * What the numeric input contract costs on the encode path.
 *
 * `contract` is the writer in `ts/proto-reader.ts`, `baseline` is
 * `@bufbuild/protobuf`'s writer with no checks, and `empty` is a bare subclass
 * of it — the third arm separates the price of subclassing from the price of
 * the guard. The generated `encode(message, writer?)` takes the writer, so all
 * three drive the same codec over the same input.
 *
 * One arm per process: running them together makes every call site inside
 * `encode()` polymorphic, which is not what a consumer loading one writer sees.
 * The driver interleaves rounds so machine drift hits the arms equally.
 *
 * Run: bun run bench:proto
 */
import { heapStats } from "bun:jsc";
import { BinaryWriter as BaselineWriter } from "@bufbuild/protobuf/wire";
import { ClientPayload, Message } from "../ts/generated/whatsapp";
import { BinaryWriter as ContractWriter } from "../ts/proto-reader";

const ITERATIONS = 150_000;
const WARMUP = 400_000;
const REPS = 9;
const ROUNDS = 7;
const MODES = ["contract", "empty", "baseline"] as const;

type Mode = (typeof MODES)[number];

class EmptySubclassWriter extends BaselineWriter {}

const WRITERS: Record<Mode, new () => BaselineWriter> = {
  contract: ContractWriter,
  empty: EmptySubclassWriter,
  baseline: BaselineWriter,
};

interface Codec {
  encode(message: unknown, writer?: unknown): { finish(): Uint8Array };
}

const media = {
  url: `https://mmg.whatsapp.net/d/f/${"x".repeat(40)}`,
  mimetype: "image/jpeg",
  caption: "a caption of roughly ordinary length for a photo",
  fileSha256: new Uint8Array(32).fill(7),
  fileLength: 182734,
  height: 1080,
  width: 1920,
  mediaKey: new Uint8Array(32).fill(9),
  fileEncSha256: new Uint8Array(32).fill(3),
  directPath: `/v/t62.7118-24/${"y".repeat(30)}`,
  mediaKeyTimestamp: 1_700_000_123,
  jpegThumbnail: new Uint8Array(512).fill(1),
  contextInfo: {
    stanzaId: `3EB0${"0".repeat(16)}`,
    participant: "5511999999999@s.whatsapp.net",
    quotedMessage: { conversation: "the quoted text" },
    expiration: 604800,
    ephemeralSettingTimestamp: 1_700_000_000,
    disappearingMode: { initiator: 0, trigger: 1, initiatedByMe: true },
  },
};

const CASES: { name: string; codec: Codec; value: unknown }[] = [
  {
    name: "Message/conversation (13 B, no numeric field)",
    codec: Message as unknown as Codec,
    value: { conversation: "hello world" },
  },
  {
    name: "Message/imageMessage (912 B, media + contextInfo)",
    codec: Message as unknown as Codec,
    value: { imageMessage: media },
  },
  {
    name: "Message/liveLocationMessage (314 B, float + double)",
    codec: Message as unknown as Codec,
    value: {
      liveLocationMessage: {
        degreesLatitude: -23.5505199,
        degreesLongitude: -46.6333094,
        accuracyInMeters: 12,
        speedInMps: 1.5,
        degreesClockwiseFromMagneticNorth: 271,
        caption: "on the move",
        sequenceNumber: 1_700_000_000_123,
        timeOffset: 60,
        jpegThumbnail: new Uint8Array(256).fill(2),
      },
    },
  },
  {
    name: "ClientPayload (110 B, int-heavy, many tags)",
    codec: ClientPayload as unknown as Codec,
    value: {
      username: 5511999999999,
      passive: false,
      userAgent: {
        platform: 14,
        appVersion: { primary: 2, secondary: 3000, tertiary: 1023421, quaternary: 0, quinary: 0 },
        mcc: "724",
        mnc: "06",
        osVersion: "0.1.0",
        manufacturer: "",
        device: "Desktop",
        osBuildNumber: "0.1.0",
        phoneId: "",
        releaseChannel: 0,
        localeLanguageIso6391: "en",
        localeCountryIso31661Alpha2: "US",
        deviceBoard: "",
        deviceExpId: "",
      },
      webInfo: { webSubPlatform: 0 },
      pushName: "bench",
      sessionId: 1234567,
      shortConnect: true,
      connectType: 1,
      connectReason: 1,
      device: 0,
      product: 0,
      connectAttemptCount: 0,
    },
  },
];

interface Result {
  name: string;
  bytes: number;
  opsPerSec: number;
  cpuUsPerOp: number;
  heapBytesPerOp: number;
  liveObjectsPerOp: number;
  rssMb: number;
}

const median = (xs: number[]): number => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)]!;

function measure(mode: Mode): Result[] {
  const Writer = WRITERS[mode];
  const encodeAll = (codec: Codec, value: unknown, n: number): void => {
    for (let i = 0; i < n; i++) codec.encode(value, new Writer()).finish();
  };

  return CASES.map(({ name, codec, value }) => {
    const bytes = codec.encode(value, new Writer()).finish().length;
    encodeAll(codec, value, WARMUP);

    const ops: number[] = [];
    const cpu: number[] = [];
    const heap: number[] = [];
    for (let rep = 0; rep < REPS; rep++) {
      Bun.gc(true);
      const heapBefore = process.memoryUsage().heapUsed;
      const cpuBefore = process.cpuUsage();
      const startedAt = Bun.nanoseconds();
      encodeAll(codec, value, ITERATIONS);
      const elapsedNs = Bun.nanoseconds() - startedAt;
      const cpuUsed = process.cpuUsage(cpuBefore);
      ops.push((ITERATIONS * 1e9) / elapsedNs);
      cpu.push((cpuUsed.user + cpuUsed.system) / ITERATIONS);
      heap.push((process.memoryUsage().heapUsed - heapBefore) / ITERATIONS);
    }

    // Objects still alive after a full GC around a fixed run. The guard builds
    // no object of its own, so this must not move between the arms.
    Bun.gc(true);
    const objectsBefore = heapStats().objectCount;
    encodeAll(codec, value, ITERATIONS);
    Bun.gc(true);

    return {
      name,
      bytes,
      opsPerSec: median(ops),
      cpuUsPerOp: median(cpu),
      heapBytesPerOp: median(heap),
      liveObjectsPerOp: (heapStats().objectCount - objectsBefore) / ITERATIONS,
      rssMb: process.memoryUsage().rss / 1e6,
    };
  });
}

const requestedMode = process.argv[2] as Mode | undefined;
if (requestedMode) {
  console.log(JSON.stringify(measure(requestedMode)));
} else {
  const rounds: Record<Mode, Result[][]> = { contract: [], empty: [], baseline: [] };
  for (let round = 0; round < ROUNDS; round++) {
    for (const mode of MODES) {
      const child = Bun.spawnSync({
        cmd: ["bun", "run", import.meta.path, mode],
        stdout: "pipe",
        stderr: "inherit",
      });
      if (child.exitCode !== 0) throw new Error(`${mode} exited with ${child.exitCode}`);
      rounds[mode].push(JSON.parse(new TextDecoder().decode(child.stdout)) as Result[]);
    }
  }

  const pick = (mode: Mode, index: number, key: keyof Result): number =>
    median(rounds[mode].map((round) => round[index]![key] as number));
  const delta = (a: number, b: number): string =>
    `${a >= b ? "+" : ""}${(((a - b) / b) * 100).toFixed(2)}%`;

  console.log(
    `bun ${Bun.version} — ${ROUNDS} interleaved rounds x ${REPS} reps x ${ITERATIONS} encodes, median of medians\n`,
  );
  CASES.forEach((_, index) => {
    const bytes = MODES.map((mode) => pick(mode, index, "bytes"));
    if (new Set(bytes).size !== 1) throw new Error("arms disagree on the encoded bytes");
    console.log(`${CASES[index]!.name}  —  ${bytes[0]} bytes, identical across arms`);
    const baseOps = pick("baseline", index, "opsPerSec");
    const baseCpu = pick("baseline", index, "cpuUsPerOp");
    for (const mode of MODES) {
      const ops = pick(mode, index, "opsPerSec");
      const lo = Math.min(...rounds[mode].map((round) => round[index]!.opsPerSec));
      const hi = Math.max(...rounds[mode].map((round) => round[index]!.opsPerSec));
      const cpu = pick(mode, index, "cpuUsPerOp");
      console.log(
        `  ${mode.padEnd(9)}${ops.toFixed(0).padStart(9)} ops/s  [${lo.toFixed(0)}..${hi.toFixed(0)}]  ${delta(ops, baseOps).padStart(8)}` +
          `   cpu ${cpu.toFixed(4)} us/op ${delta(cpu, baseCpu).padStart(8)}` +
          `   heap ${pick(mode, index, "heapBytesPerOp").toFixed(2)} B/op` +
          `   live ${pick(mode, index, "liveObjectsPerOp").toFixed(3)} obj/op`,
      );
    }
    console.log("");
  });
  for (const mode of MODES) {
    console.log(`rss ${mode.padEnd(9)} ${pick(mode, 0, "rssMb").toFixed(1)} MB`);
  }
}
