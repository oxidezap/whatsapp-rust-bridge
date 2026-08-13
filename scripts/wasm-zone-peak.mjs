/**
 * What compiling this wasm costs V8, as one deterministic number.
 *
 * The consumer-visible symptom is a ~37 MiB step in private memory the first
 * time a process opens a connection. That step is V8 compiling this module —
 * not per-session state — and the part of it that scales with our artifact is
 * the compiler's zone memory. `v8.getHeapStatistics().peak_malloced_memory`
 * counts exactly that, and with compilation serialised it is bit-identical run
 * to run, which makes it usable as a build-time gate without a benchmark host.
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

/** Private/resident footprint of this process, for the RSS half of the story. */
function processMemory() {
  const status = readFileSync("/proc/self/status", "utf8");
  const field = (key) => {
    const m = status.match(new RegExp(`^${key}:\\s+(\\d+) kB`, "m"));
    return m ? Number(m[1]) * 1024 : null;
  };
  let priv = null;
  try {
    const rollup = readFileSync("/proc/self/smaps_rollup", "utf8");
    const g = (key) => {
      const m = rollup.match(new RegExp(`^${key}:\\s+(\\d+) kB`, "m"));
      return m ? Number(m[1]) * 1024 : 0;
    };
    priv = g("Private_Dirty") + g("Private_Clean");
  } catch {
    // smaps_rollup is Linux-only; the zone number stands on its own without it.
  }
  return { rss: field("VmRSS"), peakRss: field("VmHWM"), private: priv };
}

async function child(wasmPath) {
  const bytes = readFileSync(wasmPath);
  const before = processMemory();
  const started = process.hrtime.bigint();
  new WebAssembly.Module(bytes);
  const compileMs = Number(process.hrtime.bigint() - started) / 1e6;

  // `new WebAssembly.Module` returns once the baseline tier is done; the
  // optimising tier keeps running on background tasks and is most of the zone
  // memory we came for. Wait for the peak to stop moving rather than guessing
  // a sleep.
  let peak = v8.getHeapStatistics().peak_malloced_memory;
  let unchanged = 0;
  const deadline = Date.now() + 60_000;
  while (unchanged < 12 && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 50));
    const now = v8.getHeapStatistics().peak_malloced_memory;
    if (now === peak) unchanged++;
    else {
      peak = now;
      unchanged = 0;
    }
  }

  const after = processMemory();
  process.stdout.write(
    JSON.stringify({
      peakMalloced: v8.getHeapStatistics().peak_malloced_memory,
      peakRss: after.peakRss,
      private: after.private,
      rssDelta: after.rss - before.rss,
      compileMs,
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

  const summarise = (key) => {
    const values = runs.map((r) => r[key]).filter((v) => v != null).sort((a, b) => a - b);
    if (values.length === 0) return null;
    const mib = (b) => (b / MIB).toFixed(3);
    return {
      min: mib(values[0]),
      median: mib(values[Math.floor(values.length / 2)]),
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
        compileMsMedian: Number(compileMs[Math.floor(compileMs.length / 2)].toFixed(1)),
      },
      null,
      2
    )
  );
}

const args = process.argv.slice(2);
const wasmPath = args.find((a) => !a.startsWith("--"));
if (!wasmPath) {
  console.error("usage: node scripts/wasm-zone-peak.mjs <file.wasm> [--reps N] [--serial|--parallel]");
  process.exit(2);
}

if (process.env.WASM_ZONE_PEAK_CHILD) {
  await child(wasmPath);
} else {
  const repsFlag = args.indexOf("--reps");
  driver(wasmPath, repsFlag === -1 ? 15 : Number(args[repsFlag + 1]), !args.includes("--parallel"));
}
