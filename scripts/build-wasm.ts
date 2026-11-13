import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new TypeError("invalid build metadata");
  return value as Record<string, unknown>;
}

export async function withSourcePathConfig<T>(flags: string[], work: (path: string) => Promise<T>): Promise<T> {
  const folder = mkdtempSync(join(tmpdir(), "bridge-build-paths-"));
  try {
    const path = join(folder, "source-paths.toml");
    writeFileSync(path, `[target.wasm32-unknown-unknown]\nrustflags = ${JSON.stringify(flags)}\n`, { mode: 0o600 });
    return await work(path);
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
}

export function wasmPackCommand(config: string, args: string[]): string[] {
  const separator = args.indexOf("--");
  const options = separator < 0 ? args : args.slice(0, separator);
  const cargo = separator < 0 ? [] : args.slice(separator + 1);
  return ["wasm-pack", "build", "--target", "web", "--out-dir", "pkg", "--no-pack",
    ...options, "--", "--config", config, ...cargo];
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
  const flags = sourcePathFlags(metadata, root, output("rustc", ["--print", "sysroot"]));
  const status = await withSourcePathConfig(flags, async config => {
    const child = Bun.spawn(wasmPackCommand(config, Bun.argv.slice(2)), {
      cwd: root,
      stdin: "inherit", stdout: "inherit", stderr: "inherit",
    });
    return await child.exited;
  });
  process.exit(status);
}
