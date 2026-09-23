import { join } from "node:path";

const root = join(import.meta.dir, "..");
const built = await Bun.build({
  entrypoints: [join(root, "ts/voip.ts"), join(root, "ts/voip-host.ts")],
  outdir: join(root, "dist"),
  target: "node",
  minify: true,
  external: ["node:fs"],
});
if (!built.success) {
  throw new Error(built.logs.map(String).join("\n"));
}
