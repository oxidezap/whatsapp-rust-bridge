/**
 * One measurement, one process. Prints a JSON line on stdout.
 *
 * The metric is `Private_Dirty` from /proc/self/smaps_rollup, never RSS. Two
 * separate reasons:
 *
 *  - RSS's file-backed half drifts tens of MiB with whatever else is resident
 *    on the machine, without the process's own memory changing at all.
 *  - `Private_Clean` is private only until a second process maps the same
 *    pages, at which point it becomes `Shared_Clean` and stops being a
 *    per-process cost. A fleet pays `Private_Dirty` per process; counting the
 *    clean half with it overstates what an extra process costs. `--pair`
 *    demonstrates that directly.
 *
 * Run with `--expose-gc`; without it the free steps report the wire bytes as
 * still retained and the retained figure is meaningless.
 *
 *   node --expose-gc probe.mjs <artifact.wasm> <compile|instantiate>
 */
import { readFileSync } from "node:fs";

const [wasmPath, mode = "compile"] = process.argv.slice(2);
if (!wasmPath) {
  console.error("usage: probe.mjs <artifact.wasm> [compile|instantiate]");
  process.exit(2);
}

/** The private/shared decomposition, in bytes. */
function memory() {
  const rollup = readFileSync("/proc/self/smaps_rollup", "utf8");
  const field = (name) => {
    const match = rollup.match(new RegExp(`^${name}:\\s+(\\d+) kB`, "m"));
    if (!match) throw new Error(`smaps_rollup has no ${name}`);
    return Number(match[1]) * 1024;
  };
  const privateClean = field("Private_Clean");
  const privateDirty = field("Private_Dirty");
  return {
    privateDirty,
    privateClean,
    sharedClean: field("Shared_Clean"),
    uss: privateClean + privateDirty,
    rss: field("Rss"),
  };
}

/**
 * V8 frees on its own schedule, and a single gc() leaves the last cycle's
 * garbage behind — which lands in the retained column as if the module held it.
 */
function settle() {
  for (let i = 0; i < 4; i++) global.gc();
}

/**
 * Imports are stubbed from the module's own import list rather than from the
 * wasm-bindgen glue, so an artifact built with a different feature set — and so
 * a different import list — needs no matching JS to instantiate.
 */
function stubImports(module) {
  const imports = {};
  for (const { module: ns, name, kind } of WebAssembly.Module.imports(module)) {
    imports[ns] ??= {};
    switch (kind) {
      case "function":
        imports[ns][name] = () => {};
        break;
      case "memory":
        imports[ns][name] = new WebAssembly.Memory({ initial: 1 });
        break;
      case "global":
        imports[ns][name] = new WebAssembly.Global({ value: "i32", mutable: true }, 0);
        break;
      case "table":
        imports[ns][name] = new WebAssembly.Table({ element: "anyfunc", initial: 1 });
        break;
      default:
        throw new Error(`unhandled import kind ${kind}`);
    }
  }
  return imports;
}

settle();
const base = memory();

let bytes = readFileSync(wasmPath);
const wireBytes = bytes.byteLength;
settle();
const afterRead = memory();

let module = new WebAssembly.Module(bytes);
const afterCompile = memory();

let instance = null;
let afterInstantiate = null;
if (mode === "instantiate") {
  instance = new WebAssembly.Instance(module, stubImports(module));
  afterInstantiate = memory();
}

// The wire bytes are a transient the caller can drop; what stays behind with
// only the module (and instance) alive is the per-process price of shipping it.
bytes = null;
settle();
const afterFreeWire = memory();

module = null;
instance = null;
settle();
const afterFreeAll = memory();

process.stdout.write(
  JSON.stringify({
    wasmPath,
    mode,
    wireBytes,
    node: process.version,
    base,
    afterRead,
    afterCompile,
    afterInstantiate,
    afterFreeWire,
    afterFreeAll,
  }) + "\n"
);
