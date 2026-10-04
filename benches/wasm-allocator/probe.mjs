/**
 * One sample: load one artifact in a fresh process, run one workload, print
 * the result as JSON. Fresh per sample so a previous artifact's committed
 * pages, JIT state and heap steps cannot carry into the next reading.
 *
 *   node probe.mjs <artifact> <workload>
 */
import { loadArtifact } from "./artifact.mjs";
import { workloads } from "./workloads.mjs";

const [name, workload] = process.argv.slice(2);
if (!name || !workloads[workload]) {
  console.error(`usage: probe.mjs <artifact> <${Object.keys(workloads).join("|")}>`);
  process.exit(2);
}

const wasm = await loadArtifact(name);
const before = wasm.getWasmMemoryBytes();
const result = workloads[workload](wasm);
console.log(JSON.stringify({ artifact: name, workload, committedBefore: before, ...result }));
