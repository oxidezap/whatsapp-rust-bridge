/**
 * Build the two JS entrypoints (`dist/index.js`, `dist/host.js`) over one
 * shared implementation chunk, so the package ships two entries without
 * paying for the bridge twice.
 *
 * `bun build --splitting` emits `index.js` + `host.js` as thin re-export
 * shells over a content-hashed `bridge-<hash>.js` chunk. The hash names the
 * bytes: an unchanged bridge rebuilds to the same chunk name, so the shells
 * stay tiny and the tarball carries one copy of the implementation.
 *
 * The chunk name is a build artifact, not a contract: this script renames it
 * to the stable `dist/bridge.js` and rewrites the shells' import specifier,
 * then asserts the shells are actually thin (a shell that grew its own copy
 * of the bridge is the duplication this script exists to catch, not to ship).
 * `dist/bridge.js` is internal — it is not in `package.json` `exports` — but
 * NodeNext still resolves the `./bridge.js` specifier by exact file name, so
 * the published `.d.ts` rewrite in `finalize-dist.ts` covers it like the rest.
 *
 * Run: `bun run scripts/build-shared-entrypoints.ts` (via `build:ts`).
 */
import { readdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..");
const DIST = join(ROOT, "dist");
const SHARED = "./bridge.js";

/** A shell that outgrew this is carrying its own bridge copy. */
const MAX_SHELL_BYTES = 8_192;

const built = await Bun.build({
  entrypoints: [join(ROOT, "ts", "index.ts"), join(ROOT, "ts", "host.ts")],
  outdir: DIST,
  target: "node",
  minify: true,
  splitting: true,
  external: ["node:fs"],
});

if (!built.success) {
  console.error(built.logs.map(String).join("\n"));
  process.exit(1);
}

const outputs = readdirSync(DIST).filter((name) => name.endsWith(".js"));
const chunks = outputs.filter(
  (name) => name !== "index.js" && name !== "host.js" && name !== "proto-types.js",
);
if (chunks.length !== 1) {
  throw new Error(
    `build-shared-entrypoints: expected one shared chunk, found [${chunks.join(", ")}]`,
  );
}
const chunk = chunks[0]!;
renameSync(join(DIST, chunk), join(DIST, "bridge.js"));

for (const shell of ["index.js", "host.js"]) {
  const path = join(DIST, shell);
  const source = readFileSync(path, "utf8");
  const rewritten = source.replaceAll(`./${chunk}`, SHARED);
  if (rewritten === source) {
    throw new Error(
      `build-shared-entrypoints: dist/${shell} does not import ./${chunk} — update build-shared-entrypoints.ts`,
    );
  }
  writeFileSync(path, rewritten);
  const size = Buffer.byteLength(rewritten, "utf8");
  if (size > MAX_SHELL_BYTES) {
    throw new Error(
      `build-shared-entrypoints: dist/${shell} is ${size} bytes (over ${MAX_SHELL_BYTES}) — the bridge is no longer shared`,
    );
  }
}

console.log(
  `build-shared-entrypoints: dist/bridge.js + thin shells (./${chunk} -> ${SHARED})`,
);
