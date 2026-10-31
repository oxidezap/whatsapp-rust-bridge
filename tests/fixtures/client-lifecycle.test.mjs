// Copied into an isolated consumer by check:published-tarball. Discover the
// matrix from the installed declarations, never from minified glue symbols.
import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import * as ts from 'typescript/unstable/ast';
import { API } from 'typescript/unstable/async';
import { resolve } from 'node:path';

const name = '@oxidezap/whatsapp-rust-bridge';
const entry = process.env.BRIDGE_ENTRY === 'host' ? `${name}/host` : name;
const bridge = await import(entry);
if (process.env.BRIDGE_ENTRY === 'host') {
  bridge.initSync({ module: readFileSync(fileURLToPath(import.meta.resolve(`${name}/wasm`))) });
}
const declaration = fileURLToPath(new URL('./whatsapp_rust_bridge.d.ts', import.meta.resolve(entry)));
const config = resolve('.lifecycle-tsconfig.json');
const files = {
  [declaration]: readFileSync(declaration, 'utf8'),
  [config]: JSON.stringify({ compilerOptions: { noLib: true, types: [] }, files: [declaration] }),
};
const api = new API({ fs: { readFile: name => files[name], fileExists: name => name in files ? true : undefined } });
let source;
try {
  const snapshot = await api.updateSnapshot({ openProjects: [config] });
  source = await snapshot.getProject(config).program.getSourceFile(declaration);
} finally { await api.close(); }
const clientClass = source.statements.find(s => ts.isClassDeclaration(s) && s.name?.text === 'WasmWhatsAppClient');
assert.ok(clientClass, 'installed package declares its client');
const declarations = source.statements.filter(s => (ts.isClassDeclaration(s) || ts.isInterfaceDeclaration(s)) && s.name?.text === 'WasmWhatsAppClient');
const methods = declarations.flatMap(d => d.members).filter(m => (ts.isMethodDeclaration(m) || ts.isMethodSignatureDeclaration(m)) && ts.isTypeReferenceNode(m.type) && m.type.typeName.getText(source) === 'Promise');
assert.equal(new Set(methods.map(m => m.name.getText(source))).size, methods.length, 'each method is swept once');
assert.ok(methods.length > 0, 'discover Promise methods from the installed declaration');

async function client(transport = {}, onEvent) {
  return bridge.createWhatsAppClient(
    { connect() {}, send() {}, disconnect() {}, ...transport },
    { async execute() { return {statusCode: 0, body: new Uint8Array()}; } },
    onEvent,
  );
}
const outcome = promise => Promise.resolve(promise).then(
  value => ({ value }), error => ({ error }),
);
function argument(type) {
  if (!type) return undefined;
  if (ts.isUnionTypeNode(type)) return argument(type.types.find(t => t.kind !== ts.SyntaxKind.UndefinedKeyword && t.kind !== ts.SyntaxKind.NullKeyword));
  if (ts.isLiteralTypeNode(type)) {
    if (ts.isStringLiteral(type.literal)) return type.literal.text;
    if (type.literal.kind === ts.SyntaxKind.TrueKeyword) return true;
    if (type.literal.kind === ts.SyntaxKind.FalseKeyword) return false;
    if (ts.isNumericLiteral(type.literal)) return Number(type.literal.text);
  }
  if (ts.isArrayTypeNode(type)) return [];
  switch (type.kind) {
    case ts.SyntaxKind.StringKeyword: return '10000000000@s.whatsapp.net';
    case ts.SyntaxKind.NumberKeyword: return 0;
    case ts.SyntaxKind.BooleanKeyword: return false;
    case ts.SyntaxKind.FunctionType: return () => new ReadableStream({start(c) { c.close(); }});
  }
  const named = ts.isTypeReferenceNode(type) ? type.typeName.getText(source) : '';
  if (named === 'Uint8Array') return new Uint8Array();
  if (named === 'Array') return [];
  if (named === 'ReadableStream') return new ReadableStream({start(c) {c.close();}});
  if (named === 'WritableStream') return new WritableStream();
  // DTOs intentionally take an invalid value. These cases pin that rejection
  // itself cannot retain the wrapper. Valid stateful paths have fixtures below.
  return {};
}

for (const method of methods) {
  const methodName = method.name.getText(source);
  test(`immediate free after ${methodName}`, { timeout: 5000 }, async () => {
    const c = await client();
    let pending;
    try {
      pending = outcome(c[methodName](...method.parameters.map(p => p.questionToken ? undefined : argument(p.type))));
    } catch (error) {
      // Input ABI checks may reject before entering Rust. Ownership failures
      // are never an input error and must not be hidden by this branch.
      assert.doesNotMatch(String(error), /ownership|borrowed|out of bounds|unreachable/i);
    }
    assert.doesNotThrow(() => c.free());
    if (pending) {
      const result = await pending;
      if (result.error) {
        assert.doesNotMatch(String(result.error), /ownership|borrowed|out of bounds|unreachable/i);
        assert.equal(result.error.name, 'WhatsAppError', `${methodName} rejects through BridgeError`);
        assert.equal(typeof result.error.kind, 'string');
      }
    }
  });
}

test('valid concurrent calls settle after immediate free', async () => {
  const c = await client();
  const pending = [outcome(c.fetchBlocklist()), outcome(c.fetchPrivacySettings()), outcome(c.getPushName())];
  assert.doesNotThrow(() => c.free());
  const [blocklist, privacy, pushName] = await Promise.all(pending);
  assert.equal(blocklist.error?.kind, 'not-connected');
  assert.equal(privacy.error?.kind, 'not-connected');
  assert.equal(pushName.error?.kind, 'not-connected');
});

test('disconnect remains the persistence barrier before free', async () => {
  const c = await client();
  const pending = outcome(c.fetchBlocklist());
  await c.disconnect();
  c.free();
  assert.equal((await pending).error?.kind, 'not-connected');
});

test('pending logout owns its teardown after free', async () => {
  const c = await client();
  const pending = outcome(c.logout());
  c.free();
  const result = await pending;
  if (result.error) assert.equal(result.error.kind, 'not-connected');
});

test('free cancels a call that has already entered its wait', { timeout: 5000 }, async () => {
  const c = await client();
  const pending = outcome(c.waitForSocket(60000));
  await c.getPushName();
  c.free();
  assert.equal((await pending).error?.kind, 'not-connected');
});

test('resolved operations retain their result after free', async () => {
  const c = await client();
  const value = await c.getPushName();
  c.free();
  assert.equal(typeof value, 'string');
});

for (const reject of [false, true]) {
  test(`late transport ${reject ? 'rejection' : 'resolution'} after free`, { timeout: 5000 }, async () => {
    let entered, release;
    const checkpoint = new Promise(resolve => { entered = resolve; });
    const callback = new Promise((resolve, rejectCallback) => {
      release = reject ? () => rejectCallback(new Error('late transport failure')) : resolve;
    });
    const c = await client({connect() { entered(); return callback; }});
    const pending = outcome(c.connect());
    await checkpoint;
    c.free();
    release();
    const result = await pending;
    if (result.error) assert.equal(result.error.name, 'WhatsAppError');
  });
}

test('free consumes the handle exactly once', () => client().then(c => {
  c.free();
  assert.throws(() => c.free(), /null pointer passed to rust/);
  assert.throws(() => c.getPushName(), /Attempt to use a moved value|null pointer passed to rust/);
}));

test('logout that entered the core completes after free', { timeout: 5000 }, async () => {
  let entered;
  const checkpoint = new Promise(resolve => { entered = resolve; });
  const c = await client({}, event => { if (event.type === 'logged_out') entered(); });
  const pending = outcome(c.logout());
  await checkpoint;
  c.free();
  assert.equal((await pending).error, undefined);
});
