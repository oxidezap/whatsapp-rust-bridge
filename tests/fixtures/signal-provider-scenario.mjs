import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createCipheriv, createDecipheriv, createHmac } from 'node:crypto';
import { initWasmEngine, generateKeyPair, createWhatsAppClient } from '../../dist/index.js';

const group = '120363000000000000@g.us';
const author = '5511000000001:1@s.whatsapp.net';
const counts = { encrypt: 0, decrypt: 0, hmac: 0 };
let failEncrypt = false;
const callbacks = {
  aesCbc256Encrypt(key, iv, plaintext) {
    counts.encrypt++;
    if (failEncrypt) throw new Error('synthetic host failure');
    const cipher = createCipheriv('aes-256-cbc', key, iv);
    return Buffer.concat([cipher.update(plaintext), cipher.final()]);
  },
  aesCbc256Decrypt(key, iv, ciphertext) {
    counts.decrypt++;
    const cipher = createDecipheriv('aes-256-cbc', key, iv);
    return Buffer.concat([cipher.update(ciphertext), cipher.final()]);
  },
  aesGcm256Encrypt() { throw new Error('unexpected GCM'); },
  aesGcm256Decrypt() { throw new Error('unexpected GCM'); },
  hmacSha256(key, data) { counts.hmac++; return createHmac('sha256', key).update(data).digest(); },
  hmacSha256TwoPart(key, first, second) { counts.hmac++; return createHmac('sha256', key).update(first).update(second).digest(); },
};
function memoryStore(rows) {
  return {
    async get(store, key) { return rows.get(`${store}:${key}`)?.slice() ?? null; },
    async set(store, key, value) { rows.set(`${store}:${key}`, value.slice()); },
    async delete(store, key) { rows.delete(`${store}:${key}`); },
    async commitPatch() { throw Error('unexpected appstate'); },
    async touchTcTokenSenderTimestamp() { throw Error('unexpected token'); },
    async storeReceivedTcToken() { throw Error('unexpected token'); },
  };
}
const clients = [];
async function client(rows) {
  const result = await createWhatsAppClient(
    { connect() { throw Error('network forbidden'); }, send() { throw Error('network forbidden'); }, disconnect() {} },
    { async execute() { throw Error('network forbidden'); } }, undefined, memoryStore(rows),
  );
  clients.push(result);
  return result;
}
initWasmEngine();
const key = generateKeyPair();
const pair = [...key.privKey, ...key.pubKey.slice(1)];
const device = {
  pn: { user: '5511000000001', server: 's.whatsapp.net', device: 1, agent: 0, integrator: 0 }, lid: null,
  registration_id: 123, noise_key: pair, identity_key: pair, signed_pre_key: pair, signed_pre_key_id: 1,
  signed_pre_key_signature: Array(64).fill(0), adv_secret_key: Array(32).fill(0), account: null,
  push_name: 'synthetic', app_version_primary: 2, app_version_secondary: 3000, app_version_tertiary: 0,
  app_version_last_fetched_ms: 0,
};
const freshRows = () => new Map([['device:device', new TextEncoder().encode(JSON.stringify(device))]]);
try {
  const oldClient = await client(freshRows());
  initWasmEngine(undefined, callbacks);
  const senderRows = freshRows();
  const sender = await client(senderRows);
  const receiverRows = freshRows();
  const receiver = await client(receiverRows);
  const payload = new Uint8Array(12288).fill(7);
  await oldClient.signalEncryptGroupMessage(group, payload, '');
  assert.equal(counts.encrypt, 0, 'existing client must retain Rust backend');
  const first = await sender.signalEncryptGroupMessage(group, payload, '');
  assert.equal(counts.encrypt, 1, 'new client must call host backend');
  await receiver.signalProcessSenderKeyDistribution(group, author, first.senderKeyDistributionMessage);
  const second = await sender.signalEncryptGroupMessage(group, payload.fill(8), '');
  const third = await sender.signalEncryptGroupMessage(group, payload.fill(9), '');
  const bad = third.ciphertext.slice();
  bad[bad.length - 1] ^= 1;
  await assert.rejects(receiver.signalDecryptGroupMessage(group, author, bad));
  assert.deepEqual(await receiver.signalDecryptGroupMessage(group, author, third.ciphertext), payload);
  await receiver.disconnect();
  const restarted = await client(receiverRows);
  assert.deepEqual(await restarted.signalDecryptGroupMessage(group, author, first.ciphertext), new Uint8Array(12288).fill(7));
  assert.deepEqual(await restarted.signalDecryptGroupMessage(group, author, second.ciphertext), new Uint8Array(12288).fill(8));
  const persistedBefore = [...senderRows].filter(([key]) => key.startsWith('sender_key:'));
  failEncrypt = true;
  await assert.rejects(sender.signalEncryptGroupMessage(group, payload, ''));
  assert.deepEqual([...senderRows].filter(([key]) => key.startsWith('sender_key:')), persistedBefore);
  failEncrypt = false;
  const retried = await sender.signalEncryptGroupMessage(group, payload.fill(10), '');
  assert.deepEqual(await restarted.signalDecryptGroupMessage(group, author, retried.ciphertext), payload);
  assert.ok(counts.decrypt >= 4, 'group receive must call host backend');
  const history = JSON.parse(readFileSync(new URL('./signal-group-historical.json', import.meta.url), 'utf8'));
  const historicalRows = new Map(history.rows.map(([key, value]) => [key, new Uint8Array(Buffer.from(value, 'base64'))]));
  const historicalClient = await client(historicalRows);
  for (const message of history.delayed) {
    const opened = await historicalClient.signalDecryptGroupMessage(history.group, history.author, new Uint8Array(Buffer.from(message.ciphertext, 'base64')));
    assert.deepEqual(opened, new Uint8Array(Buffer.from(message.plaintext, 'base64')));
  }
  console.log('synthetic group roundtrip, restart, skipped keys, provider isolation and failure rollback passed');
} finally {
  await Promise.all(clients.map(value => value.disconnect()));
}
