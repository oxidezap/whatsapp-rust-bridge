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
 *   cut             the ping-pong closure kept, the rest stubbed under the same
 *                   export names — codec bodies removed and nothing else
 *   cut-real        the ping-pong closure and nothing else: the removed types
 *                   are not exported, so the namespace never wraps them. This
 *                   is what the consumer-declared cut would actually ship, and
 *                   it does not run without the API change
 *   lazycodecs      codec objects deferred, `proto` assembled eagerly
 *   lazyns          codecs eager, the whole `proto` tree on first read
 *   lazyboth        both — the ceiling on deferring execution
 *   lazyns-pertype  codecs eager, one lazy getter per type in `proto`
 *   lazyboth-pertype  both, per type — the shape that could actually ship
 *   textcut-lazyns  both, over the stubbed codecs — the floor
 *
 * The `whole` arms put `proto` behind a Proxy, which is a measurement shape and
 * not a proposal: they answer "what is the most deferral could ever be worth"
 * and, once a client touches one type, they build all 657. The `pertype` arms
 * are the honest control for a shippable lazy namespace — a plain object whose
 * types materialize one at a time.
 *
 * Needs pkg/ — run `bun run build:wasm` first. A dev build works and every arm
 * carries the same wasm either way, so the differences between arms hold; only
 * the absolute totals move, and the doc's are from a release build.
 * Run: bun run bench:codec-memory:in-situ   (REPS, NODE_BIN, NODE_FLAGS honoured)
 */

import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { shuffled } from "./schedule";
import { assertRoundTrip, closure, emit, parse, PING_PONG_ROOTS, ROOT } from "./slice";

const HERE = import.meta.dir;
const WORK = join(ROOT, "target", "codec-memory", "in-situ");
const REPS = Number(process.env.REPS ?? 15);
if (!Number.isSafeInteger(REPS) || REPS < 1) throw new Error(`REPS must be a positive integer, got ${process.env.REPS}`);
const NODE = process.env.NODE_BIN ?? "node";
const NODE_FLAGS = (process.env.NODE_FLAGS ?? "").split(" ").filter(Boolean);

if (!existsSync(join(ROOT, "pkg", "whatsapp_rust_bridge.js"))) {
  throw new Error("pkg/ is missing — run `bun run build:wasm` first");
}

/**
 * A moved anchor is the dangerous failure: `slice(-1)` still bundles, and the
 * arm reports a number for a variant it is not. So assert rather than trust.
 */
const anchor = (source: string, needle: string, what: string): number => {
  const at = source.indexOf(needle);
  if (at < 0) throw new Error(`${what}: anchor not found — ${JSON.stringify(needle.slice(0, 40))}`);
  return at;
};

const parsed = parse();
assertRoundTrip(parsed);
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
  const registryStart = anchor(source, "import {\n  Message,", "proto.ts");
  const registryEnd = anchor(source, 'import * as gen from "./generated/whatsapp";', "proto.ts");
  const resolveStart = anchor(source, "function resolve(typeName: string)", "proto.ts");
  const resolveEnd = anchor(source.slice(resolveStart), "\n}\n", "proto.ts") + resolveStart + 3;
  return [
    source.slice(0, registryStart),
    'import { codecs } from "./generated/whatsapp";\n\n',
    "interface MessageFns<T> {\n  encode(message: T, writer?: any): any;\n",
    "  decode(input: Uint8Array | any, length?: number): T;\n  fromPartial(obj: any): T;\n}\n\n",
    source.slice(registryEnd, resolveStart),
    // The names the stock registry accepts that are not the generated
    // spelling. Dropping them would make the arms differ by more than when
    // a codec is built.
    // Null-prototype: a plain literal would answer `constructor` and friends
    // from `Object.prototype`, and stock's registry lookup does not.
    "const REGISTRY_ALIASES: Record<string, string> = Object.assign(Object.create(null), {\n",
    '  AdvSignedDeviceIdentity: "ADVSignedDeviceIdentity",\n',
    '  AdvSignedKeyIndexList: "ADVSignedKeyIndexList",\n',
    '  AdvDeviceIdentity: "ADVDeviceIdentity",\n',
    '  AdvSignedDeviceIdentityHmac: "ADVSignedDeviceIdentityHMAC",\n',
    '  LidMigrationMappingSyncPayload: "LIDMigrationMappingSyncPayload",\n',
    "});\n\n",
    "function resolve(typeName: string): MessageFns<any> {\n",
    '  const flat = REGISTRY_ALIASES[typeName] ?? typeName.replaceAll(".", "_");\n',
    "  const candidate = (codecs as Record<string, any>)[flat];\n",
    "  if (candidate) return candidate as MessageFns<any>;\n",
    "  throw new Error(`unknown proto type: ${typeName}`);\n}\n",
    source.slice(resolveEnd),
  ].join("");
};

/** Same resolver, over the star import, for the arm whose module lost exports. */
const rewriteProtoTsForCut = (source: string): string => {
  const registryStart = anchor(source, "import {\n  Message,", "proto.ts (cut)");
  const registryEnd = anchor(source, 'import * as gen from "./generated/whatsapp";', "proto.ts (cut)");
  const resolveStart = anchor(source, "function resolve(typeName: string)", "proto.ts (cut)");
  const resolveEnd = anchor(source.slice(resolveStart), "\n}\n", "proto.ts (cut)") + resolveStart + 3;
  return [
    source.slice(0, registryStart),
    "interface MessageFns<T> {\n  encode(message: T, writer?: any): any;\n",
    "  decode(input: Uint8Array | any, length?: number): T;\n  fromPartial(obj: any): T;\n}\n\n",
    source.slice(registryEnd, resolveStart),
    // The kept types keep their registry spellings; only the removed names go.
    // Null-prototype: a plain literal would answer `constructor` and friends
    // from `Object.prototype`, and stock's registry lookup does not.
    "const REGISTRY_ALIASES: Record<string, string> = Object.assign(Object.create(null), {\n",
    '  AdvSignedDeviceIdentity: "ADVSignedDeviceIdentity",\n',
    '  AdvSignedKeyIndexList: "ADVSignedKeyIndexList",\n',
    '  AdvDeviceIdentity: "ADVDeviceIdentity",\n',
    '  AdvSignedDeviceIdentityHmac: "ADVSignedDeviceIdentityHMAC",\n',
    '  LidMigrationMappingSyncPayload: "LIDMigrationMappingSyncPayload",\n',
    "});\n\n",
    "function resolve(typeName: string): MessageFns<any> {\n",
    '  const candidate = GENERATED_MODULE[REGISTRY_ALIASES[typeName] ?? typeName.replaceAll(".", "_")];\n',
    '  if (candidate && typeof candidate === "object" && "encode" in candidate) {\n',
    "    return candidate as MessageFns<any>;\n  }\n",
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

/**
 * The tree built at import, but each type materialized on first read and then
 * written back as a plain property.
 *
 * Deferring the whole namespace behind one Proxy answers "what is the most
 * deferral could be worth"; it does not answer "what does a client that uses
 * six types keep", because the first read builds all 657. This one does, and
 * it is also the only shape that could ship: `proto` stays a plain object and
 * property access keeps its semantics.
 *
 * Two details the obvious implementation gets wrong. Walking to a parent with
 * `cursor[segment] ??= {}` *reads* the parent, so every type with children
 * materializes at build time; the containers are therefore held in a map and
 * attached from there. And merging children with `Object.assign` reads them,
 * so the descriptors are copied instead.
 */
const rewriteNamespacePerType = (source: string, fromGetterObject: boolean): string => {
  const head = source.slice(0, anchor(source, "const buildNamespace = ", "proto-namespace.ts (per type)"));
  const codecFor = fromGetterObject
    ? "(codecs as Record<string, any>)[flatName]"
    : "(gen as Record<string, any>)[flatName]";
  return `${head}const defineLazy = (target: Record<string, any>, key: string, make: () => any): void => {
  // A consumer may have sealed or frozen the namespace before reading a type,
  // which makes this accessor non-configurable and the write-back impossible.
  // The eager namespace stays readable after a freeze and assignable after a
  // seal, so this one holds the value in the closure instead of throwing.
  let held: { value: any } | undefined;
  const keep = (value: any) => {
    try {
      Object.defineProperty(target, key, { value, writable: true, enumerable: true, configurable: true });
    } catch {
      held = { value };
    }
    return value;
  };
  Object.defineProperty(target, key, {
    configurable: true,
    enumerable: true,
    get() {
      return held ? held.value : keep(make());
    },
    set(this: any, value: any) {
      // An inherited write — \`Object.create(proto).Message = x\` — reaches this
      // setter with the overlay as the receiver. A data property would have
      // made an own property there and left the namespace alone; writing
      // through to \`target\` would change it for every holder.
      if (this !== target) {
        Object.defineProperty(this, key, { value, writable: true, enumerable: true, configurable: true });
        return;
      }
      // A frozen namespace has non-writable data properties, so assigning to
      // one throws in module code and cannot change what a read returns. An
      // accessor's setter still runs, so it has to refuse rather than accept.
      if (Object.isFrozen(target)) {
        throw new TypeError("Cannot assign to read only property '" + key + "' of object");
      }
      keep(value);
    },
  });
};

const buildNamespace = (): Record<string, any> => {
  const root: Record<string, any> = {};
  const codecNameSet = new Set<string>(codecNames);
  // One child container per flat name, reachable without reading the property
  // that will carry it.
  const containers = new Map<string, Record<string, any>>();
  const childrenOf = (flatName: string): Record<string, any> => {
    let container = containers.get(flatName);
    if (!container) containers.set(flatName, (container = {}));
    return container;
  };
  const containerFor = (path: string[]): Record<string, any> => {
    let cursor = root;
    let prefix = "";
    for (const segment of path) {
      prefix = prefix ? \`\${prefix}\${NAMESPACE_SEPARATOR}\${segment}\` : segment;
      const known = containers.has(prefix);
      const child = childrenOf(prefix);
      // A prefix that is itself a message gets its children through the getter
      // below; a pure namespace node attaches directly.
      if (!known && !codecNameSet.has(prefix)) cursor[segment] = child;
      cursor = child;
    }
    return cursor;
  };
  const place = (flatName: string): [Record<string, any>, string] => {
    const path = flatName.split(NAMESPACE_SEPARATOR);
    const leaf = path.pop()!;
    return [containerFor(path), leaf];
  };

  // Memoized per flat name so an alias and its target share one wrapper.
  const wrappers = new Map<string, () => any>();
  // Generated-source order, interleaving enums and codecs exactly as
  // Object.entries(gen) does: Object.keys(proto) is observable, and building
  // all the codecs first would reorder it.
  for (const flatName of exportOrder) {
    if (!codecNameSet.has(flatName)) {
      const value = (gen as Record<string, any>)[flatName];
      if (!isVisibleGeneratedExport(flatName, value) || !isEnumLike(value)) continue;
      const [enumCursor, enumLeaf] = place(flatName);
      enumCursor[enumLeaf] = Object.assign({}, childrenOf(flatName), value);
      continue;
    }
    const [cursor, leaf] = place(flatName);
    const children = childrenOf(flatName);
    let built: any;
    const make = () => {
      if (built === undefined) {
        built = wrapMessage(flatName.replaceAll(NAMESPACE_SEPARATOR, PROTO_TYPE_SEPARATOR), ${codecFor});
        Object.defineProperties(built, Object.getOwnPropertyDescriptors(children));
      }
      return built;
    };
    wrappers.set(flatName, make);
    defineLazy(cursor, leaf, make);
  }

  for (const [alias, target] of Object.entries(HISTORICAL_ALIASES)) {
    const make = wrappers.get(target);
    if (make && !Object.getOwnPropertyDescriptor(root, alias)) defineLazy(root, alias, make);
  }

  for (const parent of FORWARD_COMPATIBLE_PARENTS) {
    const descriptor = Object.getOwnPropertyDescriptor(root, parent);
    if (!descriptor?.get) continue;
    const inner = descriptor.get;
    // Memoized, not just deferred: on a frozen namespace the getter cannot
    // write itself back, and an unmemoized wrapper would hand out a new Proxy
    // per read, so proto.Message !== proto.Message — which the eager tree
    // never does.
    let wrapped: any;
    defineLazy(root, parent, () => (wrapped ??= wrapWithForwardCompatibleChildren(inner.call(root))));
  }

  return root;
};

export const proto: Record<string, any> = buildNamespace();
`;
};

/** The same tree, assembled on the first property read of `proto`. */
const rewriteNamespaceLazy = (source: string, fromGetterObject: boolean): string => {
  const head = source.slice(0, anchor(source, "const buildNamespace = ", "proto-namespace.ts (whole)"));
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
const IMPORT_LAZY = `${IMPORT_GEN}\nimport { codecs } from "./generated/whatsapp";\nimport { codecNames, exportOrder } from "./codec-names";`;

interface Arm {
  name: string;
  codecModule?: string;
  /** `whole`: one Proxy over the entire tree. `pertype`: a getter per type. */
  lazyNamespace?: "whole" | "pertype";
  /** The removed types are gone from the module, so `proto.ts` cannot name them. */
  realCut?: boolean;
  /** `textcut` cannot run the ping-pong traffic — its codecs throw. */
  touchable: boolean;
}

const writeCodecNames = (tree: string): void => {
  const list = (names: readonly string[]) => names.map((name) => `  ${JSON.stringify(name)},`).join("\n");
  writeFileSync(
    join(tree, "ts", "codec-names.ts"),
    `export const codecNames: readonly string[] = [\n${list(CODEC_NAMES)}\n];\n` +
      `export const exportOrder: readonly string[] = [\n${list(parsed.exportOrder)}\n];\n`,
  );
};

const PING_PONG = closure(parsed, PING_PONG_ROOTS);

/**
 * The enums a generator emitting only `keep` would still emit: the ones nested
 * under a kept message, and the ones a kept message's field references. Leaving
 * all 212 in would rebuild `proto.HistorySync` out of its enums alone, which is
 * a namespace node the cut is supposed to have removed.
 */
const reachableEnums = (keep: Set<string>): Set<string> => {
  const flat = (dotted: string) => dotted.replaceAll(".", "_");
  const declared = new Set<string>();
  const referenced = new Set<string>();
  for (const line of readFileSync(join(ROOT, "ts", "generated", "whatsapp-surface.txt"), "utf8").split("\n")) {
    if (!line || line.startsWith("#")) continue;
    const columns = line.split("\t");
    if (columns.length === 2 && columns[1] === "enum") {
      declared.add(flat(columns[0]!));
      continue;
    }
    const [owner, , , , type, ref] = columns;
    if (type !== "enum" || !ref || !keep.has(flat(owner!))) continue;
    referenced.add(flat(ref.split("=")[0]!));
  }
  const kept = new Set<string>();
  for (const name of declared) {
    if (referenced.has(name)) {
      kept.add(name);
      continue;
    }
    // Nested under a kept message: the longest prefix that names one.
    const segments = name.split("_");
    for (let cut = segments.length - 1; cut > 0; cut--) {
      if (keep.has(segments.slice(0, cut).join("_"))) {
        kept.add(name);
        break;
      }
    }
  }
  return kept;
};

/** The codec module a generator would emit for `keep`: nothing else in it. */
const realCut = (keep: Set<string>): string => {
  const enums = reachableEnums(keep);
  const trimmed: typeof parsed = {
    ...parsed,
    blocks: parsed.blocks.filter((block) => block.kind !== "enum" || enums.has(block.name!)),
  };
  return emit(trimmed, keep, "eager", false);
};
const arms: Arm[] = [
  { name: "stock", touchable: true },
  { name: "textcut", codecModule: stubCodecs(new Set()), touchable: false },
  { name: "cut", codecModule: stubCodecs(PING_PONG), touchable: true },
  { name: "cut-real", codecModule: realCut(PING_PONG), realCut: true, touchable: true },
  { name: "lazycodecs", codecModule: LAZY_CODECS, touchable: true },
  { name: "lazyns", lazyNamespace: "whole", touchable: true },
  { name: "lazyboth", codecModule: LAZY_CODECS, lazyNamespace: "whole", touchable: true },
  { name: "lazyns-pertype", lazyNamespace: "pertype", touchable: true },
  { name: "lazyboth-pertype", codecModule: LAZY_CODECS, lazyNamespace: "pertype", touchable: true },
  { name: "textcut-lazyns", codecModule: stubCodecs(new Set()), lazyNamespace: "whole", touchable: false },
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
  if (arm.realCut) {
    // The 31-entry registry names types this arm no longer has. Resolving
    // through the star import is what a generated-for-your-types codec would
    // do, and the names that are gone throw — which is the API change.
    writeFileSync(join(tree, "ts", "proto.ts"), rewriteProtoTsForCut(readFileSync(join(tree, "ts", "proto.ts"), "utf8")));
  }
  if (lazyCodecs) {
    writeCodecNames(tree);
    writeFileSync(join(tree, "ts", "proto.ts"), rewriteProtoTs(readFileSync(join(tree, "ts", "proto.ts"), "utf8")));
  }
  const namespacePath = join(tree, "ts", "proto-namespace.ts");
  let namespaceSource = readFileSync(namespacePath, "utf8");
  if (lazyCodecs) {
    namespaceSource = namespaceSource.replace(IMPORT_GEN, IMPORT_LAZY);
  } else if (arm.lazyNamespace === "pertype") {
    // The per-type tree is keyed by name, so it needs the list even when the
    // codec module itself stayed eager.
    writeCodecNames(tree);
    namespaceSource = namespaceSource.replace(
      IMPORT_GEN,
      `${IMPORT_GEN}\nimport { codecNames, exportOrder } from "./codec-names";`,
    );
  }
  if (arm.lazyNamespace === "whole") {
    namespaceSource = rewriteNamespaceLazy(namespaceSource, lazyCodecs);
  } else if (arm.lazyNamespace === "pertype") {
    namespaceSource = rewriteNamespacePerType(namespaceSource, lazyCodecs);
  } else if (lazyCodecs) {
    // Eager namespace over a getter object: it reads every getter while
    // assembling, which is the whole point of the `lazycodecs` arm.
    anchor(namespaceSource, "for (const [flatName, value] of Object.entries(gen)) {", "proto-namespace.ts (eager bag)");
    namespaceSource = namespaceSource.replace(
      "for (const [flatName, value] of Object.entries(gen)) {",
      'for (const [flatName, value] of [...codecNames.map((n) => [n, (codecs as any)[n]] as const), ...Object.entries(gen).filter(([n]) => n !== "codecs")]) {',
    );
  }
  writeFileSync(namespacePath, namespaceSource);

  // The arm's own module has to carry what the arm's name claims, or the
  // number it reports is for a variant nobody described.
  if (arm.codecModule) {
    const bodies = (arm.codecModule.match(/^function createBase/gm) ?? []).length;
    const factories = (arm.codecModule.match(/^function _mk_/gm) ?? []).length;
    const expected = arm.codecModule === LAZY_CODECS ? CODEC_NAMES.length : undefined;
    if (arm.codecModule === LAZY_CODECS && factories !== expected) {
      throw new Error(`${arm.name}: ${factories} lazy factories, expected ${expected}`);
    }
    if (arm.realCut && bodies !== PING_PONG.size) {
      throw new Error(`${arm.name}: ${bodies} codec bodies, expected ${PING_PONG.size}`);
    }
  }

  const built = Bun.spawnSync({
    cmd: ["bun", "build", join(tree, "ts", "index.ts"), "--minify", "--target", "node", "--outfile", join(WORK, `${arm.name}.js`)],
    stdout: "pipe",
    stderr: "inherit",
  });
  if (built.exitCode !== 0) throw new Error(`build failed: ${arm.name}`);
}

// Bytes only, never measured for memory: the codec share of the bundle is the
// published size minus this one, and `textcut`'s four-method stubs cost enough
// bytes of their own to understate it.
const emptyBundle = (): number => {
  const tree = join(WORK, "bytes-only");
  rmSync(tree, { recursive: true, force: true });
  cpSync(join(ROOT, "ts"), join(tree, "ts"), { recursive: true });
  symlinkSync(join(ROOT, "pkg"), join(tree, "pkg"));
  symlinkSync(join(ROOT, "node_modules"), join(tree, "node_modules"));
  writeFileSync(
    join(tree, "ts", "generated", "whatsapp.ts"),
    parsed.blocks
      .map((block) =>
        block.kind === "codec"
          ? `export const ${block.name}: any = {};`
          : block.kind === "createBase"
            ? ""
            : block.text,
      )
      .join("\n"),
  );
  const built = Bun.spawnSync({
    cmd: ["bun", "build", join(tree, "ts", "index.ts"), "--minify", "--target", "node", "--outfile", join(WORK, "bytes-only.js")],
    stdout: "pipe",
    stderr: "inherit",
  });
  if (built.exitCode !== 0) throw new Error("build failed: bytes-only");
  return statSync(join(WORK, "bytes-only.js")).size;
};
const emptyBytes = emptyBundle();
const stockBytes = statSync(join(WORK, "stock.js")).size;

const runs: { arm: Arm; touch: boolean }[] = [];
for (const arm of arms) {
  runs.push({ arm, touch: false });
  if (arm.touchable) runs.push({ arm, touch: true });
}
const label = (run: { arm: Arm; touch: boolean }) => `${run.arm.name}${run.touch ? " +touch" : ""}`;

const priv = new Map<string, number[]>(runs.map((run) => [label(run), []]));
const heaps = new Map<string, number[]>(runs.map((run) => [label(run), []]));
// Node 22 charges the codec text to the JS heap and node 26 to external
// memory, so neither column alone says what an arm retains. Summed per sample,
// not per median: the two move in opposite directions on node 26, and a median
// of one plus a median of the other is a number no process ever had.
const externals = new Map<string, number[]>(runs.map((run) => [label(run), []]));
const retained = new Map<string, number[]>(runs.map((run) => [label(run), []]));

// A fresh permutation each repetition, so no arm keeps a position in the sweep
// and drift within one cannot be charged to arm identity.
for (let rep = 0; rep < REPS; rep++) {
  for (const run of shuffled(runs, rep, REPS)) {
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
    externals.get(label(run))!.push(row.external);
    retained.get(label(run))!.push(row.heapUsed + row.external);
  }
}

const median = (xs: number[]) => {
  const sorted = [...xs].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid]! : (sorted[mid - 1]! + sorted[mid]!) / 2;
};

const version = Bun.spawnSync({ cmd: [NODE, "-v"], stdout: "pipe" }).stdout.toString().trim();
// bun built every arm here, and its optimizer decides the bytes being measured,
// so the header names it alongside the node that ran them.
console.log(`reps=${REPS} node=${version} bun=${Bun.version} flags=${NODE_FLAGS.join(" ") || "(none)"}`);
console.log(
  `bundle=${stockBytes} B, codec bodies and names replaced by {}=${emptyBytes} B, codec share=${stockBytes - emptyBytes} B (${(((stockBytes - emptyBytes) / stockBytes) * 100).toFixed(1)} %)`,
);
console.log(
  ["arm", "bundleKiB", "PrivDirty med", "min", "max", "vs base", "retained med", "vs base", "heapUsed med", "external med"].join("\t"),
);
// A `+touch` arm is measured against `stock +touch`: the traffic itself costs
// 0.12–0.28 MiB, and charging that to the arm would flatter every one of them.
const baseline = (run: { touch: boolean }) => (run.touch ? "stock +touch" : "stock");
const stockPriv = (run: { touch: boolean }) => median(priv.get(baseline(run))!);
const stockRetained = (run: { touch: boolean }) => median(retained.get(baseline(run))!);
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
      (median(xs) - stockPriv(run)).toFixed(0),
      median(retained.get(key)!).toFixed(0),
      (median(retained.get(key)!) - stockRetained(run)).toFixed(0),
      median(heaps.get(key)!).toFixed(0),
      median(externals.get(key)!).toFixed(0),
    ].join("\t"),
  );
}
