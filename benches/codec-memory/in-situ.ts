/**
 * The same changes, inside the real library.
 *
 * Seven bundles of `ts/index.ts` that differ only in what the codec layer does
 * at import — same wasm, same glue, same reader, same wire-batch codecs — so a
 * difference between two of them is the codec layer's.
 *
 *   stock           as published
 *   textcut         657 codec bodies replaced by four throwing methods; every
 *                   export name and the namespace's own work stay identical,
 *                   so what this removes is codec *text* and nothing else
 *   cut             the ping-pong closure kept, the rest stubbed — the largest
 *                   cut a Baileys-compatible client could actually take
 *   lazycodecs      codec objects deferred, `proto` assembled eagerly
 *   lazyns          codecs eager, `proto` assembled on first property read
 *   lazyboth        both — the ceiling on deferring execution
 *   textcut-lazyns  both, over the stubbed codecs — the floor
 *
 * The lazy arms build `proto` behind a Proxy. That is a measurement shape, not
 * a proposal: it answers "what is the most deferral could ever be worth" and
 * nothing about what the published namespace should be.
 *
 * Needs pkg/ — run `bun run build:wasm` first. A dev build works and every arm
 * carries the same wasm either way, so the differences between arms hold; only
 * the absolute totals move, and the doc's are from a release build.
 * Run: bun run bench:codec-memory:in-situ   (REPS, NODE_BIN, NODE_FLAGS honoured)
 */

import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { closure, emit, parse, PING_PONG_ROOTS, ROOT } from "./slice";

const HERE = import.meta.dir;
const WORK = join(ROOT, "target", "codec-memory", "in-situ");
const REPS = Number(process.env.REPS ?? 12);
const NODE = process.env.NODE_BIN ?? "node";
const NODE_FLAGS = (process.env.NODE_FLAGS ?? "").split(" ").filter(Boolean);

if (!existsSync(join(ROOT, "pkg", "whatsapp_rust_bridge.js"))) {
  throw new Error("pkg/ is missing — run `bun run build:wasm` first");
}

const parsed = parse();
const LAZY_CODECS = emit(parsed, new Set(parsed.codecs.keys()), "lazy");
const CODEC_NAMES = [...parsed.codecs.keys()];

/**
 * Same declarations, same export names, same four methods `wrapMessage` looks
 * for — no bodies. Stubbing them as `{}` instead would also stop the namespace
 * recognising them as codecs, and the arm would be measuring two changes.
 */
const stubCodecs = (keep: Set<string>): string =>
  parsed.blocks
    .map((block) => {
      if (block.kind === "createBase") return keep.has(block.name!) ? block.text : "";
      if (block.kind !== "codec" || keep.has(block.name!)) return block.text;
      return [
        `export const ${block.name}: any = {`,
        "  encode(): any { throw new Error('stub'); },",
        "  decode(): any { throw new Error('stub'); },",
        "  create(): any { throw new Error('stub'); },",
        "  fromPartial(): any { throw new Error('stub'); },",
        "};",
      ].join("\n");
    })
    .join("\n");

/** `proto.ts` resolves through the getter object rather than a 31-entry registry. */
const rewriteProtoTs = (source: string): string => {
  const registryStart = source.indexOf("import {\n  Message,");
  const registryEnd = source.indexOf('import * as gen from "./generated/whatsapp";');
  const resolveStart = source.indexOf("function resolve(typeName: string)");
  const resolveEnd = source.indexOf("\n}\n", resolveStart) + 3;
  return [
    source.slice(0, registryStart),
    'import { codecs } from "./generated/whatsapp";\n\n',
    "interface MessageFns<T> {\n  encode(message: T, writer?: any): any;\n",
    "  decode(input: Uint8Array | any, length?: number): T;\n  fromPartial(obj: any): T;\n}\n\n",
    source.slice(registryEnd, resolveStart),
    "function resolve(typeName: string): MessageFns<any> {\n",
    '  const candidate = (codecs as Record<string, any>)[typeName.replaceAll(".", "_")];\n',
    "  if (candidate) return candidate as MessageFns<any>;\n",
    "  throw new Error(`unknown proto type: ${typeName}`);\n}\n",
    source.slice(resolveEnd),
  ].join("");
};

const messagePass = (fromGetterObject: boolean) =>
  fromGetterObject
    ? `  for (const flatName of codecNames) {
    const [cursor, leaf] = place(flatName);
    cursor[leaf] = Object.assign(
      wrapMessage(
        flatName.replaceAll(NAMESPACE_SEPARATOR, PROTO_TYPE_SEPARATOR),
        (codecs as Record<string, any>)[flatName],
      ),
      cursor[leaf] ?? {},
    );
  }
  for (const [flatName, value] of Object.entries(gen)) {
    if (flatName === "codecs" || !isVisibleGeneratedExport(flatName, value) || !isEnumLike(value)) continue;
    const [cursor, leaf] = place(flatName);
    cursor[leaf] = Object.assign({}, cursor[leaf] ?? {}, value);
  }`
    : `  for (const [flatName, value] of Object.entries(gen)) {
    if (!isVisibleGeneratedExport(flatName, value)) continue;
    const [cursor, leaf] = place(flatName);
    if (isMessageFns(value)) {
      cursor[leaf] = Object.assign(
        wrapMessage(flatName.replaceAll(NAMESPACE_SEPARATOR, PROTO_TYPE_SEPARATOR), value),
        cursor[leaf] ?? {},
      );
    } else if (isEnumLike(value)) {
      cursor[leaf] = Object.assign({}, cursor[leaf] ?? {}, value);
    }
  }`;

/** The same tree, assembled on the first property read of `proto`. */
const rewriteNamespaceLazy = (source: string, fromGetterObject: boolean): string => {
  const head = source.slice(0, source.indexOf("const buildNamespace = "));
  return `${head}const buildNamespace = (): Record<string, any> => {
  const root: Record<string, any> = {};
  const place = (flatName: string): [Record<string, any>, string] => {
    const path = flatName.split(NAMESPACE_SEPARATOR);
    const leaf = path.pop()!;
    let cursor = root;
    for (const segment of path) {
      cursor[segment] ??= {};
      cursor = cursor[segment];
    }
    return [cursor, leaf];
  };
${messagePass(fromGetterObject)}
  for (const [alias, target] of Object.entries(HISTORICAL_ALIASES)) {
    if (root[target] && !root[alias]) root[alias] = root[target];
  }
  for (const parent of FORWARD_COMPATIBLE_PARENTS) {
    if (root[parent]) root[parent] = wrapWithForwardCompatibleChildren(root[parent]);
  }
  return root;
};

let assembled: Record<string, any> | undefined;
const namespace = (): Record<string, any> => (assembled ??= buildNamespace());

export const proto: Record<string, any> = new Proxy(Object.create(null) as Record<string, any>, {
  get: (_t, key) => namespace()[key as string],
  has: (_t, key) => (key as string) in namespace(),
  ownKeys: () => Reflect.ownKeys(namespace()),
  getOwnPropertyDescriptor: (_t, key) => {
    const descriptor = Object.getOwnPropertyDescriptor(namespace(), key);
    return descriptor && { ...descriptor, configurable: true };
  },
});
`;
};

const IMPORT_GEN = 'import * as gen from "./generated/whatsapp";';
const IMPORT_LAZY = `${IMPORT_GEN}\nimport { codecs } from "./generated/whatsapp";\nimport { codecNames } from "./codec-names";`;

interface Arm {
  name: string;
  codecModule?: string;
  lazyNamespace?: boolean;
  /** `textcut` cannot run the ping-pong traffic — its codecs throw. */
  touchable: boolean;
}

const PING_PONG = closure(parsed, PING_PONG_ROOTS);
const arms: Arm[] = [
  { name: "stock", touchable: true },
  { name: "textcut", codecModule: stubCodecs(new Set()), touchable: false },
  { name: "cut", codecModule: stubCodecs(PING_PONG), touchable: true },
  { name: "lazycodecs", codecModule: LAZY_CODECS, touchable: true },
  { name: "lazyns", lazyNamespace: true, touchable: true },
  { name: "lazyboth", codecModule: LAZY_CODECS, lazyNamespace: true, touchable: true },
  { name: "textcut-lazyns", codecModule: stubCodecs(new Set()), lazyNamespace: true, touchable: false },
];

mkdirSync(WORK, { recursive: true });
// The bundle resolves the wasm next to itself, the way dist/ ships it.
cpSync(join(ROOT, "pkg", "whatsapp_rust_bridge_bg.wasm"), join(WORK, "whatsapp_rust_bridge_bg.wasm"));
for (const arm of arms) {
  const tree = join(WORK, arm.name);
  rmSync(tree, { recursive: true, force: true });
  cpSync(join(ROOT, "ts"), join(tree, "ts"), { recursive: true });
  symlinkSync(join(ROOT, "pkg"), join(tree, "pkg"));
  symlinkSync(join(ROOT, "node_modules"), join(tree, "node_modules"));

  const lazyCodecs = arm.codecModule === LAZY_CODECS;
  if (arm.codecModule) writeFileSync(join(tree, "ts", "generated", "whatsapp.ts"), arm.codecModule);
  if (lazyCodecs) {
    writeFileSync(
      join(tree, "ts", "codec-names.ts"),
      `export const codecNames: readonly string[] = [\n${CODEC_NAMES.map((n) => `  ${JSON.stringify(n)},`).join("\n")}\n];\n`,
    );
    writeFileSync(join(tree, "ts", "proto.ts"), rewriteProtoTs(readFileSync(join(tree, "ts", "proto.ts"), "utf8")));
  }
  const namespacePath = join(tree, "ts", "proto-namespace.ts");
  let namespaceSource = readFileSync(namespacePath, "utf8");
  if (lazyCodecs) namespaceSource = namespaceSource.replace(IMPORT_GEN, IMPORT_LAZY);
  if (arm.lazyNamespace) {
    namespaceSource = rewriteNamespaceLazy(namespaceSource, lazyCodecs);
  } else if (lazyCodecs) {
    // Eager namespace over a getter object: it reads every getter while
    // assembling, which is the whole point of the `lazycodecs` arm.
    namespaceSource = namespaceSource.replace(
      "for (const [flatName, value] of Object.entries(gen)) {",
      'for (const [flatName, value] of [...codecNames.map((n) => [n, (codecs as any)[n]] as const), ...Object.entries(gen).filter(([n]) => n !== "codecs")]) {',
    );
  }
  writeFileSync(namespacePath, namespaceSource);

  const built = Bun.spawnSync({
    cmd: ["bun", "build", join(tree, "ts", "index.ts"), "--minify", "--target", "node", "--outfile", join(WORK, `${arm.name}.js`)],
    stdout: "pipe",
    stderr: "inherit",
  });
  if (built.exitCode !== 0) throw new Error(`build failed: ${arm.name}`);
}

const runs: { arm: Arm; touch: boolean }[] = [];
for (const arm of arms) {
  runs.push({ arm, touch: false });
  if (arm.touchable) runs.push({ arm, touch: true });
}
const label = (run: { arm: Arm; touch: boolean }) => `${run.arm.name}${run.touch ? " +touch" : ""}`;

const priv = new Map<string, number[]>(runs.map((run) => [label(run), []]));
const heaps = new Map<string, number[]>(runs.map((run) => [label(run), []]));

for (let rep = 0; rep < REPS; rep++) {
  for (const run of runs) {
    const proc = Bun.spawnSync({
      cmd: [
        NODE,
        "--expose-gc",
        ...NODE_FLAGS,
        join(HERE, "in-situ-probe.mjs"),
        join(WORK, `${run.arm.name}.js`),
        ...(run.touch ? ["touch"] : []),
      ],
      stdout: "pipe",
      stderr: "pipe",
    });
    if (proc.exitCode !== 0) throw new Error(`${label(run)} failed: ${proc.stderr.toString().slice(0, 400)}`);
    const row = JSON.parse(proc.stdout.toString());
    priv.get(label(run))!.push(row.delta);
    heaps.get(label(run))!.push(row.heapUsed);
  }
}

const median = (xs: number[]) => {
  const sorted = [...xs].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid]! : (sorted[mid - 1]! + sorted[mid]!) / 2;
};

const version = Bun.spawnSync({ cmd: [NODE, "-v"], stdout: "pipe" }).stdout.toString().trim();
console.log(`reps=${REPS} node=${version} flags=${NODE_FLAGS.join(" ") || "(none)"}`);
console.log(["arm", "bundleKiB", "PrivDirty med", "min", "max", "vs stock", "heapUsed med"].join("\t"));
const stock = median(priv.get("stock")!);
for (const run of runs) {
  const key = label(run);
  const xs = priv.get(key)!;
  console.log(
    [
      key,
      (statSync(join(WORK, `${run.arm.name}.js`)).size / 1024).toFixed(1),
      median(xs).toFixed(0),
      Math.min(...xs),
      Math.max(...xs),
      (median(xs) - stock).toFixed(0),
      median(heaps.get(key)!).toFixed(0),
    ].join("\t"),
  );
}
