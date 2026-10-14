import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";

// Captured from the compiled bridge/core revisions recorded in this fixture,
// before moving the methods into the catalog. Do not regenerate from the catalog.
const baseline = JSON.parse(
  readFileSync(join(import.meta.dir, "fixtures/generated-operation-contract.json"), "utf8")
) as { signatures: Record<string, string> };

test("generated operations preserve the published method contracts", () => {
  const source = ts.createSourceFile(
    "bridge.d.ts",
    readFileSync(join(import.meta.dir, "../pkg/whatsapp_rust_bridge.d.ts"), "utf8"),
    ts.ScriptTarget.Latest,
    true
  );
  const client = source.statements.find(
    (statement): statement is ts.ClassDeclaration =>
      ts.isClassDeclaration(statement) && statement.name?.text === "WasmWhatsAppClient"
  );
  expect(client).toBeDefined();
  for (const [name, signature] of Object.entries(baseline.signatures)) {
    const methods = client!.members.filter((member) => member.name?.getText(source) === name);
    expect(methods.length, name).toBe(1);
    expect(methods[0]!.getText(source).replace(/\s+/g, " "), name).toBe(signature);
  }
});
