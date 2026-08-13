/**
 * How a wasm module's code section is distributed across functions.
 *
 * Two callers read the same parse: `wasm-fn-sizes.mjs` prints it, and
 * `check-wasm-shape.mjs` gates on the largest body. Keeping one parser is what
 * keeps the number the gate enforces the number the report shows.
 *
 * Body bytes, not function extents — they line up with the indices V8 prints
 * under `--trace-wasm-compilation-times`. Names come back when the module
 * still carries a name section; the shipped one does not (`strip = true` plus
 * wasm-opt's `--strip-debug`).
 */
import { readFileSync } from "node:fs";

export function codeShape(path) {
  const buf = readFileSync(path);
  let at = 0;
  const byte = () => buf[at++];
  const u32 = () => {
    const v = buf.readUInt32LE(at);
    at += 4;
    return v;
  };
  const uleb = () => {
    let result = 0;
    let shift = 0;
    let b;
    do {
      b = buf[at++];
      result += (b & 0x7f) * 2 ** shift;
      shift += 7;
    } while (b & 0x80);
    return result;
  };

  if (u32() !== 0x6d736100) throw new Error(`${path} is not a wasm module`);
  u32();

  let importedFunctions = 0;
  const bodies = [];
  const names = new Map();

  while (at < buf.length) {
    const id = byte();
    const size = uleb();
    const start = at;

    if (id === 2) {
      // Imported functions come first in the index space, so the code section's
      // Nth body is function N + importedFunctions.
      const count = uleb();
      for (let i = 0; i < count; i++) {
        const moduleLength = uleb();
        at += moduleLength;
        const fieldLength = uleb();
        at += fieldLength;
        const kind = byte();
        if (kind === 0) {
          uleb();
          importedFunctions++;
        } else if (kind === 1) {
          byte();
          const flags = byte();
          uleb();
          if (flags & 1) uleb();
        } else if (kind === 2) {
          const flags = byte();
          uleb();
          if (flags & 1) uleb();
        } else if (kind === 3) {
          byte();
          byte();
        } else if (kind === 4) {
          byte();
          uleb();
        } else {
          throw new Error(`unknown import kind ${kind}`);
        }
      }
    } else if (id === 10) {
      const count = uleb();
      for (let i = 0; i < count; i++) {
        const bodySize = uleb();
        bodies.push({ index: importedFunctions + i, size: bodySize });
        at += bodySize;
      }
    } else if (id === 0) {
      const nameLength = uleb();
      const sectionName = buf.subarray(at, at + nameLength).toString("utf8");
      at += nameLength;
      if (sectionName === "name") {
        const end = start + size;
        while (at < end) {
          const subsection = byte();
          const subsectionSize = uleb();
          const subsectionEnd = at + subsectionSize;
          if (subsection === 1) {
            const count = uleb();
            for (let i = 0; i < count; i++) {
              const index = uleb();
              const length = uleb();
              names.set(index, buf.subarray(at, at + length).toString("utf8"));
              at += length;
            }
          }
          at = subsectionEnd;
        }
      }
    }

    at = start + size;
  }

  // A module with no code section would report a largest body of `undefined`
  // and a median of NaN — numbers a gate would then compare against and pass.
  if (bodies.length === 0) throw new Error(`${path} has no code section`);

  const bySize = [...bodies].sort((a, b) => a.size - b.size);
  const total = bodies.reduce((sum, f) => sum + f.size, 0);

  // Averaging the middle pair on an even function count. A module with an even
  // count is the normal case here, so taking the upper of the two would quietly
  // report the larger half's smallest function as typical.
  const middle = bySize.length >> 1;
  const medianBody =
    bySize.length % 2
      ? bySize[middle].size
      : (bySize[middle - 1].size + bySize[middle].size) / 2;

  return {
    path,
    bytes: buf.length,
    bodies,
    // Largest first, which is the order both callers want.
    bySize: bySize.reverse(),
    importedFunctions,
    names,
    total,
    medianBody,
    largestBody: bySize[0].size,
  };
}
