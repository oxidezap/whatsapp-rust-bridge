import { expect, test } from "bun:test";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { sourcePathFlags, wasmPackCommand, withSourcePathConfig } from "../scripts/build-wasm";

const metadata = {
  packages: [
    { name: "whatsapp-rust", version: "0.7.0", source: "git+https://github.com/oxidezap/whatsapp-rust?rev=abc#abc", manifest_path: "/cargo/git/core/Cargo.toml" },
    { name: "bytes", version: "1.12.0", source: "registry+https://github.com/rust-lang/crates.io-index", manifest_path: "/cargo/registry/bytes-1.12.0/Cargo.toml" },
  ],
};

test("WASM builds preserve wasm-pack options and Cargo argument forwarding", () => {
  const base = ["wasm-pack", "build", "--target", "web", "--out-dir", "pkg", "--no-pack"];
  expect(wasmPackCommand("paths.toml", [])).toEqual([...base, "--", "--config", "paths.toml"]);
  expect(wasmPackCommand("paths.toml", ["--dev", "--", "--no-default-features"])).toEqual([
    ...base, "--dev", "--", "--config", "paths.toml", "--no-default-features",
  ]);
});

test("Cargo preserves configured, target-environment and global compiler flags", async () => {
  const folder = mkdtempSync(join(tmpdir(), "bridge cargo flags-"));
  let configPath = "";
  try {
    mkdirSync(join(folder, ".cargo"));
    copyFileSync(new URL("../rust-toolchain.toml", import.meta.url), join(folder, "rust-toolchain.toml"));
    writeFileSync(join(folder, "Cargo.toml"), '[package]\nname="path-flag-probe"\nversion="0.0.0"\nedition="2024"\n[lib]\npath="lib.rs"\n[lints.rust]\nunexpected_cfgs={level="deny",check-cfg=["cfg(configured)","cfg(extra)","cfg(target_env_flag)","cfg(global_env_flag)","cfg(encoded_env_flag)"]}\n');
    writeFileSync(join(folder, ".cargo/config.toml"), '[target.wasm32-unknown-unknown]\nrustflags=["--cfg","configured"]\n');
    writeFileSync(join(folder, "lib.rs"), '#[unsafe(no_mangle)]\npub static FLAGS:u8=(cfg!(configured) as u8)|((cfg!(extra) as u8)<<1)|((cfg!(target_env_flag) as u8)<<2)|((cfg!(global_env_flag) as u8)<<3)|((cfg!(encoded_env_flag) as u8)<<4);\n');
    const inherited = { ...process.env };
    delete inherited.RUSTFLAGS;
    delete inherited.CARGO_ENCODED_RUSTFLAGS;
    delete inherited.CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS;
    const cases: [NodeJS.ProcessEnv, number][] = [
      [{}, 3],
      [{ CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS: "--cfg target_env_flag" }, 7],
      [{ RUSTFLAGS: "--cfg global_env_flag" }, 8],
      [{ RUSTFLAGS: "--cfg global_env_flag", CARGO_ENCODED_RUSTFLAGS: "--cfg\x1fencoded_env_flag" }, 16],
    ];
    await withSourcePathConfig(["--cfg", "extra"], async config => {
      configPath = config;
      for (const [env, expected] of cases) {
        const ir = join(folder, `flags-${expected}.ll`);
        const result = Bun.spawnSync([
          "cargo", "rustc", "--lib", "--target", "wasm32-unknown-unknown", "--config", config,
          "--", `--emit=llvm-ir=${ir}`,
        ], { cwd: folder, env: { ...inherited, ...env }, stdout: "pipe", stderr: "pipe" });
        expect(result.exitCode, result.stderr.toString()).toBe(0);
        const value = readFileSync(ir, "utf8").match(/@FLAGS = .*c"\\([0-9A-Fa-f]{2})"/);
        expect(value).not.toBeNull();
        expect(parseInt(value![1]!, 16)).toBe(expected);
      }
    });
    expect(existsSync(configPath)).toBe(false);
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
});

test("source mappings retain dependency names and versions with stable ordering", () => {
  const flags = sourcePathFlags(metadata, "/project", "/toolchain");
  expect(flags).toEqual([
    "--remap-path-prefix=/cargo/git/core=core",
    "--remap-path-prefix=/cargo/registry/bytes-1.12.0=bytes-1.12.0",
    "--remap-path-prefix=/project=bridge",
    `--remap-path-prefix=${join("/toolchain", "lib/rustlib/src/rust/library")}=rust/library`,
  ]);
  expect(sourcePathFlags({ packages: [...metadata.packages].reverse() }, "/project", "/toolchain")).toEqual(flags);
  expect(sourcePathFlags({ packages: [{ ...metadata.packages[0], source: null }] }, "/project", "/toolchain")).toContain("--remap-path-prefix=/cargo/git/core=core");
  expect(() => sourcePathFlags({ packages: [] }, "/project", "/toolchain")).toThrow("locked core source missing");
  expect(() => sourcePathFlags({ packages: [{ ...metadata.packages[0], manifest_path: null }] }, "/project", "/toolchain")).toThrow();
});

test("rustc remapping preserves diagnostic file and line information", () => {
  const folder = mkdtempSync(join(tmpdir(), "bridge-source-paths-"));
  try {
    const source = join(folder, "diagnostic.rs");
    const ir = join(folder, "diagnostic.ll");
    writeFileSync(source, '#[unsafe(no_mangle)]\npub static LOCATION: &str = concat!(file!(), ":", line!());\n');
    const result = Bun.spawnSync([
      "rustc", "--crate-type=lib", "--emit=llvm-ir", source, "-o", ir, `--remap-path-prefix=${folder}=bridge`,
    ], { stdout: "pipe", stderr: "pipe" });
    expect(result.exitCode, result.stderr.toString()).toBe(0);
    const output = readFileSync(ir, "utf8");
    expect(output).toContain('c"bridge');
    expect(output).toContain("diagnostic.rs:2");
    expect(output).not.toContain(folder);
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
});
