import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";

// Frozen from audit-bridge/pkg before migrating host callbacks; never derive from the catalog.
const baseline = JSON.parse(readFileSync(join(import.meta.dir, "fixtures/host-callback-contract.json"), "utf8")) as { interfaces: Record<string, string[]> };

test("host callback catalogs preserve public member types and optionality", () => {
  const source = ts.createSourceFile("bridge.d.ts", readFileSync(join(import.meta.dir, "../pkg/whatsapp_rust_bridge.d.ts"), "utf8"), ts.ScriptTarget.Latest, true);
  for (const [name, members] of Object.entries(baseline.interfaces)) {
    const declarations = source.statements.filter((node): node is ts.InterfaceDeclaration => ts.isInterfaceDeclaration(node) && node.name.text === name);
    expect(declarations.length, name).toBe(1);
    expect(declarations[0]!.members.map(member => member.getText(source).replace(/\s+/g, " ")), name).toEqual(members);
  }
});
