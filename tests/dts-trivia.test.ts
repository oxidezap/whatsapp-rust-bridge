import { expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import ts from "typescript";
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
  const scanner = ts.createScanner(ts.ScriptTarget.Latest, false, ts.LanguageVariant.Standard, text);
  const comments: string[] = [];
  for (let kind = scanner.scan(); kind !== ts.SyntaxKind.EndOfFileToken; kind = scanner.scan()) {
    if (kind === ts.SyntaxKind.SingleLineCommentTrivia || kind === ts.SyntaxKind.MultiLineCommentTrivia) {
      comments.push(scanner.getTokenText());
    }
  }
  return comments;
}

function fingerprint(text: string, aliases: ReadonlyMap<string, string>) {
  const file = ts.createSourceFile("surface.d.ts", text, ts.ScriptTarget.Latest, true);
  const bodies = new Map([...aliases].map(([name, body]) => [name,
    (ts.createSourceFile("alias.d.ts", `type X=${body};`, ts.ScriptTarget.Latest, true)
      .statements[0] as ts.TypeAliasDeclaration).type]));
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

function program(declaration: string, consumer = "", exactOptionalPropertyTypes = false) {
  const dir = resolve("tests/fixtures/.dts-compact-virtual");
  const declarationPath = `${dir}/proto-types.d.ts`;
  const consumerPath = `${dir}/consumer.ts`;
  const virtual = new Map([[declarationPath, declaration], [consumerPath, consumer]]);
  const options: ts.CompilerOptions = {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.NodeNext,
    moduleResolution: ts.ModuleResolutionKind.NodeNext,
    strict: true,
    skipLibCheck: false,
    exactOptionalPropertyTypes,
    noEmit: true,
    types: [],
  };
  const host = ts.createCompilerHost(options);
  const read = host.readFile;
  const exists = host.fileExists;
  const directoryExists = host.directoryExists!;
  host.readFile = name => virtual.get(name) ?? read(name);
  host.fileExists = name => virtual.has(name) || exists(name);
  host.directoryExists = name => name === dir || directoryExists(name);
  host.getSourceFile = (name, languageVersion) => {
    const text = host.readFile(name);
    return text === undefined ? undefined : ts.createSourceFile(name, text, languageVersion, true);
  };
  const result = ts.createProgram([declarationPath, consumerPath], options, host);
  return { result, declarationPath, consumerPath };
}

function exported(declaration: string) {
  const { result, declarationPath } = program(declaration);
  const checker = result.getTypeChecker();
  const source = result.getSourceFile(declarationPath)!;
  return checker.getExportsOfModule(checker.getSymbolAtLocation(source)!).map(symbol => {
    const target = symbol.flags & ts.SymbolFlags.Alias ? checker.getAliasedSymbol(symbol) : symbol;
    return [symbol.name, target.flags];
  }).sort((a, b) => String(a[0]).localeCompare(String(b[0])));
}

function diagnostics(declaration: string, consumer: string, exact: boolean) {
  const { result, declarationPath, consumerPath } = program(declaration, consumer, exact);
  // Check both authored roots, plus global/options errors, with skipLibCheck false.
  // Rechecking TypeScript's immutable stdlib per variant adds no compactor coverage.
  const roots = [declarationPath, consumerPath].map(path => result.getSourceFile(path)!);
  const errors = [
    ...result.getOptionsDiagnostics(),
    ...result.getGlobalDiagnostics(),
    ...roots.flatMap(source => [
      ...result.getSyntacticDiagnostics(source),
      ...result.getSemanticDiagnostics(source),
    ]),
  ];
  return errors.map(diagnostic => ({
    code: diagnostic.code,
    message: ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"),
  }));
}

test("interpolated template text survives trivia compaction, including nested tails", () => {
  const original = 'export type T = `a${string} b`;\n' +
    'export type Nested = ` a${`inside ${string} x`} \\t${number}\\n tail `;\n';
  const compact = compactDeclarationTrivia(original);
  expect(fingerprint(compact, new Map())).toEqual(fingerprint(original, new Map()));
  expect(compactDeclarationTrivia(compact)).toBe(compact);
});

test("trivia retains comment bytes, literals and newline-sensitive signatures", () => {
  const original = `// leading directive\n/** doc */\nexport interface X {\n  value: "a b";\n  f(): void\n  g(): void\n}\n`;
  const compact = compactDeclarationTrivia(original);
  expect(commentTokens(compact)).toEqual(commentTokens(original));
  expect(compact).toContain('"a b"');
  expect(fingerprint(compact, new Map())).toEqual(fingerprint(original, new Map()));
});

test("actual published declarations retain own AST/docs, specifiers and idempotence", () => {
  for (const name of ["proto-types.d.ts", "whatsapp_rust_bridge.d.ts"]) {
    const path = `dist/${name}`;
    expect(existsSync(path), `${path} is absent — run \`bun run build\` first`).toBe(true);
    const original = readFileSync(path, "utf8");
    const candidate = compactDeclarationTrivia(original);
    expect(commentTokens(candidate)).toEqual(commentTokens(original));
    expect(fingerprint(candidate, new Map())).toEqual(fingerprint(original, new Map()));
    expect(compactDeclarationTrivia(candidate)).toBe(candidate);
  }
});

test("ambient Long/protobuf/proto exports retain kinds without adding bindings", () => {
  const candidate = { text: compactDeclarationTrivia(FIXTURE) };
  expect(exported(candidate.text)).toEqual(exported(FIXTURE));
  expect(exported(candidate.text).map(([name]) => name)).toEqual(["$protobuf", "Long", "proto"]);
});

test("strict comparisons diagnose both broken declarations and invalid consumer assignments", () => {
  for (const exact of [false, true]) {
    expect(diagnostics(FIXTURE + "\nexport type Broken = MissingType;\n", "", exact)
      .some(error => error.code === 2304)).toBe(true);
    expect(diagnostics(FIXTURE,
      'import { proto } from "./proto-types.js"; new proto.Account().lid = 7;', exact)
      .some(error => error.code === 2322)).toBe(true);
  }
});

test("ordinary own-property and augmentation consumers remain strict in both exactOptional modes", () => {
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
    expect(diagnostics(FIXTURE, consumer, exact)).toEqual([]);
    expect(diagnostics(candidate.text, consumer, exact)).toEqual([]);
  }
});

test("adding alias-like namespace names cannot shadow previously public field types", () => {
  const candidate = { text: compactDeclarationTrivia(FIXTURE) };
  const alias = "_D0";
  const consumer = `import { proto } from "./proto-types.js";
declare module "./proto-types.js" { namespace proto { type ${alias} = number; } }
const input: proto.IAccount = { lid: "lid", username: "name", country: "country" };
const instance = new proto.Account(input);
const value: string | null | undefined = instance.lid;
`;
  const outcomes = [false, true].map(exact => ({ exact,
    original: diagnostics(FIXTURE, consumer, exact),
    candidate: diagnostics(candidate.text, consumer, exact),
  }));
  for (const outcome of outcomes) {
    expect(outcome.original).toEqual([]);
    expect(outcome.candidate).toEqual(outcome.original);
  }
});
