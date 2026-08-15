/**
 * What compiling this wasm costs V8, as one deterministic number.
 *
 * The consumer-visible symptom is a ~37 MiB step in private memory the first
 * time a process opens a connection. That step is V8 compiling this module —
 * not per-session state — and `v8.getHeapStatistics().peak_malloced_memory`
 * tracks the part of it that scales with our artifact. With compilation
 * serialised it is bit-identical run to run, which makes it usable as a
 * comparator without a benchmark host.
 *
 * What it tracks is per-function compilation work, and NOT one tier's zones:
 * compiling every function costs ~7.9 MiB where a lazy load costs 0.5, stubbing
 * every body takes it to 0.28 — and `--liftoff-only` leaves it where it was, to
 * three decimals. Read a change in it as "the module got cheaper to compile",
 * not as a claim about which compiler paid.
 *
 * Node, not Bun: the number is a V8 statistic, and the flags below are V8
 * flags. Plain `.mjs` for the same reason — the child processes are spawned
 * with `--no-wasm-lazy-compilation`, and adding a TypeScript loader to that
 * command line only adds a way for it to break.
 *
 *   node scripts/wasm-zone-peak.mjs pkg/whatsapp_rust_bridge_bg.wasm
 *   node scripts/wasm-zone-peak.mjs <wasm> --reps 15 --parallel
 *
 * `--serial` (the default) pins `--wasm-num-compilation-tasks=1`, so the peak
 * is set by the single most expensive function and repeats exactly. `--parallel`
 * leaves V8's own scheduling alone: closer to what a host actually pays, but
 * the peak then depends on which functions happen to compile at once, so read
 * the median of many runs and not a pair.
 *
 * Note what this does NOT measure: it compiles the WHOLE module eagerly, while
 * a real connect compiles roughly a fifth of it lazily. It is a comparator
 * between two artifacts, not a prediction of a host's RSS.
 */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import v8 from "node:v8";

const SELF = fileURLToPath(import.meta.url);
const MIB = 1024 * 1024;

/**
 * Private/resident footprint of this process, for the RSS half of the story.
 *
 * procfs is Linux-only, and the zone number — the reason this script exists —
 * is not. Every read here is optional: a machine without procfs reports the
 * V8 statistic and leaves these fields null, rather than failing before the
 * compile it came to measure.
 */
function processMemory() {
  const kilobytes = (text, key) => {
    if (text === null) return null;
    const match = text.match(new RegExp(`^${key}:\\s+(\\d+) kB`, "m"));
    return match ? Number(match[1]) * 1024 : null;
  };
  const read = (path) => {
    try {
      return readFileSync(path, "utf8");
    } catch {
      return null;
    }
  };

  const status = read("/proc/self/status");
  const rollup = read("/proc/self/smaps_rollup");
  const dirty = kilobytes(rollup, "Private_Dirty");
  const clean = kilobytes(rollup, "Private_Clean");

  return {
    rss: kilobytes(status, "VmRSS"),
    peakRss: kilobytes(status, "VmHWM"),
    private: dirty === null && clean === null ? null : (dirty ?? 0) + (clean ?? 0),
  };
}

// The synchronous constructor under `--no-wasm-lazy-compilation` returns with
// the whole module compiled, so there is nothing to wait for afterwards. An
// earlier version settled for 600 ms per run against a moving peak: serial mode
// reported the same 7.894 MiB with and without it, and 15 parallel runs each way
// were indistinguishable (median 18.85 vs 19.82 MiB, ranges overlapping). Sample
// straight after the constructor.
function child(wasmPath) {
  const bytes = readFileSync(wasmPath);
  const before = processMemory();
  const started = process.hrtime.bigint();
  const compiled = new WebAssembly.Module(bytes);
  const compileMs = Number(process.hrtime.bigint() - started) / 1e6;
  const after = processMemory();

  // The module has to still be reachable through the sample above. A collection
  // before it would hand the compiled code back, and the RSS figures would then
  // report whether a GC happened rather than what the module costs. Reading it
  // afterwards is what keeps it alive.
  const exports = WebAssembly.Module.exports(compiled).length;

  process.stdout.write(
    JSON.stringify({
      peakMalloced: v8.getHeapStatistics().peak_malloced_memory,
      peakRss: after.peakRss,
      private: after.private,
      rssDelta: after.rss === null || before.rss === null ? null : after.rss - before.rss,
      compileMs,
      exports,
    })
  );
}

function driver(wasmPath, reps, serial) {
  const flags = ["--no-wasm-lazy-compilation"];
  if (serial) flags.push("--wasm-num-compilation-tasks=1");

  const runs = [];
  for (let i = 0; i < reps; i++) {
    const out = execFileSync(process.execPath, [...flags, SELF, wasmPath], {
      encoding: "utf8",
      env: { ...process.env, WASM_ZONE_PEAK_CHILD: "1" },
    });
    runs.push(JSON.parse(out.trim().split("\n").pop()));
  }

  // Averaging the middle pair on an even count, rather than taking the upper of
  // the two: with `--reps 2` the upper-middle IS the maximum, which reports the
  // worse of two runs as the typical one.
  const median = (sorted) => {
    const middle = sorted.length >> 1;
    return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
  };

  const summarise = (key) => {
    const values = runs.map((r) => r[key]).filter((v) => v != null).sort((a, b) => a - b);
    if (values.length === 0) return null;
    const mib = (b) => (b / MIB).toFixed(3);
    return {
      min: mib(values[0]),
      median: mib(median(values)),
      max: mib(values[values.length - 1]),
      distinctValues: new Set(values).size,
    };
  };
  const compileMs = runs.map((r) => r.compileMs).sort((a, b) => a - b);

  console.log(
    JSON.stringify(
      {
        wasm: wasmPath,
        wasmBytes: readFileSync(wasmPath).length,
        mode: serial ? "serial" : "parallel",
        reps,
        zonePeakMiB: summarise("peakMalloced"),
        peakRssMiB: summarise("peakRss"),
        privateMiB: summarise("private"),
        compileMsMedian: Number(median(compileMs).toFixed(1)),
      },
      null,
      2
    )
  );
}

const USAGE =
  "usage: node scripts/wasm-zone-peak.mjs <file.wasm> [--reps N] [--serial|--parallel]";

const args = process.argv.slice(2);
let wasmPath = null;
let reps = 15;
let serial = true;

// `--reps` takes a value, so the scan has to consume it — otherwise a path
// written after the flag is shadowed by the count, and the script measures a
// file named "15".
for (let i = 0; i < args.length; i++) {
  const arg = args[i];
  if (arg === "--reps") reps = Number(args[++i]);
  else if (arg === "--serial") serial = true;
  else if (arg === "--parallel") serial = false;
  else if (arg.startsWith("--")) {
    console.error(`unknown option ${arg}\n${USAGE}`);
    process.exit(2);
  } else if (wasmPath === null) wasmPath = arg;
  else {
    console.error(`unexpected argument ${arg}\n${USAGE}`);
    process.exit(2);
  }
}

if (wasmPath === null) {
  console.error(USAGE);
  process.exit(2);
}

// A rep count that runs nothing would still print a summary — nulls and a
// `mode`, which reads like a measurement of an artifact that costs nothing.
if (!Number.isInteger(reps) || reps < 1) {
  console.error(`--reps must be a positive integer\n${USAGE}`);
  process.exit(2);
}

if (process.env.WASM_ZONE_PEAK_CHILD) child(wasmPath);
else driver(wasmPath, reps, serial);
