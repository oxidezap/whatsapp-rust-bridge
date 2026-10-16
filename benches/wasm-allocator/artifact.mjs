/**
 * Loads one artifact from `benches/wasm-module-rss/artifacts/` the way
 * `ts/index.ts` loads the shipped one: read the bytes, `initSync`, use the
 * exports. The glue is wasm-bindgen `--target web` output, so the module URL
 * it would fetch never comes into it.
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const artifactDir = join(here, "..", "wasm-module-rss", "artifacts");

export async function loadArtifact(name) {
  const wasm = join(artifactDir, `${name}.wasm`);
  const glue = join(artifactDir, `${name}.glue.js`);
  const module = await import(pathToFileURL(glue).href);
  module.initSync({ module: readFileSync(wasm) });
  return module;
}
