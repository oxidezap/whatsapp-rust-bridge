import { readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const ROOT = join(import.meta.dir, "..");
const DIST = join(ROOT, "dist");
const SHARED = "./voip-runtime.js";

/** A shell that outgrew this is carrying its own bridge copy. */
const MAX_SHELL_BYTES = 8_192;

// Build into a scratch dir, not dist/: a previous run's voip-runtime.js would
// otherwise sit beside the new hashed chunk and read as two chunks.
const SCRATCH = join(DIST, ".voip-entrypoints-tmp");
// A killed or failed run can leave a previous scratch behind; clearing it
// first as well as last keeps the chunk discovery below honest every time.
rmSync(SCRATCH, { recursive: true, force: true });
try {
  const built = await Bun.build({
    entrypoints: [join(ROOT, "ts", "voip.ts"), join(ROOT, "ts", "voip-host.ts")],
    outdir: SCRATCH,
    target: "node",
    minify: true,
    splitting: true,
    external: ["node:fs"],
  });

  if (!built.success) {
    throw new Error(built.logs.map(String).join("\n"));
  }

  const outputs = readdirSync(SCRATCH).filter((name) => name.endsWith(".js"));
  const chunks = outputs.filter(
    (name) => name !== "voip.js" && name !== "voip-host.js",
  );
  if (chunks.length !== 1) {
    throw new Error(
      `build-voip-entrypoints: expected one shared chunk, found [${chunks.join(", ")}]`,
    );
  }
  const chunk = chunks[0]!;
  renameSync(join(SCRATCH, "voip.js"), join(DIST, "voip.js"));
  renameSync(join(SCRATCH, "voip-host.js"), join(DIST, "voip-host.js"));
  renameSync(join(SCRATCH, chunk), join(DIST, "voip-runtime.js"));

  for (const shell of ["voip.js", "voip-host.js"]) {
    const path = join(DIST, shell);
    const source = readFileSync(path, "utf8");
    const rewritten = source.replaceAll(`./${chunk}`, SHARED);
    if (rewritten === source) {
      throw new Error(
        `build-voip-entrypoints: dist/${shell} does not import ./${chunk} — update build-voip-entrypoints.ts`,
      );
    }
    writeFileSync(path, rewritten);
    const size = Buffer.byteLength(rewritten, "utf8");
    if (size > MAX_SHELL_BYTES) {
      throw new Error(
        `build-voip-entrypoints: dist/${shell} is ${size} bytes (over ${MAX_SHELL_BYTES}) — the bridge is no longer shared`,
      );
    }
  }

  console.log(
    `build-voip-entrypoints: dist/voip-runtime.js + thin shells (./${chunk} -> ${SHARED})`,
  );
} finally {
  rmSync(SCRATCH, { recursive: true, force: true });
}
