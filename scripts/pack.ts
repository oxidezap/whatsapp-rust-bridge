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
  const [packed] = JSON.parse(
    execSync("npm pack --dry-run --json --ignore-scripts", { cwd: ROOT, encoding: "utf8" })
  );
  return { files: packed.files, unpackedSize: packed.unpackedSize };
}
