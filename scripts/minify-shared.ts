import { minify } from "terser";

/** Keep property reads observable: a host adapter may supply a getter. */
export async function minifySharedCode(source: string): Promise<string> {
  const result = await minify(source, {
    module: true,
    compress: { passes: 2, pure_getters: false },
    mangle: true,
  });
  if (!result.code) throw new Error("build-shared-entrypoints: empty compressed chunk");
  return result.code;
}
