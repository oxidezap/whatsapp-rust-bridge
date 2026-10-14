import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";

// Captured from the compiled bridge/core revisions recorded in this fixture,
// before moving the methods into the catalog. Do not regenerate from the catalog.
const baseline = JSON.parse(
  readFileSync(join(import.meta.dir, "fixtures/generated-operation-contract.json"), "utf8")
) as { signatures: Record<string, string>; storageCallbacks: Record<string, string>; mergedSignatures: Record<string, string> };

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
  const store = source.statements.find(
    (statement): statement is ts.InterfaceDeclaration => ts.isInterfaceDeclaration(statement) && statement.name.text === "JsStoreCallbacks"
  );
  expect(store).toBeDefined();
  for (const [name, signature] of Object.entries(baseline.storageCallbacks)) {
    const members = store!.members.filter(member => member.name?.getText(source) === name);
    expect(members.length, name).toBe(1);
    expect(members[0]!.getText(source).replace(/\s+/g, " "), name).toBe(signature);
  }
  const merged = source.statements.find(
    (statement): statement is ts.InterfaceDeclaration => ts.isInterfaceDeclaration(statement) && statement.name.text === "WasmWhatsAppClient"
  );
  expect(merged).toBeDefined();
  for (const [name, signature] of Object.entries(baseline.mergedSignatures)) {
    const members = merged!.members.filter(member => member.name?.getText(source) === name);
    expect(members.length, name).toBe(1);
    expect(members[0]!.getText(source).replace(/\s+/g, " "), name).toBe(signature);
  }
  for (const [name, signature] of Object.entries(baseline.signatures)) {
    const methods = client!.members.filter((member) => member.name?.getText(source) === name);
    expect(methods.length, name).toBe(1);
    expect(methods[0]!.getText(source).replace(/\s+/g, " "), name).toBe(signature);
  }
});

// Capture comes from the compiled pre-refactor bridge, independently of schemas.
test("generated DTOs preserve field types, optionality and binary declarations", () => {
  const baseline = JSON.parse(readFileSync(join(import.meta.dir, "fixtures/generated-dto-contract.json"), "utf8")) as {types: Record<string, string[]>};
  const source = ts.createSourceFile("bridge.d.ts", readFileSync(join(import.meta.dir, "../pkg/whatsapp_rust_bridge.d.ts"), "utf8"), ts.ScriptTarget.Latest, true);
  for (const [name, fields] of Object.entries(baseline.types)) {
    const declarations = source.statements.filter((node): node is ts.InterfaceDeclaration => ts.isInterfaceDeclaration(node) && node.name.text === name);
    expect(declarations.length, name).toBe(1);
    expect(declarations[0]!.members.map(member => member.getText(source).replace(/\s+/g, " ")), name).toEqual(fields);
  }
});

test("generated event callbacks preserve required and optional public signatures", () => {
  const baseline = JSON.parse(readFileSync(join(import.meta.dir, "fixtures/generated-event-contract.json"), "utf8")) as {members: Record<string,string>};
  const source=ts.createSourceFile("bridge.d.ts",readFileSync(join(import.meta.dir,"../pkg/whatsapp_rust_bridge.d.ts"),"utf8"),ts.ScriptTarget.Latest,true);
  const declarations=source.statements.filter((node): node is ts.InterfaceDeclaration=>ts.isInterfaceDeclaration(node)&&node.name.text==="WhatsAppEventCallbacks");
  expect(declarations.length).toBe(1);
  expect(Object.fromEntries(declarations[0]!.members.map(member=>[member.name!.getText(source),member.getText(source).replace(/\s+/g," ")]))).toEqual(baseline.members);
});

// Frozen from the compiled pre-refactor package, independently of variant tables.
test("generated sum types preserve tags, payload types and optional fields", () => {
  const baseline = JSON.parse(readFileSync(join(import.meta.dir, "fixtures/generated-variant-contract.json"), "utf8")) as {types: Record<string, string>};
  const source = ts.createSourceFile("bridge.d.ts", readFileSync(join(import.meta.dir, "../pkg/whatsapp_rust_bridge.d.ts"), "utf8"), ts.ScriptTarget.Latest, true);
  for (const [name, shape] of Object.entries(baseline.types)) {
    const declarations = source.statements.filter((node): node is ts.TypeAliasDeclaration => ts.isTypeAliasDeclaration(node) && node.name.text === name);
    expect(declarations.length, name).toBe(1);
    expect(declarations[0]!.type.getText(source).replace(/\s+/g, " "), name).toBe(shape);
  }
});
