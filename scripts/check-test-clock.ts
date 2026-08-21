/**
 * Clock gate: runs the suite, and fails when one test costs more than its
 * budget.
 *
 * The suite is serial, so a test's duration is wall clock the whole repository
 * pays on every run. Nothing measured that, and it drifted the way the package
 * size drifted before `check:size`: one test reached 60 seconds, 80% of
 * `bun test`, and arrived green, in review, with nobody positioned to see the
 * number.
 *
 * This runs the suite rather than sitting beside it, because measuring it by
 * running it twice would cost more than the gate saves. Bun's own output goes
 * straight through and its exit code is honoured first, so a failing suite
 * reports as a failing suite and not as a budget breach. The durations come
 * from `--reporter=junit` rather than from the console: the per-test lines are
 * only printed to a terminal, so a gate reading them would measure nothing in
 * CI and report a pass for it.
 *
 *   bun run scripts/check-test-clock.ts [...bun test args]
 */
import { readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

/**
 * Twice the slowest test that is not exempt (4.2s, the published-.d.ts
 * typecheck), so a runner having a bad minute cannot trip it. This is not a
 * limit on ordinary work. It is the number that would have made a 60-second
 * test someone's decision instead of nobody's.
 */
const MS_BUDGET = 8_000;

/**
 * Tests allowed past the budget, each with the ceiling it gets instead. An
 * entry is a debt, not a pardon: the ceiling still fails if the cost grows, and
 * a name no longer in the suite fails here as a stale exemption.
 */
const EXEMPT: Record<string, { ms: number; why: string }> = {
  "the exported surface > assertSessions settles once the core's offline-sync gate expires": {
    ms: 70_000,
    why: "waits out the core's DEFAULT_OFFLINE_SYNC_TIMEOUT, which is 60s and pub(crate)",
  },
};

/**
 * Unattributed wall: everything the suite spent that no test body accounts for,
 * which is module loading and `beforeAll`/`afterEach` hooks. Bun's junit report
 * charges none of it to anything — a 1.5s `beforeAll` leaves its testcase at
 * 0.0002s and its enclosing testsuite at 0 — so without this a slow fixture is
 * the one way past the budget above. It is one number for the whole run because
 * that is the only level the report measures it at. 15s against the 0.9s a full
 * suite spends today, so it catches a fixture that ran away and nothing else.
 */
const MS_SETUP_BUDGET = 15_000;

const report = join(tmpdir(), `whatsapp-rust-bridge-test-clock-${process.pid}.xml`);

// A caller can forward `bun test` arguments, which may select a subset. The
// checks that only make sense over the whole suite are skipped when they do.
const forwarded = Bun.argv.slice(2);
const wholeSuite = forwarded.length === 0;

const proc = Bun.spawn(
  ["bun", "test", "--reporter=junit", `--reporter-outfile=${report}`, ...forwarded],
  { stdout: "inherit", stderr: "inherit", env: process.env }
);
const status = await proc.exited;
if (status !== 0) {
  rmSync(report, { force: true });
  process.exit(status);
}

function unescape(value: string) {
  return value
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

let xml: string;
try {
  xml = readFileSync(report, "utf8");
} catch {
  console.error(`check-test-clock: bun wrote no junit report to ${report}.`);
  process.exit(1);
}
rmSync(report, { force: true });

/**
 * One entry per testcase, never collapsed by name. Two files can declare the
 * same `describe > test` (`emitted types > every new operation is declared` is
 * in both the business and newsletter surfaces), and keying on the name alone
 * dropped one of the two durations out of the attributed total, where it then
 * reappeared as fixture time the suite had not spent.
 */
const seen: Array<{ display: string; file: string; ms: number }> = [];
for (const testcase of xml.matchAll(/<testcase\b([^>]*)>/g)) {
  const attrs = new Map(
    [...testcase[1].matchAll(/([\w:-]+)="([^"]*)"/g)].map(([, key, value]) => [key, unescape(value)])
  );
  const name = attrs.get("name");
  if (name === undefined) continue;
  const suite = attrs.get("classname");
  seen.push({
    display: suite ? `${suite} > ${name}` : name,
    file: attrs.get("file") ?? "",
    ms: Number(attrs.get("time") ?? 0) * 1000,
  });
}

/** How a test is named in a message: with its file only when the name repeats. */
const ambiguous = new Set(
  seen.filter((t, i) => seen.findIndex((o) => o.display === t.display) !== i).map((t) => t.display)
);
const label = (t: { display: string; file: string }) =>
  ambiguous.has(t.display) ? `${t.display} (${t.file})` : t.display;

const problems: string[] = [];

// Nothing parsed means the report moved and this measured an empty suite: a
// pass that proves nothing, which is the failure mode a budget gate has.
if (seen.length === 0) {
  problems.push(`check-test-clock: no <testcase> durations in bun's junit report.`);
}

if (wholeSuite) {
  for (const [name, { why }] of Object.entries(EXEMPT)) {
    if (!seen.some((t) => t.display === name)) {
      problems.push(`check-test-clock: exempt test is no longer in the suite: ${name} (${why})`);
    }
  }
}

const attributed = seen.reduce((sum, t) => sum + t.ms, 0);
const suiteTotal = Number(/<testsuites\b[^>]*\btime="([\d.]+)"/.exec(xml)?.[1] ?? 0) * 1000;
const setup = suiteTotal - attributed;
console.log(
  `check-test-clock: ${seen.length} tests, ${(attributed / 1000).toFixed(1)}s in test bodies, ` +
    `${(setup / 1000).toFixed(1)}s in loading and fixtures`
);
if (setup > MS_SETUP_BUDGET) {
  problems.push(
    `check-test-clock: ${(setup / 1000).toFixed(1)}s of the run went to module loading and ` +
      `fixtures, over the ${MS_SETUP_BUDGET / 1000}s budget.\n` +
      `check-test-clock: bun charges hook time to no test, so look for a slow beforeAll or a ` +
      `module that does work at import.`
  );
}
const ranked = [...seen].sort((a, b) => b.ms - a.ms);
for (const test of ranked.slice(0, 5)) {
  console.log(`check-test-clock:   ${(test.ms / 1000).toFixed(2).padStart(7)}s  ${label(test)}`);
}

for (const test of ranked) {
  const ms = test.ms;
  const budget = EXEMPT[test.display]?.ms ?? MS_BUDGET;
  if (ms <= budget) continue;
  problems.push(
    `check-test-clock: ${(ms / 1000).toFixed(2)}s for "${label(test)}", over its ` +
      `${(budget / 1000).toFixed(1)}s budget.\n` +
      `check-test-clock: the suite is serial, so that is wall clock on every run. Make it ` +
      `cheaper, or add it to EXEMPT in scripts/check-test-clock.ts with a reason and a ceiling.`
  );
}

if (problems.length > 0) {
  for (const problem of problems) console.error(problem);
  process.exit(1);
}

console.log(`check-test-clock: nothing over its budget (${MS_BUDGET / 1000}s unless exempt)`);
