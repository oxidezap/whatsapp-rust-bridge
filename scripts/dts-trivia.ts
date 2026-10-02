import ts from "typescript";

interface Token {
  kind: ts.SyntaxKind;
  text: string;
}
const comment = (kind: ts.SyntaxKind | undefined) =>
  kind === ts.SyntaxKind.SingleLineCommentTrivia || kind === ts.SyntaxKind.MultiLineCommentTrivia;
const whitespace = (kind: ts.SyntaxKind) =>
  kind === ts.SyntaxKind.WhitespaceTrivia || kind === ts.SyntaxKind.NewLineTrivia;

function tokens(text: string): Token[] {
  const scanner = ts.createScanner(ts.ScriptTarget.Latest, false, ts.LanguageVariant.Standard, text);
  const out: Token[] = [];
  for (let kind = scanner.scan(); kind !== ts.SyntaxKind.EndOfFileToken; kind = scanner.scan()) {
    out.push({ kind, text: scanner.getTokenText() });
  }
  return out;
}

/** Keep comments and newline-sensitive grammar; only explicit terminators lose line breaks. */
export function compactDeclarationTrivia(text: string): string {
  const parts = tokens(text);
  // A context-free scanner cannot distinguish template tails from ordinary trivia.
  // Keep the whole declaration unchanged rather than guess at interpolation boundaries.
  if (parts.some(part => part.kind === ts.SyntaxKind.TemplateHead)) return text;
  return parts.map((part, index) => {
    if (!whitespace(part.kind)) return part.text;
    const previous = parts[index - 1];
    const next = parts[index + 1];
    if (comment(previous?.kind) || comment(next?.kind)) return part.text;
    if (part.kind === ts.SyntaxKind.NewLineTrivia) {
      let left = index - 1;
      let right = index + 1;
      while (left >= 0 && whitespace(parts[left].kind)) left--;
      while (right < parts.length && whitespace(parts[right].kind)) right++;
      return parts[left]?.kind === ts.SyntaxKind.SemicolonToken && !comment(parts[right]?.kind)
        ? ""
        : part.text;
    }
    if (!previous || !next || whitespace(previous.kind) || whitespace(next.kind)) return "";
    const joined = tokens(previous.text + next.text);
    return joined.length === 2 && joined[0].kind === previous.kind &&
      joined[0].text === previous.text && joined[1].kind === next.kind && joined[1].text === next.text
      ? "" : " ";
  }).join("");
}
