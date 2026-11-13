import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new TypeError("invalid build metadata");
  return value as Record<string, unknown>;
}

export function targetFlags(config: unknown): string[] {
  const flags = record(record(record(config).target)["wasm32-unknown-unknown"]).rustflags;
  if (!Array.isArray(flags) || !flags.every((flag): flag is string => typeof flag === "string")) {
    throw new TypeError("invalid WASM target rustflags");
  }
  return flags;
}

export function inheritedFlags(env: NodeJS.ProcessEnv, configured: string[]): string[] {
  if (env.CARGO_ENCODED_RUSTFLAGS !== undefined) return env.CARGO_ENCODED_RUSTFLAGS.split("\x1f").filter(Boolean);
  if (env.RUSTFLAGS !== undefined) return env.RUSTFLAGS.split(/\s+/).filter(Boolean);
  return configured;
}

export function sourcePathFlags(metadata: unknown, root: string, sysroot: string): string[] {
  const packages = record(metadata).packages;
  if (!Array.isArray(packages)) throw new TypeError("invalid Cargo packages");
  const paths = new Map([[root, "bridge"]]);
  let coreFound = false;
  for (const item of packages) {
    const { name, version, source, manifest_path: manifest } = record(item);
    if (typeof name !== "string" || typeof version !== "string" || typeof manifest !== "string" ||
        (source !== null && typeof source !== "string")) throw new TypeError("invalid Cargo package");
    if (name === "whatsapp-rust") {
      if (coreFound) throw new TypeError("multiple core sources");
      paths.set(dirname(manifest), "core");
      coreFound = true;
    } else if (source?.startsWith("registry+")) {
      paths.set(dirname(manifest), `${name}-${version}`);
    }
  }
  if (!coreFound) throw new TypeError("locked core source missing");
  paths.set(join(sysroot, "lib/rustlib/src/rust/library"), "rust/library");
  return [...paths].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)
    .map(([from, to]) => `--remap-path-prefix=${from}=${to}`);
}

if (import.meta.main) {
  const root = fileURLToPath(new URL("../", import.meta.url)).replace(/[\\/]$/, "");
  const output = (program: string, args: string[]): string => {
    const result = Bun.spawnSync([program, ...args], { cwd: root, stdout: "pipe", stderr: "pipe" });
    if (result.exitCode !== 0) throw new Error(`${program} failed: ${result.stderr.toString()}`);
    return result.stdout.toString().trim();
  };
  const metadata: unknown = JSON.parse(output("cargo", ["metadata", "--locked", "--format-version", "1"]));
  const config: unknown = Bun.TOML.parse(readFileSync(join(root, ".cargo/config.toml"), "utf8"));
  const flags = [
    ...inheritedFlags(process.env, targetFlags(config)),
    ...sourcePathFlags(metadata, root, output("rustc", ["--print", "sysroot"])),
  ];
  const child = Bun.spawn([
    "wasm-pack", "build", "--target", "web", "--out-dir", "pkg", "--no-pack", ...Bun.argv.slice(2),
  ], {
    cwd: root,
    env: { ...process.env, CARGO_ENCODED_RUSTFLAGS: flags.join("\x1f") },
    stdin: "inherit", stdout: "inherit", stderr: "inherit",
  });
  process.exit(await child.exited);
}
