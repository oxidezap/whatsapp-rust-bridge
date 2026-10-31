/**
 * Publish guard: fails when the npm tarball would ship duplicated or
 * forbidden content (e.g. the pkg/ copy of proto-types.d.ts that used to
 * double the package).
 */
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { ROOT, packedContents } from "./pack";

const MIN_DUP_BYTES = 4096;

const files: string[] = packedContents().files.map((f) => f.path);

const errors: string[] = [];

for (const path of files) {
  if (path.startsWith("pkg/")) {
    errors.push(`forbidden path in tarball: ${path} (dist/ is the only publish root)`);
  }
}

// Every path `exports` promises has to be in the tarball. `build:boltffi`
// skips itself when `boltffi` is not installed, which is right for a
// contributor and wrong for a publish: without this the package would ship a
// `./boltffi` entry resolving to nothing, and the consumer would find out.
const packageJson = JSON.parse(readFileSync(join(ROOT, "package.json"), "utf8"));
const exported = new Set<string>();
const collect = (value: unknown) => {
  if (typeof value === "string") {
    if (value.startsWith("./")) exported.add(value.slice(2));
    return;
  }
  if (value && typeof value === "object") Object.values(value).forEach(collect);
};
collect(packageJson.exports);
for (const path of ["main", "types"] as const) {
  collect(packageJson[path]);
}

const shipped = new Set(files);
for (const path of [...exported].sort()) {
  if (!shipped.has(path)) {
    errors.push(`package.json points at ${path}, which the tarball does not contain`);
  }
}

const byHash = new Map<string, string[]>();
for (const path of files) {
  const bytes = readFileSync(join(ROOT, path));
  if (bytes.length < MIN_DUP_BYTES) continue;
  const hash = createHash("sha256").update(bytes).digest("hex");
  byHash.set(hash, [...(byHash.get(hash) ?? []), path]);
}
for (const paths of byHash.values()) {
  if (paths.length > 1) {
    errors.push(`duplicated content in tarball: ${paths.join(" == ")}`);
  }
}

if (errors.length > 0) {
  console.error(errors.map((e) => `check-pack: ${e}`).join("\n"));
  process.exit(1);
}
console.log(`check-pack: ${files.length} files, no duplicates`);
