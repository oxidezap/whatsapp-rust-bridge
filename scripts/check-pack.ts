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
