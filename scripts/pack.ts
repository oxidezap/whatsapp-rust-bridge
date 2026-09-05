/**
 * What `npm pack` would ship, asked once.
 *
 * Both publish guards measure the tarball, and each measuring it its own way
 * is how they end up disagreeing about what "the package" is — a flag added
 * here and not there, and one of them is checking something the other is not.
 */
import { execSync } from "node:child_process";
import { join } from "node:path";

export const ROOT = join(import.meta.dir, "..");

export type PackedFile = { path: string; size: number };

export type Packed = {
  files: PackedFile[];
  /** Total size of the files as they land on disk, not the compressed tarball. */
  unpackedSize: number;
};

/**
 * `--ignore-scripts` because `prepack` is one of the callers: without it the
 * guard would re-enter itself.
 */
export function packedContents(): Packed {
  const raw = execSync("npm pack --dry-run --json --ignore-scripts", {
    cwd: ROOT,
    encoding: "utf8",
  });
  return parsePackOutput(raw);
}

/** Parse and validate one `npm pack --dry-run --json` document. */
export function parsePackOutput(raw: string): Packed {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("pack: npm pack --dry-run --json did not emit JSON");
  }
  // npm 12 emits one record keyed by package name; older npm emitted a
  // single-element array. Anything else (zero or several records, a
  // non-object) fails closed: the guards must measure exactly this package.
  const records: unknown[] = Array.isArray(parsed) ? parsed : Object.values(parsed ?? {});
  if (records.length !== 1) {
    throw new Error(`pack: expected one packed record, got ${records.length}`);
  }
  const record = records[0] as { files?: unknown; unpackedSize?: unknown };
  if (typeof record !== "object" || record === null) {
    throw new Error("pack: packed record is not an object");
  }
  if (!Array.isArray(record.files)) {
    throw new Error("pack: packed record has no files array");
  }
  for (const file of record.files) {
    const entry = file as { path?: unknown; size?: unknown };
    if (typeof entry?.path !== "string" || typeof entry?.size !== "number") {
      throw new Error("pack: packed record has a malformed file entry");
    }
  }
  if (
    typeof record.unpackedSize !== "number" ||
    !Number.isFinite(record.unpackedSize)
  ) {
    throw new Error("pack: packed record has no finite unpackedSize");
  }
  return {
    files: record.files as PackedFile[],
    unpackedSize: record.unpackedSize,
  };
}
