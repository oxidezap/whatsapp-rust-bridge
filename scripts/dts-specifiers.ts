/**
 * Relative module specifiers in the published `.d.ts` files.
 *
 * NodeNext resolves a type import by exact file name, so `from "./proto"` is
 * a hard error (TS2834/TS2835) where the bundler resolution accepts it — and
 * neither tsc nor wasm-bindgen adds the extension. The sources stay
 * extensionless (that is what bun and the Rust toolchain read), so
 * `finalize-dist.ts` points the published declarations at the emitted file.
 *
 * Specifiers are collected from the TypeScript AST, in module-specifier
 * positions only (`from`, `export ... from`, `import("...")` types,
 * side-effect imports). A quoted path in a comment or in a string-literal
 * type is never a module specifier, so it is never touched. Whether a
 * specifier needs `.js` is decided by resolution against the `dist/` the
 * caller sees — not by whether the last path segment contains a dot, which a
 * dotted basename like `./foo.types` would defeat.
 */

import ts from "typescript";

export interface DtsSpecifier {
  specifier: string;
  /** Offset of the opening quote in the source text. */
  start: number;
  /** Offset just past the closing quote. */
  end: number;
  quote: string;
}

/** Every module specifier in module-specifier position, comments and string literals excluded. */
export function collectModuleSpecifiers(
  sourceText: string,
  fileName: string,
): DtsSpecifier[] {
  const source = ts.createSourceFile(
    fileName,
    sourceText,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  const found: DtsSpecifier[] = [];
  const pushLiteral = (node: ts.Expression | undefined): void => {
    if (node !== undefined && ts.isStringLiteralLike(node)) {
      const start = node.getStart(source);
      found.push({
        specifier: node.text,
        start,
        end: node.end,
        quote: sourceText[start] ?? '"',
      });
    }
  };
  const visit = (node: ts.Node): void => {
    if (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) {
      pushLiteral(node.moduleSpecifier);
    } else if (ts.isImportTypeNode(node)) {
      const argument = node.argument;
      if (ts.isLiteralTypeNode(argument)) pushLiteral(argument.literal);
    } else if (
      ts.isImportEqualsDeclaration(node) &&
      ts.isExternalModuleReference(node.moduleReference)
    ) {
      pushLiteral(node.moduleReference.expression);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return found;
}

/**
 * The specifier the published declaration must carry, or null when the source
 * specifier already resolves. `fileExists` answers dist-relative paths
 * (`"proto.d.ts"`, `"whatsapp_rust_bridge.js"`) with a file check.
 *
 * Throws when a relative specifier names nothing: leaving it would ship the
 * TS2834/TS2835 the rewrite exists to remove, and guessing would invent a
 * target no file honours.
 */
export function fixModuleSpecifier(
  specifier: string,
  fileExists: (distRelativePath: string) => boolean,
): string | null {
  if (!specifier.startsWith("./") && !specifier.startsWith("../")) {
    return null;
  }
  if (specifier.startsWith("../")) {
    throw new Error(
      `"${specifier}" points outside dist/ — the publish root is self-contained`,
    );
  }
  if (fileExists(specifier)) return null;
  if (specifier.endsWith(".js")) {
    if (fileExists(`${specifier.slice(0, -".js".length)}.d.ts`)) return null;
    throw new Error(
      `"${specifier}" names no emitted file beside a sibling declaration`,
    );
  }
  if (fileExists(`${specifier}.d.ts`) || fileExists(`${specifier}.js`)) {
    return `${specifier}.js`;
  }
  throw new Error(
    `"${specifier}" names nothing in dist/ — refusing to invent a target`,
  );
}

export interface DtsSpecifierEdit {
  from: string;
  to: string;
}

/** Rewrite every relative specifier that does not already resolve. */
export function rewriteDtsSpecifiers(
  sourceText: string,
  fileName: string,
  fileExists: (distRelativePath: string) => boolean,
): { text: string; edits: DtsSpecifierEdit[] } {
  const specs = collectModuleSpecifiers(sourceText, fileName);
  const edits: DtsSpecifierEdit[] = [];
  let text = sourceText;
  for (const spec of [...specs].reverse()) {
    let replacement: string | null;
    try {
      replacement = fixModuleSpecifier(spec.specifier, fileExists);
    } catch (error) {
      throw new Error(
        `${fileName}: ${(error as Error).message}`,
      );
    }
    if (replacement === null) continue;
    edits.push({ from: spec.specifier, to: replacement });
    text =
      text.slice(0, spec.start) +
      `${spec.quote}${replacement}${spec.quote}` +
      text.slice(spec.end);
  }
  return { text, edits };
}
