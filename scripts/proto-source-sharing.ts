import ts from "typescript";

const UNKNOWN_BODY = [
  "if (tag >>> 3 === 0 || (tag & 7) === 4) {",
  '  throw new RangeError(`illegal protobuf tag ${tag} at offset ${reader.pos}`);',
  "}",
  "reader.skip(tag & 7);",
];
const UNKNOWN_HELPER = `function skipUnknownProtoField(reader: BinaryReader, tag: number): void {\n${UNKNOWN_BODY.map(line => `  ${line}`).join("\n")}\n}\n`;
const UNKNOWN_CALL = "skipUnknownProtoField(reader, tag);";

// The retained public fromPartial workload showed these scalar loops were costly.
const DIRECT_SCALAR_CODECS = new Set([
  "ClientPayload", "Message", "Message_ImageMessage", "ContextInfo", "DisappearingMode",
]);

/** The wire guard must still inspect the exact shared framing operation, not trust its name. */
export function expandSharedUnknownFields(source: string): string {
  const calls = source.includes(UNKNOWN_CALL);
  const helper = source.includes("function skipUnknownProtoField(");
  if (!calls && !helper) return source;
  if (!calls || !helper || source.split(UNKNOWN_HELPER).length !== 2) {
    throw new Error("shared unknown-field helper changed its framing contract");
  }
  return source.replace(UNKNOWN_HELPER, "").replace(
    /^([ \t]*)skipUnknownProtoField\(reader, tag\);$/gm,
    (_, indent: string) => UNKNOWN_BODY.map(line => indent + line).join("\n"),
  );
}

export interface SharingReport {
  text: string;
  unknownEpilogues: number;
  scalarRuns: number;
  scalarFields: number;
  createMethods: number;
}

/** Share only post-value operations; all fresh base construction and field reads stay ordered. */
export function shareProtoPrivateWork(source: string): SharingReport {
  for (const name of ["skipUnknownProtoField", "copyPartialScalars", "createPartialMessage", "partialScalarKeys"]) {
    if (source.includes(name)) throw new Error(`generated sharing helper collision: ${name}`);
  }
  const epilogue = /^([ \t]+)if \(tag >>> 3 === 0 \|\| \(tag & 7\) === 4\) \{\n[ \t]+throw new RangeError\(`illegal protobuf tag \$\{tag\} at offset \$\{reader.pos\}`\);\n\1\}\n\1reader\.skip\(tag & 7\);/gm;
  let unknownEpilogues = 0;
  const loops = source.split("reader.skip(tag & 7);").length - 1;
  let text = source.replace(epilogue, (_, indent: string) => {
    unknownEpilogues++;
    return indent + UNKNOWN_CALL;
  });
  if (!loops || unknownEpilogues !== loops) throw new Error(`unknown framing shape drift (${unknownEpilogues}/${loops})`);
  const file = ts.createSourceFile("whatsapp.ts", text, ts.ScriptTarget.Latest, true);
  const edits: { start: number; end: number; text: string }[] = [];
  const keys: string[] = [];
  let scalarRuns = 0;
  let scalarFields = 0;
  let createMethods = 0;
  let partialMethods = 0;
  function visit(node: ts.Node) {
    if (ts.isMethodDeclaration(node) && node.name.getText(file) === "fromPartial" && node.body) {
      partialMethods++;
      const statements = node.body.statements;
      if (!/^const message =\s*createBase\w+\(\);$/.test(statements[0]?.getText(file) ?? "") ||
        statements[statements.length - 1]?.getText(file) !== "return message;") {
        throw new Error("fromPartial base construction changed");
      }
      const codec = node.parent.parent;
      if (!ts.isVariableDeclaration(codec) || !ts.isIdentifier(codec.name)) {
        throw new Error("fromPartial codec identity changed");
      }
      const directScalars = DIRECT_SCALAR_CODECS.has(codec.name.text);
      let run: { node: ts.Statement; key: string }[] = [];
      function finish() {
        // One field cannot amortize a call and private metadata; leave it alone.
        if (!directScalars && run.length >= 2) {
          const start = keys.length;
          keys.push(...run.map(field => field.key));
          edits.push({ start: run[0].node.getStart(file), end: run[run.length - 1].node.end,
            text: `copyPartialScalars(message, object, ${start}, ${keys.length});` });
          scalarRuns++;
          scalarFields += run.length;
        }
        run = [];
      }
      for (const statement of statements.slice(1, -1)) {
        const match = /^message\.([A-Za-z0-9_]+) = object\.\1 \?\? undefined;$/.exec(statement.getText(file));
        if (match) run.push({ node: statement, key: match[1] });
        else finish();
      }
      finish();
    }
    if (ts.isMethodDeclaration(node) && node.name.getText(file) === "create" && node.body) {
      const body = node.body.statements;
      const statement = body[0];
      const call = statement && ts.isReturnStatement(statement) ? statement.expression : undefined;
      const callee = call && ts.isCallExpression(call) ? call.expression : undefined;
      const argument = call && ts.isCallExpression(call) ? call.arguments[0] : undefined;
      if (body.length !== 1 || !call || !ts.isCallExpression(call) || call.arguments.length !== 1 ||
        !callee || !ts.isPropertyAccessExpression(callee) || callee.name.text !== "fromPartial" ||
        !ts.isIdentifier(callee.expression) || !argument || !ts.isBinaryExpression(argument) ||
        argument.operatorToken.kind !== ts.SyntaxKind.QuestionQuestionToken ||
        !ts.isIdentifier(argument.left) || argument.left.text !== "base" ||
        !ts.isObjectLiteralExpression(argument.right) || argument.right.properties.length) {
        throw new Error("generated create method changed");
      }
      edits.push({ start: statement.getStart(file), end: statement.end,
        text: `return createPartialMessage(${callee.expression.text}, base);` });
      createMethods++;
    }
    ts.forEachChild(node, visit);
  }
  visit(file);
  if (!partialMethods || partialMethods !== createMethods) throw new Error("generated construction surface changed");
  for (const edit of edits.sort((a, b) => b.start - a.start)) {
    text = text.slice(0, edit.start) + edit.text + text.slice(edit.end);
  }
  text += `\n${UNKNOWN_HELPER}\n`;
  if (keys.length) text += `function copyPartialScalars(message: any, object: any, start: number, end: number): void {
  for (let index = start; index < end; index++) {
    const key = partialScalarKeys[index];
    message[key] = object[key] ?? undefined;
  }
}
\nconst partialScalarKeys: readonly string[] = ${JSON.stringify(keys)};\n`;
  text += `\nfunction createPartialMessage(codec: { fromPartial(object: any): any }, base: any): any {
  return codec.fromPartial(base ?? {});
}
`;
  return { text, unknownEpilogues, scalarRuns, scalarFields, createMethods };
}
