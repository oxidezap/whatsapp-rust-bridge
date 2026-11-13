import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { inheritedFlags, sourcePathFlags, targetFlags } from "../scripts/build-wasm";

const configured = ["-C", "target-feature=+simd128", "--cfg", 'getrandom_backend="wasm_js"'];
const metadata = {
  packages: [
    { name: "whatsapp-rust", version: "0.7.0", source: "git+https://github.com/oxidezap/whatsapp-rust?rev=abc#abc", manifest_path: "/cargo/git/core/Cargo.toml" },
    { name: "bytes", version: "1.12.0", source: "registry+https://github.com/rust-lang/crates.io-index", manifest_path: "/cargo/registry/bytes-1.12.0/Cargo.toml" },
  ],
};

test("WASM builds retain target flags and Cargo environment precedence", () => {
  expect(targetFlags({ target: { "wasm32-unknown-unknown": { rustflags: configured } } })).toEqual(configured);
  expect(inheritedFlags({}, configured)).toEqual(configured);
  expect(inheritedFlags({ RUSTFLAGS: "--cfg custom" }, configured)).toEqual(["--cfg", "custom"]);
  expect(inheritedFlags({ RUSTFLAGS: "ignored", CARGO_ENCODED_RUSTFLAGS: "--cfg\x1fcustom" }, configured)).toEqual(["--cfg", "custom"]);
  expect(inheritedFlags({ CARGO_ENCODED_RUSTFLAGS: "" }, configured)).toEqual([]);
  expect(() => targetFlags({ target: { "wasm32-unknown-unknown": { rustflags: [1] } } })).toThrow();
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
