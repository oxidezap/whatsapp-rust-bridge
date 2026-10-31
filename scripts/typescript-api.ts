import { API } from "typescript/unstable/async";
import type { Node } from "typescript/unstable/ast";
import { resolve } from "node:path";

export * from "typescript/unstable/ast";

// TypeScript 7 parses through its native service; keep each virtual project isolated.
export async function createSourceFile(fileName: string, text: string) {
  const path = resolve(fileName);
  return withProject({ [path]: text }, { noLib: true, types: [] }, async ({ program }) => {
    const file = await program.getSourceFile(path);
    if (!file) throw new Error(`TypeScript did not parse ${path}`);
    return file;
  });
}

export function forEachChild<T>(node: Node, visitor: (node: Node) => T): T | undefined {
  return node.forEachChild(visitor);
}

export { SymbolFlags } from "typescript/unstable/async";
import type { Project } from "typescript/unstable/async";

export async function withProject<T>(files: Record<string, string | undefined>, options: Record<string, unknown>, run: (project: Project) => Promise<T>): Promise<T> {
  const config = resolve(".bridge-check-tsconfig.json");
  const roots = Object.keys(files);
  const virtual: Record<string, string | undefined> = { ...Object.fromEntries(Object.entries(files).filter(([, text]) => text !== undefined)),
    [config]: JSON.stringify({ compilerOptions: options, files: roots }) };
  const api = new API({ fs: {
    readFile: name => virtual[name],
    fileExists: name => name in virtual ? true : undefined,
    directoryExists: name => Object.keys(virtual).some(file => file.startsWith(`${name}/`)) ? true : undefined,
  } });
  try {
    const snapshot = await api.updateSnapshot({ openProjects: [config] });
    return await run(snapshot.getProject(config)!);
  } finally {
    await api.close();
  }
}

export async function diagnostics(files: Record<string, string | undefined>, options: Record<string, unknown>) {
  return withProject(files, options, async ({ program }) => [
    ...await program.getConfigFileParsingDiagnostics(),
    ...await program.getGlobalDiagnostics(),
    ...await program.getSyntacticDiagnostics(),
    ...await program.getSemanticDiagnostics(),
  ]);
}
