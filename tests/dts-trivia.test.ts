import { expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import * as ts from "../scripts/typescript-api";
import { compactDeclarationTrivia } from "../scripts/dts-trivia";

const FIXTURE = `/** Long documentation stays here. */
type Long = number | { low: number; high: number; unsigned: boolean };
declare namespace $protobuf { interface Writer { finish(): number; } }
export namespace proto {
  /** Input documentation. */
  interface IAccount {
    lid?: (string|null);
    username?: (string|null);
    country?: (string|null);
  }
  /** Concrete instance documentation. */
  class Account implements IAccount {
    constructor(p?: IAccount);
    public lid?: (string|null);
    public username?: (string|null);
    public country?: (string|null);
    public static create(p?: IAccount): Account;
    public toJSON(): { [k: string]: any };
  }
}
`;

function commentTokens(text: string) {
  const scanner = ts.createScanner(false, ts.LanguageVariant.Standard, text);
  const comments: string[] = [];
  for (let kind = scanner.scan(); kind !== ts.SyntaxKind.EndOfFile; kind = scanner.scan()) {
    if (kind === ts.SyntaxKind.SingleLineCommentTrivia || kind === ts.SyntaxKind.MultiLineCommentTrivia) {
      comments.push(scanner.getTokenText());
    }
  }
  return comments;
}

async function fingerprint(text: string, aliases: ReadonlyMap<string, string>) {
  const file = await ts.createSourceFile("surface.d.ts", text);
  const bodies = new Map<string, ts.TypeNode>();
  for (const [name, body] of aliases) {
    const alias = await ts.createSourceFile("alias.d.ts", `type X=${body};`);
    bodies.set(name, (alias.statements[0] as ts.TypeAliasDeclaration).type);
  }
  function visit(node: ts.Node): unknown {
    if (ts.isTypeReferenceNode(node) && ts.isIdentifier(node.typeName) && bodies.has(node.typeName.text)) {
      return visit(bodies.get(node.typeName.text)!);
    }
    const children: unknown[] = [];
    ts.forEachChild(node, child => {
      if (child.parent === file && ts.isTypeAliasDeclaration(child) && aliases.has(child.name.text)) return;
      if (child.parent === file && ts.isExportDeclaration(child) && !child.moduleSpecifier &&
        child.exportClause && ts.isNamedExports(child.exportClause) && !child.exportClause.elements.length) return;
      // These two declarations were already exported by ambient export context.
      if (child.kind === ts.SyntaxKind.ExportKeyword && node.parent === file &&
        ((ts.isTypeAliasDeclaration(node) && node.name.text === "Long") ||
          (ts.isModuleDeclaration(node) && node.name.getText(file) === "$protobuf"))) return;
      children.push(visit(child));
    });
    const docs = (node as ts.Node & { jsDoc?: readonly ts.JSDoc[] }).jsDoc?.map(doc => doc.getText());
    return [node.kind, ts.isIdentifier(node) || ts.isLiteralExpression(node) || ts.isTemplateLiteralToken(node)
      ? node.text : undefined, docs, children];
  }
  return visit(file);
}

function projectFiles(declaration: string, consumer = "") {
  const dir = resolve("tests/fixtures/.dts-compact-virtual");
  return { [`${dir}/proto-types.d.ts`]: declaration, [`${dir}/consumer.ts`]: consumer };
}
const OPTIONS = { target: "es2022", module: "nodenext", moduleResolution: "nodenext",
  strict: true, skipLibCheck: false, noEmit: true, types: [] };

async function exported(declaration: string) {
  const files = projectFiles(declaration);
  return ts.withProject(files, OPTIONS, async ({ program, checker }) => {
    const source = (await program.getSourceFile(Object.keys(files)[0]))!;
    const symbol = (await checker.getSymbolAtLocation(source))!;
    const exports = await checker.getExportsOfModule(symbol);
    const entries = await Promise.all(exports.map(async symbol => {
      const target = symbol.flags & ts.SymbolFlags.Alias ? await checker.getAliasedSymbol(symbol) : symbol;
      return [symbol.name, target.flags];
    }));
    return entries.sort((a, b) => String(a[0]).localeCompare(String(b[0])));
  });
}

async function diagnostics(declaration: string, consumer: string, exact: boolean) {
  const errors = await ts.diagnostics(projectFiles(declaration, consumer),
    { ...OPTIONS, exactOptionalPropertyTypes: exact }, true);
  return errors.map(error => ({ code: error.code, message: error.text }));
}

test("interpolated template text survives trivia compaction, including nested tails", async () => {
  const original = 'export type T = `a${string} b`;\n' +
    'export type Nested = ` a${`inside ${string} x`} \\t${number}\\n tail `;\n';
  const compact = compactDeclarationTrivia(original);
  expect(await fingerprint(compact, new Map())).toEqual(await fingerprint(original, new Map()));
  expect(compactDeclarationTrivia(compact)).toBe(compact);
});

test("trivia retains comment bytes, literals and newline-sensitive signatures", async () => {
  const original = `// leading directive\n/** doc */\nexport interface X {\n  value: "a b";\n  f(): void\n  g(): void\n}\n`;
  const compact = compactDeclarationTrivia(original);
  expect(commentTokens(compact)).toEqual(commentTokens(original));
  expect(compact).toContain('"a b"');
  expect(await fingerprint(compact, new Map())).toEqual(await fingerprint(original, new Map()));
});

test("actual published declarations retain own AST/docs, specifiers and idempotence", async () => {
  for (const name of ["proto-types.d.ts", "whatsapp_rust_bridge.d.ts"]) {
    const path = `dist/${name}`;
    expect(existsSync(path), `${path} is absent — run \`bun run build\` first`).toBe(true);
    const original = readFileSync(path, "utf8");
    const candidate = compactDeclarationTrivia(original);
    expect(commentTokens(candidate)).toEqual(commentTokens(original));
    expect(await fingerprint(candidate, new Map())).toEqual(await fingerprint(original, new Map()));
    expect(compactDeclarationTrivia(candidate)).toBe(candidate);
  }
});

test("ambient Long/protobuf/proto exports retain kinds without adding bindings", async () => {
  const candidate = { text: compactDeclarationTrivia(FIXTURE) };
  expect(await exported(candidate.text)).toEqual(await exported(FIXTURE));
  expect((await exported(candidate.text)).map(([name]) => name)).toEqual(["$protobuf", "Long", "proto"]);
});

test("strict comparisons diagnose both broken declarations and invalid consumer assignments", async () => {
  for (const exact of [false, true]) {
    expect((await diagnostics(FIXTURE + "\nexport type Broken = MissingType;\n", "", exact))
      .some(error => error.code === 2304)).toBe(true);
    expect((await diagnostics(FIXTURE,
      'import { proto } from "./proto-types.js"; new proto.Account().lid = 7;', exact))
      .some(error => error.code === 2322)).toBe(true);
  }
});

test("ordinary own-property and augmentation consumers remain strict in both exactOptional modes", async () => {
  const candidate = { text: compactDeclarationTrivia(FIXTURE) };
  const consumer = `import { proto, Long } from "./proto-types.js";
declare module "./proto-types.js" { namespace proto {
  interface IAccount { extra?: boolean; }
  interface Account { ownExtra?: number; }
} }
const input: proto.IAccount = { lid: "lid", extra: true };
const instance = new proto.Account(input);
const value: string | null | undefined = instance.lid;
instance.ownExtra = 1;
// @ts-expect-error input augmentation does not derive a new instance field
instance.extra;
const long: Long = 1;
`;
  for (const exact of [false, true]) {
    expect((await diagnostics(FIXTURE, consumer, exact))).toEqual([]);
    expect((await diagnostics(candidate.text, consumer, exact))).toEqual([]);
  }
});

test("adding alias-like namespace names cannot shadow previously public field types", async () => {
  const candidate = { text: compactDeclarationTrivia(FIXTURE) };
  const alias = "_D0";
  const consumer = `import { proto } from "./proto-types.js";
declare module "./proto-types.js" { namespace proto { type ${alias} = number; } }
const input: proto.IAccount = { lid: "lid", username: "name", country: "country" };
const instance = new proto.Account(input);
const value: string | null | undefined = instance.lid;
`;
  const outcomes = await Promise.all([false, true].map(async exact => ({ exact,
    original: await diagnostics(FIXTURE, consumer, exact),
    candidate: await diagnostics(candidate.text, consumer, exact),
  })));
  for (const outcome of outcomes) {
    expect(outcome.original).toEqual([]);
    expect(outcome.candidate).toEqual(outcome.original);
  }
});
