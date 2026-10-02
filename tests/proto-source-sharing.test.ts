import { expect, test } from "bun:test";
import { expandSharedUnknownFields, shareProtoPrivateWork } from "../scripts/proto-source-sharing";

const SOURCE = `
import { BinaryReader } from "${new URL("../ts/proto-reader.ts", import.meta.url).href}";
function createBaseLeaf() { return {}; }
export const Leaf = {
  create(base?: any): any { return Leaf.fromPartial(base ?? {}); },
  fromPartial(object: any): any {
    const message = createBaseLeaf();
    message.a = object.a ?? undefined;
    message.b = object.b ?? undefined;
    return message;
  },
  decode(input: BinaryReader): any {
    const reader = input;
    const message = createBaseLeaf();
    while (reader.pos < reader.len) {
      const tag = reader.uint32();
      switch (tag >>> 3) {
        case 1:
          if (tag !== 8) { break; }
          message.a = reader.uint32();
          continue;
      }
      if (tag >>> 3 === 0 || (tag & 7) === 4) {
        throw new RangeError(\`illegal protobuf tag \${tag} at offset \${reader.pos}\`);
      }
      reader.skip(tag & 7);
    }
    return message;
  },
};
function createBaseMixed() { return {}; }
export const Mixed = {
  create(base?: any): any { return Mixed.fromPartial(base ?? {}); },
  fromPartial(object: any): any {
    const message = createBaseMixed();
    message.a = object.a ?? undefined;
    message.b = object.b ?? undefined;
    message.long = object.long ?? undefined;
    message.bytes = object.bytes ?? undefined;
    message.child = object.child !== undefined && object.child !== null ? Leaf.fromPartial(object.child) : undefined;
    message.names = object.names?.map(e => e) || undefined;
    message.map = Object.entries(object.map ?? {}).reduce((acc, [key, value]) => {
      if (value !== undefined) { acc[key] = Leaf.fromPartial(value); }
      return acc;
    }, {});
    message.c = object.c ?? undefined;
    message.d = object.d ?? undefined;
    return message;
  },
};
`;

async function modules() {
  const shared = shareProtoPrivateWork(SOURCE);
  const transpiler = new Bun.Transpiler({ loader: "ts", target: "bun" });
  const load = (source: string) => import(`data:text/javascript;base64,${Buffer.from(transpiler.transformSync(source)).toString("base64")}`);
  const original = await load(SOURCE);
  const candidate = await load(shared.text);
  return { original, candidate, shared };
}

test("private sharing retains exports, public methods, construction, ordered reads and representation", async () => {
  const { original, candidate, shared } = await modules();
  expect(shared).toMatchObject({ unknownEpilogues: 1, scalarRuns: 3, scalarFields: 8, createMethods: 2 });
  expect(Object.keys(candidate)).toEqual(Object.keys(original));
  for (const name of ["Leaf", "Mixed"]) {
    for (const method of ["create", "fromPartial"]) {
      expect(candidate[name][method].name).toBe(original[name][method].name);
      expect(candidate[name][method].length).toBe(original[name][method].length);
      expect(() => new candidate[name][method]()).toThrow(TypeError);
    }
  }
  const long = { low: 1, high: 2, unsigned: true };
  const bytes = new Uint8Array([1, 2]);
  const source = { a: 0, b: false, long, bytes, child: { a: 3 }, names: ["x"], map: { k: { b: 4 } }, c: "", d: null, ignored: 1 };
  const results: any[] = [];
  for (const module of [original, candidate]) {
    const trace: string[] = [];
    const object = new Proxy(source, { get(target, key, receiver) {
      trace.push(String(key));
      if (key === "b") expect(module.Leaf.fromPartial({ a: 9 })).toEqual({ a: 9, b: undefined });
      return Reflect.get(target, key, receiver);
    } });
    const result = module.Mixed.fromPartial(object);
    expect(Object.getPrototypeOf(result)).toBe(Object.prototype);
    expect(result.long).toBe(long);
    expect(result.bytes).toBe(bytes);
    expect(result.names).not.toBe(source.names);
    expect(result.child).not.toBe(source.child);
    expect(result.map).not.toBe(source.map);
    expect(result.map.k).not.toBe(source.map.k);
    expect(Object.hasOwn(result, "d")).toBe(true);
    expect(Object.hasOwn(result, "ignored")).toBe(false);
    expect(module.Mixed.fromPartial({})).not.toBe(module.Mixed.fromPartial({}));
    results.push({ result, keys: Object.keys(result), descriptors: Object.getOwnPropertyDescriptors(result), trace });
  }
  expect(results[1]).toEqual(results[0]);
});

test("first getter/setter failures preserve identity, partial effects and no later reads", async () => {
  const { original, candidate } = await modules();
  const sentinel = new Error("first field failure");
  const outcomes: any[] = [];
  for (const module of [original, candidate]) {
    const trace: string[] = [];
    let partial: any;
    Object.defineProperty(Object.prototype, "a", { configurable: true, set(value) {
      trace.push(`set:a:${value}`);
      partial = this;
      Object.defineProperty(this, "a", { value, writable: true, configurable: true, enumerable: true });
    } });
    try {
      const input = new Proxy({}, { get(_, key) {
        trace.push(`get:${String(key)}`);
        if (key === "b") throw sentinel;
        return 8;
      } });
      let failure: unknown;
      try { module.Mixed.fromPartial(input); } catch (error) { failure = error; }
      expect(failure).toBe(sentinel);
    } finally {
      delete (Object.prototype as any).a;
    }
    outcomes.push({ trace, partial });
  }
  expect(outcomes[1]).toEqual(outcomes[0]);
  expect(outcomes[0]).toEqual({ trace: ["get:a", "set:a:8", "get:b"], partial: { a: 8 } });
  for (const module of [original, candidate]) {
    const reads: string[] = [];
    Object.defineProperty(Object.prototype, "a", { configurable: true, set() { throw sentinel; } });
    try {
      const input = new Proxy({}, { get(_, key) { reads.push(String(key)); return 1; } });
      let failure: unknown;
      try { module.Leaf.fromPartial(input); } catch (error) { failure = error; }
      expect(failure).toBe(sentinel);
    } finally { delete (Object.prototype as any).a; }
    expect(reads).toEqual(["a"]);
  }
});

test("create keeps callee getter, receiver, fresh nullish base and first failure", async () => {
  const { original, candidate } = await modules();
  for (const module of [original, candidate]) {
    const codec = module.Leaf;
    const descriptor = Object.getOwnPropertyDescriptor(codec, "fromPartial")!;
    const bases: any[] = [];
    const calls: string[] = [];
    Object.defineProperty(codec, "fromPartial", { configurable: true, get() {
      calls.push("get");
      return function(base: any) { expect(this).toBe(codec); calls.push("call"); bases.push(base); return base; };
    } });
    try {
      const explicit = { a: 1 };
      expect(codec.create.call({}, explicit)).toBe(explicit);
      codec.create(undefined);
      codec.create(null);
      expect(bases[1]).toEqual({});
      expect(bases[2]).toEqual({});
      expect(bases[1]).not.toBe(bases[2]);
      expect(calls).toEqual(["get", "call", "get", "call", "get", "call"]);
      const sentinel = new Error("callee getter");
      Object.defineProperty(codec, "fromPartial", { configurable: true, get() { throw sentinel; } });
      let failure: unknown;
      try { codec.create(undefined); } catch (error) { failure = error; }
      expect(failure).toBe(sentinel);
    } finally { Object.defineProperty(codec, "fromPartial", descriptor); }
  }
});

test("shared framing keeps wire skips, next field, reader receiver and invalid-tag error", async () => {
  const { original, candidate, shared } = await modules();
  const { BinaryReader } = await import("../ts/proto-reader");
  const outcomes: any[] = [];
  for (const module of [original, candidate]) {
    const reader = new BinaryReader(new Uint8Array([10, 2, 255, 255, 8, 9]));
    const skip = reader.skip;
    const trace: number[] = [];
    reader.skip = function(wire) { expect(this).toBe(reader); trace.push(wire); return skip.call(this, wire); };
    const value = module.Leaf.decode(reader);
    const errors = [];
    for (const tag of [0, 7, 12]) {
      const bad = new BinaryReader(new Uint8Array([tag]));
      try { module.Leaf.decode(bad); throw new Error("accepted invalid framing"); }
      catch (error: any) { errors.push([error.name, error.message, bad.pos]); }
    }
    outcomes.push({ value, trace, pos: reader.pos, errors });
  }
  expect(outcomes[1]).toEqual(outcomes[0]);
  expect(outcomes[0].value).toEqual({ a: 9 });
  expect(outcomes[0].trace).toEqual([2]);
  expect(outcomes[0].errors[0]).toEqual(["RangeError", "illegal protobuf tag 0 at offset 1", 1]);
  expect(expandSharedUnknownFields(shared.text)).not.toContain("skipUnknownProtoField(reader, tag);");
  expect(() => expandSharedUnknownFields(shared.text.replace("reader.skip(tag & 7);", "reader.skip(0);"))).toThrow("framing contract");
  expect(() => expandSharedUnknownFields(shared.text.replace("skipUnknownProtoField(reader, tag);", "return message;"))).toThrow("framing contract");
});

test("sharing refuses changed generator shapes instead of silently omitting work", () => {
  expect(() => shareProtoPrivateWork(SOURCE.replace("return Leaf.fromPartial(base ?? {});", "return Leaf.fromPartial(base);"))).toThrow("create method changed");
  expect(() => shareProtoPrivateWork(SOURCE.replace("reader.skip(tag & 7);", "reader.skip(0);"))).toThrow("framing shape drift");
  expect(() => shareProtoPrivateWork(SOURCE.replace("const message = createBaseMixed();", "const message = {};"))).toThrow("base construction changed");
});
