import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";

const root = join(import.meta.dir, "..");
const declarations = ts.createSourceFile("proto-types.d.ts", readFileSync(join(root, "ts/proto-types.d.ts"), "utf8"), ts.ScriptTarget.Latest, true);
const classes: string[][] = [];
function visit(node: ts.Node, path: string[]) {
  if (ts.isModuleDeclaration(node) && ts.isIdentifier(node.name)) {
    if (node.body) visit(node.body, [...path, node.name.text]);
  } else if (ts.isClassDeclaration(node) && node.name && path[0] === "proto") {
    classes.push([...path, node.name.text]);
  } else {
    ts.forEachChild(node, child => visit(child, path));
  }
}
visit(declarations, []);

const fixture = `
import { proto } from "../dist/proto-types.js";
import type { BinaryReader } from "../dist/index.js";
type Eq<A,B> = (<T>()=>T extends A?1:2) extends (<T>()=>T extends B?1:2)?true:false;
type Assert<T extends true> = T;
interface Writer { finish(): Uint8Array }
type Encode<I> = {encode(m:I,w?:Writer):Writer}["encode"];
type Decode<C> = {decode(r:BinaryReader|Uint8Array,l?:number):C}["decode"];
${classes.map((path, index) => {
  const name = path.join(".");
  const input = [...path.slice(0, -1), `I${path.at(-1)}`].join(".");
  return `
type Keys${index} = Assert<Eq<keyof ${name}, keyof ${input}|"toJSON">>;
type Fields${index} = Assert<Eq<Pick<${name},keyof ${input}>,${input}>>;
type Constructor${index} = Assert<Eq<ConstructorParameters<typeof ${name}>,[p?:${input}|undefined]>>;
type Encoder${index} = Assert<Eq<typeof ${name}.encode,Encode<${input}>>>;
type Decoder${index} = Assert<Eq<typeof ${name}.decode,Decode<${name}>>>;
const create${index}: (p?:${input})=>${name} = ${name}.create;
const from${index}: (d:{[k:string]:any})=>${name} = ${name}.fromObject;
const to${index}: (m:${name},o?:{[k:string]:any})=>{[k:string]:any} = ${name}.toObject;
`;
}).join("\n")}
// Method parameters were bivariant before compaction; a plain function alias would fail.
declare const narrowed: (m:proto.IMessage & {requiredMarker:true},w?:Writer)=>Writer;
const methodVariance: typeof proto.Message.encode = narrowed;
declare const instance: proto.Message;
const writableConversation: typeof instance.conversation = "text";
instance.conversation = writableConversation;
`;

for (const exactOptionalPropertyTypes of [false, true]) {
  test(`all protobuf factories preserve fields, constructors and method variance, exactOptional=${exactOptionalPropertyTypes}`, () => {
    expect(classes.length).toBeGreaterThan(700);
    const file = join(root, "tests/proto-factory-contract-consumer.ts");
    const options: ts.CompilerOptions = {
      strict: true, exactOptionalPropertyTypes, skipLibCheck: false, noEmit: true,
      target: ts.ScriptTarget.ESNext, module: ts.ModuleKind.ESNext,
      moduleResolution: ts.ModuleResolutionKind.Bundler, types: [],
    };
    const host = ts.createCompilerHost(options);
    const original = host.getSourceFile.bind(host);
    host.getSourceFile = (name, language, onError, fresh) => name === file
      ? ts.createSourceFile(file, fixture, language, true)
      : original(name, language, onError, fresh);
    const program = ts.createProgram([file], options, host);
    const diagnostics = ts.getPreEmitDiagnostics(program);
    expect(diagnostics.map(item => {
      const line = item.file && item.start !== undefined ? item.file.getLineAndCharacterOfPosition(item.start).line : undefined;
      return `${line === undefined ? "" : fixture.split("\n")[line]}: ${ts.flattenDiagnosticMessageText(item.messageText, "\n")}`;
    })).toEqual([]);
  }, 8000);
}
