/**
 * What each global-allocator arm costs on this bridge's own load.
 *
 * Artifacts are measured round-robin, and the order rotates and then mirrors
 * every round pair, so each arm spends as many launches before the base arm as
 * after it. Each cell reports a median and the spread around it; a difference
 * smaller than the spread is not a difference.
 *
 *   node benches/wasm-allocator/run.mjs [--rounds=9] [--workload=boundary,...] a b c
 *
 * Artifacts are names under `benches/wasm-module-rss/artifacts/`, built by
 * `build-variant.sh`.
 */
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { workloads } from "./workloads.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const probe = join(here, "probe.mjs");

const argv = process.argv.slice(2);
// Rounded up to even: the counterbalancing below pairs each round with its
// mirror, and an odd count leaves one arm's position uncancelled.
const rounds = 2 * Math.ceil(Number(argv.find((a) => a.startsWith("--rounds="))?.slice(9) ?? 9) / 2);
const selected = (argv.find((a) => a.startsWith("--workload="))?.slice(11) ?? "").split(",").filter(Boolean);
const artifacts = argv.filter((a) => !a.startsWith("--"));
const chosen = selected.length ? selected : Object.keys(workloads);

if (artifacts.length === 0) {
  console.error("usage: run.mjs [--rounds=N] [--workload=a,b] <artifact> …");
  process.exit(2);
}

const median = (xs) => {
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
const mean = (xs) => xs.reduce((a, b) => a + b, 0) / xs.length;
const stddev = (xs) => {
  if (xs.length < 2) return 0;
  const m = mean(xs);
  return Math.sqrt(xs.reduce((a, b) => a + (b - m) ** 2, 0) / (xs.length - 1));
};

const samples = new Map();
for (const workload of chosen) {
  for (const artifact of artifacts) samples.set(`${workload} ${artifact}`, []);
}

for (let round = 0; round < rounds; round++) {
  // Rotate so no arm is always measured on a cold machine, and reverse on odd
  // rounds so each arm sits as far before the base arm as it sat after it.
  // Rotation alone cannot do that: it moves every arm together, so an arm two
  // launches after the base stays two launches after it in almost every round,
  // and the paired ratio then absorbs within-round drift instead of cancelling
  // it. Rounds are consumed in pairs, hence the halved rotation index.
  const rot = Math.floor(round / 2) % artifacts.length;
  const order = artifacts.map((_, i) => artifacts[(i + rot) % artifacts.length]);
  if (round % 2 === 1) order.reverse();
  for (const workload of chosen) {
    for (const artifact of order) {
      const out = execFileSync(process.execPath, [probe, artifact, workload], {
        encoding: "utf8",
        maxBuffer: 1 << 24,
      });
      samples.get(`${workload} ${artifact}`).push(JSON.parse(out));
    }
  }
  process.stderr.write(`round ${round + 1}/${rounds}\n`);
}

const MIB = 1024 * 1024;
const pad = (s, n) => String(s).padEnd(n);
const rpad = (s, n) => String(s).padStart(n);

for (const workload of chosen) {
  console.log(`\n## ${workload}`);
  const rows = artifacts.map((artifact) => {
    const runs = samples.get(`${workload} ${artifact}`);
    const ns = runs.map((r) => r.ns);
    const peak = runs.map((r) => r.committedPeak ?? r.committedFinal);
    const final = runs.map((r) => r.committedFinal);
    const afterPeak = runs.map((r) => r.committedAfterPeak).filter((v) => v !== undefined);
    return { artifact, ns, peak, final, afterPeak };
  });
  const base = rows[0];
  const hasAfterPeak = rows.some((r) => r.afterPeak.length > 0);
  const header = ["artifact", "ns/op", "vs base", "paired", "slower", "peak MiB", "final MiB"];
  const widths = [26, 18, 9, 9, 8, 16, 16];
  if (hasAfterPeak) {
    header.push("after peak MiB");
    widths.push(16);
  }
  console.log(header.map((h, i) => (i ? rpad(h, widths[i]) : pad(h, widths[i]))).join("  "));
  for (const row of rows) {
    const m = median(row.ns);
    const delta = row === base ? "" : `${(((m - median(base.ns)) / median(base.ns)) * 100).toFixed(1)}%`;
    // Round by round against the base arm, which cancels whatever the machine
    // was doing that round. `slower` is how many rounds the arm lost outright.
    const ratios = row.ns.map((v, i) => v / base.ns[i]);
    const paired = row === base ? "" : `${((median(ratios) - 1) * 100).toFixed(1)}%`;
    const slower = row === base ? "" : `${ratios.filter((r) => r > 1).length}/${ratios.length}`;
    console.log(
      [
        pad(row.artifact, widths[0]),
        rpad(`${m.toFixed(1)} ±${stddev(row.ns).toFixed(1)}`, widths[1]),
        rpad(delta, widths[2]),
        rpad(paired, widths[3]),
        rpad(slower, widths[4]),
        rpad(`${(median(row.peak) / MIB).toFixed(2)} ±${(stddev(row.peak) / MIB).toFixed(2)}`, widths[5]),
        rpad(`${(median(row.final) / MIB).toFixed(2)} ±${(stddev(row.final) / MIB).toFixed(2)}`, widths[6]),
        ...(hasAfterPeak
          ? [
              rpad(
                `${(median(row.afterPeak) / MIB).toFixed(2)} ±${(stddev(row.afterPeak) / MIB).toFixed(2)}`,
                widths[7]
              ),
            ]
          : []),
      ].join("  ")
    );
  }
}

console.log(
  `\nnode ${process.versions.node}, ${rounds} rounds, one process per sample, order rotated and mirrored per round pair`
);
