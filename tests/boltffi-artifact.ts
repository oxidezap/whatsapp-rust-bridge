/**
 * Locates the BoltFFI artifact for the suites that exercise it.
 *
 * `build:boltffi` skips itself when `boltffi` is not installed, so a
 * contributor without the CLI still gets a working `bun run build`. The tests
 * have to hold up the same end: importing the artifact statically would turn
 * that supported state into a module-resolution error across three files, and
 * `bun run build && bun test` is the documented workflow.
 *
 * CI and the release runners install the CLI, so the artifact is present there
 * and none of these suites skip.
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..");

export const BOLTFFI_ENTRY = join(ROOT, "dist", "boltffi", "pkg", "node.js");
export const BOLTFFI_DTS = join(
  ROOT,
  "dist",
  "boltffi",
  "pkg",
  "whatsapp_rust_bridge_boltffi_node.d.ts",
);

export const boltffiAvailable = existsSync(BOLTFFI_ENTRY) && existsSync(BOLTFFI_DTS);

if (!boltffiAvailable) {
  const pin = readFileSync(join(ROOT, "crates", "bridge-boltffi", "Cargo.toml"), "utf8");
  const version = pin.match(/^boltffi\s*=\s*\{[^}]*\bversion\s*=\s*"=([^"]+)"/m)?.[1];
  if (version === undefined) throw new Error("The BoltFFI crate must pin an exact release.");
  const install = `\`cargo install boltffi_cli --version ${version} --locked\``;
  console.warn(
    `dist/boltffi is absent — BoltFFI suites skipped. Install ${install} and rebuild to run them.`,
  );
}

/** The artifact's exports, or an empty object when it was not built. */
export const boltffi: Record<string, unknown> = boltffiAvailable
  ? await import(BOLTFFI_ENTRY)
  : {};
