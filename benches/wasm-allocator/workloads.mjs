/**
 * What the global allocator is asked to do, in the shapes this bridge asks it.
 *
 * Two of these are per-message and two are the history-sync load. Nothing here
 * needs a server: every export used is a free function over bytes, which is
 * also why none of it proves an end-to-end response.
 *
 * Each workload returns `{ ns, committedPeak, committedFinal }` for one sample.
 * `ns` is per operation. Committed bytes come from `getWasmMemoryBytes()`,
 * which counts pages the module holds rather than heap the allocator handed
 * out, and pages never go back to the host on wasm32.
 */
import { deflateSync } from "node:zlib";

function lcg(seed) {
  let s = BigInt(seed);
  return () => {
    s = (s * 6364136223846793005n + 1442695040888963407n) & 0xffffffffffffffffn;
    return Number(s >> 40n);
  };
}

/** Bytes that compress like a protobuf history blob: repeated field tags,
 * short varints and runs of text, tiled from a 64 KiB seeded pattern. */
function historyBlob(bytes, seed) {
  const rand = lcg(seed);
  const names = ["Ana", "Bruno", "Carla", "Diego", "Elena", "Fabio"];
  const parts = [];
  let patternBytes = 0;
  while (patternBytes < 65536) {
    const name = names[rand() % names.length];
    const part =
      `\x0a${String.fromCharCode(name.length)}${name}\x10${String.fromCharCode(rand() % 120)}` +
      `\x1a\x20abcdefghijklmnopqrstuvwxyz012345`;
    parts.push(part);
    patternBytes += part.length;
  }
  const pattern = Buffer.from(parts.join(""), "latin1");

  const out = Buffer.alloc(bytes);
  for (let at = 0; at < bytes; at += pattern.length) {
    pattern.copy(out, at, 0, Math.min(pattern.length, bytes - at));
  }
  return out;
}

/**
 * Per-operation nanoseconds, as the fastest of `samples` batches rather than
 * the mean of one. A batch that lost the CPU reports the scheduler, not the
 * allocator, and a mean cannot tell those apart; the fastest batch is the one
 * that was least interrupted.
 */
const bench = (iterations, fn, samples = 7) => {
  const batch = Math.max(1, Math.floor(iterations / samples));
  let best = Infinity;
  for (let s = 0; s < samples; s++) {
    const started = process.hrtime.bigint();
    for (let i = 0; i < batch; i++) fn(i);
    const ns = Number(process.hrtime.bigint() - started) / batch;
    if (ns < best) best = ns;
  }
  return best;
};

/**
 * One `&[u8]` in, one `Uint8Array` out, 1 KiB each way. This is the allocator
 * on the boundary itself: `__wbindgen_malloc`, the copy in, the hash, the
 * result allocation, `__wbindgen_free`. Nothing else in the bridge's hot path
 * calls the allocator more often than a crossing does.
 */
export function boundary(wasm, { iterations = 200_000 } = {}) {
  const payload = historyBlob(1024, 11);
  wasm.md5(payload);
  const warm = 50_000;
  bench(warm, () => wasm.md5(payload), 1);
  const ns = bench(iterations, () => wasm.md5(payload));
  return { ns, committedFinal: wasm.getWasmMemoryBytes() };
}

/**
 * The same crossing with almost no work behind it: 16 bytes in, 16 out. What
 * separates this from `boundary` is how much of the call the allocator is,
 * and that is the point of running both. An allocator that is faster shows up
 * here first, and is diluted by whatever real work the export does.
 */
export function churn(wasm, { iterations = 700_000 } = {}) {
  const payload = historyBlob(16, 99);
  bench(100_000, () => wasm.md5(payload), 1);
  const ns = bench(iterations, () => wasm.md5(payload));
  return { ns, committedFinal: wasm.getWasmMemoryBytes() };
}

/**
 * The same call with no allocation at all: no arguments, an `f64` back. The
 * floor under `churn`, so the two together price what `__wbindgen_malloc` and
 * `__wbindgen_free` are worth on a crossing.
 */
export function callOnly(wasm, { iterations = 2_000_000 } = {}) {
  bench(200_000, () => wasm.getWasmMemoryBytes(), 1);
  const ns = bench(iterations, () => wasm.getWasmMemoryBytes());
  return { ns, committedFinal: wasm.getWasmMemoryBytes() };
}

/**
 * The two curve operations every ratchet step runs, with the small
 * allocations they carry. `curve25519-dalek` is `opt-level = 3` in the release
 * profile precisely because this is the per-message cost.
 */
export function ratchet(wasm, { iterations = 8_000 } = {}) {
  const pair = wasm.generateKeyPair();
  const peer = wasm.generateKeyPair();
  const message = historyBlob(256, 22);
  const priv = pair.privKey;
  const pub = peer.pubKey;
  const step = () => {
    wasm.calculateAgreement(pub, priv);
    wasm.calculateSignature(priv, message);
  };
  bench(2_000, step, 1);
  const ns = bench(iterations, step);
  return { ns, committedFinal: wasm.getWasmMemoryBytes() };
}

/**
 * One history-sync blob inflated, repeatedly. `inflateZlib` pools its
 * decompressor and scratch buffer, so what this measures is the output
 * allocation, its growth, and the `Uint8Array` copy back out.
 */
export function inflate(wasm, { iterations = 120, bytes = 4 << 20 } = {}) {
  const compressed = deflateSync(historyBlob(bytes, 33));
  let peak = 0;
  const step = () => {
    wasm.inflateZlib(compressed, 64 << 20);
    const now = wasm.getWasmMemoryBytes();
    if (now > peak) peak = now;
  };
  bench(20, step, 1);
  const ns = bench(iterations, step);
  return { ns, committedPeak: peak, committedFinal: wasm.getWasmMemoryBytes() };
}

/**
 * A history sync with the sizes a real one has: blobs from 256 KiB to 16 MiB
 * in a shuffled order, each inflated and dropped, with per-message crossings
 * in between. Committed memory is the number that matters here, because a page
 * this commits is a page the process holds for good.
 */
export function historySync(wasm, { rounds = 8 } = {}) {
  const sizes = [1 << 18, 1 << 20, 1 << 22, 1 << 23, 1 << 24, 1 << 21, 1 << 19, 1 << 22];
  const blobs = sizes.map((bytes, i) => deflateSync(historyBlob(bytes, 44 + i)));
  const small = historyBlob(1024, 55);

  let peak = wasm.getWasmMemoryBytes();
  const step = (i) => {
    wasm.inflateZlib(blobs[i % blobs.length], 64 << 20);
    for (let k = 0; k < 2_000; k++) wasm.md5(small);
    const now = wasm.getWasmMemoryBytes();
    if (now > peak) peak = now;
  };

  bench(blobs.length, step, 1);
  const ns = bench(rounds * blobs.length, step, rounds);

  return { ns, committedPeak: peak, committedFinal: wasm.getWasmMemoryBytes() };
}

/**
 * What the allocator gives back. A peak is reached, everything is dropped, and
 * then a long tail of small work runs: the question is whether the small work
 * fits in what the peak already committed, or asks the host for more.
 */
export function retention(wasm, { rounds = 7 } = {}) {
  const big = deflateSync(historyBlob(24 << 20, 66));
  for (let i = 0; i < 3; i++) wasm.inflateZlib(big, 64 << 20);
  const afterPeak = wasm.getWasmMemoryBytes();

  const small = historyBlob(4096, 77);
  const medium = deflateSync(historyBlob(1 << 20, 88));
  const step = () => {
    wasm.inflateZlib(medium, 64 << 20);
    for (let k = 0; k < 500; k++) wasm.md5(small);
  };
  bench(7, step, 1);
  const ns = bench(rounds * 7, step, rounds);

  const committedFinal = wasm.getWasmMemoryBytes();
  return {
    ns,
    committedPeak: afterPeak,
    committedFinal,
    // What the tail asked for beyond what the peak already committed. Pages
    // never go back on wasm32, so this is the whole of "did the allocator
    // reuse the peak".
    committedAfterPeak: committedFinal - afterPeak,
  };
}

export const workloads = { callOnly, churn, boundary, ratchet, inflate, historySync, retention };
