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
  // The revision is read from the pin rather than repeated here: the CLI and
  // the macro agree on wasm symbol names only within a revision, and a hint
  // naming a stale one sends a contributor to a build whose exports the
  // generated JavaScript cannot find.
  const pin = join(ROOT, "crates", "bridge-boltffi", "Cargo.toml");
  const rev = readFileSync(pin, "utf8").match(/rev\s*=\s*"([0-9a-f]+)"/)?.[1];
  // Falling back to an install without `--rev` would name the one command that
  // cannot work — it resolves to whatever main happens to be. Say so instead.
  // Reachable without anything being broken: once the pin moves back to a
  // published version, there is no `rev` to read.
  const install = rev
    ? `\`cargo install --git https://github.com/boltffi/boltffi --rev ${rev} boltffi_cli --locked\``
    : "the CLI built from whatever `crates/bridge-boltffi/Cargo.toml` pins — " +
      "no `rev` was found there, so read the pin rather than installing latest";
  console.warn(
    `dist/boltffi is absent — BoltFFI suites skipped. Install ${install} and rebuild to run them.`,
  );
}

/** The artifact's exports, or an empty object when it was not built. */
export const boltffi: Record<string, unknown> = boltffiAvailable
  ? await import(BOLTFFI_ENTRY)
  : {};
