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
  // The pin is read rather than repeated here: the CLI and the macro agree on
  // wasm symbol names only within a version, and a hint naming a stale one
  // sends a contributor to a build whose exports the generated JavaScript
  // cannot find. Either form of pin is understood, because this pinned a main
  // revision for as long as no release carried the fixes the backend needs.
  const pin = readFileSync(join(ROOT, "crates", "bridge-boltffi", "Cargo.toml"), "utf8");
  const version = pin.match(/^boltffi\s*=\s*\{[^}]*\bversion\s*=\s*"([^"]+)"/m)?.[1];
  const rev = pin.match(/^boltffi\s*=\s*\{[^}]*\brev\s*=\s*"([0-9a-f]+)"/m)?.[1];
  // Naming neither would name the one command that cannot work: an install
  // without a pin resolves to whatever is latest, which is the mismatch this
  // whole hint exists to avoid.
  const install = version
    ? `\`cargo install boltffi_cli --version ${version} --locked\``
    : rev
      ? `\`cargo install --git https://github.com/boltffi/boltffi --rev ${rev} boltffi_cli --locked\``
      : "the CLI matching whatever `crates/bridge-boltffi/Cargo.toml` pins — " +
        "neither a version nor a `rev` was found there, so read the pin rather " +
        "than installing latest";
  console.warn(
    `dist/boltffi is absent — BoltFFI suites skipped. Install ${install} and rebuild to run them.`,
  );
}

/** The artifact's exports, or an empty object when it was not built. */
export const boltffi: Record<string, unknown> = boltffiAvailable
  ? await import(BOLTFFI_ENTRY)
  : {};
