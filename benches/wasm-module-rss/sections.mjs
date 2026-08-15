/**
 * Section sizes of a wasm binary, in bytes.
 *
 * The interesting column is `code`: it is the part V8 has to keep a copy of and
 * the part it later compiles, and it is not the file size — the data and name
 * sections move independently of it.
 */
import { readFileSync } from "node:fs";

const SECTION_NAMES = [
  "custom",
  "type",
  "import",
  "function",
  "table",
  "memory",
  "global",
  "export",
  "start",
  "element",
  "code",
  "data",
  "datacount",
  "tag",
];

export function sectionSizes(path) {
  const bytes = readFileSync(path);
  if (bytes.readUInt32LE(0) !== 0x6d736100) throw new Error(`${path} is not a wasm binary`);

  let offset = 8;
  const sizes = { total: bytes.byteLength };
  while (offset < bytes.byteLength) {
    const id = bytes[offset++];
    let size = 0;
    let shift = 0;
    for (;;) {
      const byte = bytes[offset++];
      size |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) break;
      shift += 7;
    }
    let name = SECTION_NAMES[id] ?? `section${id}`;
    if (id === 0) {
      // A custom section carries its own name; `name` and the debug sections
      // are the ones worth telling apart from each other.
      let cursor = offset;
      let nameLen = 0;
      let nameShift = 0;
      for (;;) {
        const byte = bytes[cursor++];
        nameLen |= (byte & 0x7f) << nameShift;
        if ((byte & 0x80) === 0) break;
        nameShift += 7;
      }
      name = `custom:${bytes.toString("utf8", cursor, cursor + nameLen)}`;
    }
    // The count of code bodies, not just their bytes: V8's per-function
    // bookkeeping scales with it independently of how big the bodies are, and
    // a build that leaves inlining off has twice the functions for the same
    // work. Reporting only bytes makes that look like noise on the slope.
    if (id === 10) {
      let count = 0;
      let countShift = 0;
      let cursor = offset;
      for (;;) {
        const byte = bytes[cursor++];
        count |= (byte & 0x7f) << countShift;
        if ((byte & 0x80) === 0) break;
        countShift += 7;
      }
      sizes.functionCount = count;
    }
    sizes[name] = (sizes[name] ?? 0) + size;
    offset += size;
  }
  return sizes;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  for (const path of process.argv.slice(2)) {
    console.log(path, JSON.stringify(sectionSizes(path)));
  }
}
