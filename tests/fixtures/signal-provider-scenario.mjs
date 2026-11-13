import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createCipheriv, createDecipheriv, createHmac } from 'node:crypto';
import { initWasmEngine, generateKeyPair, calculateSignature, verifySignature, createWhatsAppClient, BinaryReader } from '../../dist/index.js';

/** Read one public protobuf field independently of the SDK's record projections. */
function wireField(body, number, wireType) {
  const reader = new BinaryReader(body);
  while (reader.pos < reader.len) {
    const tag = reader.uint32();
    if ((tag >>> 3) === number) {
      assert.equal(tag & 7, wireType);
      return wireType === 0 ? reader.uint32() : reader.bytes();
    }
    reader.skip(tag & 7);
  }
  throw Error(`missing synthetic wire field ${number}`);
}
/** Unwrap a prekey envelope when present and inspect the authenticated counter. */
function pairwiseCounter(packet) {
  const message = packet.type === 'pkmsg' ? wireField(packet.ciphertext.subarray(1), 4, 2) : packet.ciphertext;
  return wireField(message.subarray(1, -8), 2, 0);
}

const group = '120363000000000000@g.us';
const author = '5511000000001:1@s.whatsapp.net';
const counts = { encrypt: 0, decrypt: 0, hmac: 0 };
const hmacParts = [];
let failEncrypt = false;
let rejectSenderKeyWrites = false;
let malformedCbcLength = null;
let failHmac = false;
const callbacks = {
  /** Inject failures before native encryption to expose sender rollback defects. */
  aesCbc256Encrypt(key, iv, plaintext) {
    counts.encrypt++;
    if (failEncrypt) throw new Error('synthetic host failure');
    if (malformedCbcLength !== null) return new Uint8Array(malformedCbcLength);
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
  /** Compute a reference digest while exposing the shared failure switch. */
  hmacSha256(key, data) { counts.hmac++; if (failHmac) throw Error('synthetic HMAC failure'); return createHmac('sha256', key).update(data).digest(); },
  /** Preserve split-input order in the independent Node reference implementation. */
  hmacSha256TwoPart(key, first, second) { hmacParts.push([first.length, second.length]); counts.hmac++; if (failHmac) throw Error('synthetic HMAC failure'); return createHmac('sha256', key).update(first).update(second).digest(); },
};
/** Keep synthetic rows durable across client reloads and inject selected write failures. */
function memoryStore(rows) {
  return {
    /** Return copies so later WASM writes cannot mutate an earlier storage read. */
    async get(store, key) { return rows.get(`${store}:${key}`)?.slice() ?? null; },
    /** Commit complete copied rows, or fail before writing selected sender-key rows. */
    async set(store, key, value) { if (store === 'sender_key' && rejectSenderKeyWrites) throw Error('synthetic sender-key write failure'); rows.set(`${store}:${key}`, value.slice()); },
    async delete(store, key) { rows.delete(`${store}:${key}`); },
    async commitPatch() { throw Error('unexpected appstate'); },
    async touchTcTokenSenderTimestamp() { throw Error('unexpected token'); },
    async storeReceivedTcToken() { throw Error('unexpected token'); },
  };
}
const clients = [];
/** Create an offline client over the supplied synthetic persistent rows. */
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
const freshRows = (record = device) => new Map([['device:device', new TextEncoder().encode(JSON.stringify(record))]]);
try {
  const oldClient = await client(freshRows());
  initWasmEngine(undefined, callbacks);
  const senderRows = freshRows();
  let sender = await client(senderRows);
  const receiverRows = freshRows();
  const receiver = await client(receiverRows);
  for (const user of ['1'.repeat(43), '1'.repeat(44), 'λ'.repeat(23), '🦀'.repeat(12), 'λ'.repeat(2048)]) {
    const jid = `${user}:127@s.whatsapp.net`;
    assert.equal(sender.jidToSignalProtocolAddress(jid), `${user}:127@c.us.0`);
    await assert.rejects(sender.signalEncryptMessage(jid, new Uint8Array([1])), error => {
      assert.ok(error instanceof Error);
      assert.equal(error.name, 'WhatsAppError');
      assert.equal(error.kind, 'internal');
      assert.match(error.message, /^no session for pn#[a-f0-9]{16}:127@s\.whatsapp\.net\.0$/);
      assert.ok(!error.message.includes(user), 'diagnostics must retain address redaction');
      return true;
    });
  }
  const payload = new Uint8Array(12288).fill(7);
  await oldClient.signalEncryptGroupMessage(group, payload, '');
  assert.equal(counts.encrypt, 0, 'existing client must retain Rust backend');
  const first = await sender.signalEncryptGroupMessage(group, payload, '');
  assert.equal(counts.encrypt, 1, 'new client must call host backend');
  await receiver.signalProcessSenderKeyDistribution(group, author, first.senderKeyDistributionMessage);
  const second = await sender.signalEncryptGroupMessage(group, payload.fill(8), '');
  const third = await sender.signalEncryptGroupMessage(group, payload.fill(9), '');
  const distribution = await sender.signalGetSenderKeyDistribution(group, author);
  const beforeDistribution = [...senderRows].filter(([key]) => key.startsWith('sender_key:'));
  rejectSenderKeyWrites = true;
  try {
    assert.deepEqual(await sender.signalGetSenderKeyDistribution(group, author), distribution);
    assert.deepEqual([...senderRows].filter(([key]) => key.startsWith('sender_key:')), beforeDistribution,
      'reexporting the current distribution must preserve persisted sender keys');
  } finally {
    rejectSenderKeyWrites = false;
  }
  const bad = third.ciphertext.slice();
  bad[bad.length - 1] ^= 1;
  await assert.rejects(receiver.signalDecryptGroupMessage(group, author, bad));
  assert.deepEqual(await receiver.signalDecryptGroupMessage(group, author, third.ciphertext), payload);
  // A duplicate distribution must retain consumed counters and delayed keys.
  await receiver.signalProcessSenderKeyDistribution(group, author, first.senderKeyDistributionMessage);
  await assert.rejects(receiver.signalDecryptGroupMessage(group, author, third.ciphertext));
  await receiver.disconnect();
  const restarted = await client(receiverRows);
  assert.deepEqual(await restarted.signalDecryptGroupMessage(group, author, first.ciphertext), new Uint8Array(12288).fill(7));
  assert.deepEqual(await restarted.signalDecryptGroupMessage(group, author, second.ciphertext), new Uint8Array(12288).fill(8));
  const persistedBefore = [...senderRows].filter(([key]) => key.startsWith('sender_key:'));
  failEncrypt = true;
  await assert.rejects(sender.signalEncryptGroupMessage(group, payload, ''));
  assert.deepEqual([...senderRows].filter(([key]) => key.startsWith('sender_key:')), persistedBefore);
  failEncrypt = false;
  for (const wrongLength of [0, 1, 15, 16, 17, payload.length, payload.length + 32]) {
    malformedCbcLength = wrongLength;
    await assert.rejects(sender.signalEncryptGroupMessage(group, payload, ''));
    assert.deepEqual([...senderRows].filter(([key]) => key.startsWith('sender_key:')), persistedBefore,
      'malformed CBC output must not advance persisted sender keys');
  }
  malformedCbcLength = null;
  const retried = await sender.signalEncryptGroupMessage(group, payload.fill(10), '');
  assert.equal(wireField(third.ciphertext.subarray(1, -64), 2, 0), 2);
  assert.equal(wireField(retried.ciphertext.subarray(1, -64), 2, 0), 3,
    'same-client group failures must not burn a sender counter');
  assert.deepEqual(await restarted.signalDecryptGroupMessage(group, author, retried.ciphertext), payload);
  assert.ok(counts.decrypt >= 4, 'group receive must call host backend');
  const bobKey = generateKeyPair();
  const bobPair = [...bobKey.privKey, ...bobKey.pubKey.slice(1)];
  const signature = calculateSignature(bobKey.privKey, bobKey.pubKey);
  const bobDevice = { ...device, pn: { ...device.pn, user: '5511000000002' },
    registration_id: 456, noise_key: bobPair, identity_key: bobPair,
    signed_pre_key: bobPair, signed_pre_key_signature: [...signature] };
  assert.equal(verifySignature(bobKey.pubKey, bobKey.pubKey, signature), true);
  assert.equal(verifySignature(key.pubKey, bobKey.pubKey, signature), false);
  const changedSignature = signature.slice();
  changedSignature[0] ^= 1;
  assert.equal(verifySignature(bobKey.pubKey, bobKey.pubKey, changedSignature), false);
  const bobRows = freshRows(bobDevice);
  let bob = await client(bobRows);
  const bobJid = '5511000000002:1@s.whatsapp.net';
  await sender.signalInstallPreKeyBundle(bobJid, {
    registrationId: 456, identityKey: bobKey.pubKey.slice(1),
    signedPreKey: { keyId: 1, publicKey: bobKey.pubKey.slice(1), signature },
  });
  const hmacBefore = counts.hmac;
  const direct = await sender.signalEncryptMessage(bobJid, payload.fill(11));
  assert.equal(pairwiseCounter(direct), 0);
  assert.ok(counts.hmac > hmacBefore, 'large pairwise authentication must call the host HMAC');
  assert.ok(hmacParts.some(([prefix, ciphertext]) => prefix === 66 && ciphertext >= payload.length),
    'three-part authentication must pass the small identity prefix separately from the payload');
  const directSnapshot = direct.ciphertext.slice();
  const beforeMalformed = [...bobRows].filter(([key]) => key.startsWith('session:'));
  for (const truncated of [new Uint8Array(), direct.ciphertext.slice(0, 1), direct.ciphertext.slice(0, Math.floor(direct.ciphertext.length / 2))]) {
    await assert.rejects(bob.signalDecryptMessage(author, direct.type, truncated));
    assert.deepEqual([...bobRows].filter(([key]) => key.startsWith('session:')), beforeMalformed,
      'rejected envelope parsing must not persist a partial session');
  }
  const receivePartsBefore = hmacParts.length;
  assert.deepEqual(await bob.signalDecryptMessage(author, direct.type, direct.ciphertext), payload);
  assert.ok(hmacParts.slice(receivePartsBefore).some(([prefix, ciphertext]) => prefix === 66 && ciphertext >= payload.length),
    'received authentication must keep the identity prefix separate from the payload');
  const sessionsBefore = [...senderRows].filter(([key]) => key.startsWith('session:'));
  assert.ok(sessionsBefore.length > 0, 'pairwise state must be persisted before publication');
  failHmac = true;
  await assert.rejects(sender.signalEncryptMessage(bobJid, payload.fill(12)));
  assert.deepEqual([...senderRows].filter(([key]) => key.startsWith('session:')), sessionsBefore);
  failHmac = false;
  failEncrypt = true;
  await assert.rejects(sender.signalEncryptMessage(bobJid, payload));
  assert.deepEqual([...senderRows].filter(([key]) => key.startsWith('session:')), sessionsBefore);
  failEncrypt = false;
  for (const wrongLength of [0, 1, 15, 16, 17, payload.length, payload.length + 32]) {
    malformedCbcLength = wrongLength;
    await assert.rejects(sender.signalEncryptMessage(bobJid, payload));
    assert.deepEqual([...senderRows].filter(([key]) => key.startsWith('session:')), sessionsBefore,
      'malformed CBC output must not advance persisted sessions');
  }
  malformedCbcLength = null;
  const liveRetry = await sender.signalEncryptMessage(bobJid, payload);
  assert.equal(pairwiseCounter(liveRetry), 1, 'same-client pairwise failures must not burn a sender counter');
  assert.deepEqual(await bob.signalDecryptMessage(author, liveRetry.type, liveRetry.ciphertext), payload);
  await sender.disconnect();
  sender = await client(senderRows);
  const directRetry = await sender.signalEncryptMessage(bobJid, payload);
  const receiverSessionsBefore = [...bobRows].filter(([key]) => key.startsWith('session:'));
  failHmac = true;
  await assert.rejects(bob.signalDecryptMessage(author, directRetry.type, directRetry.ciphertext));
  assert.deepEqual([...bobRows].filter(([key]) => key.startsWith('session:')), receiverSessionsBefore,
    'failed receive authentication must preserve persisted sessions and counters');
  failHmac = false;
  assert.deepEqual(await bob.signalDecryptMessage(author, directRetry.type, directRetry.ciphertext), payload);
  assert.deepEqual(direct.ciphertext, directSnapshot, 'later sends must not mutate returned JS bytes');
  for (const length of [0, 1, 15, 16, 17, 127, 128, 129, 511, 512, 513, 2031, 2032, 2033, 8191, 8192, 8193]) {
    const input = new Uint8Array(length).fill(length % 251);
    const encryptsBefore = counts.encrypt;
    const encrypted = await sender.signalEncryptMessage(bobJid, input);
    assert.equal(counts.encrypt - encryptsBefore, length >= 512 ? 1 : 0,
      'CBC callback selection must retain the bridge size policy');
    assert.deepEqual(await bob.signalDecryptMessage(author, encrypted.type, encrypted.ciphertext), input);
  }
  const delayedArchivePlaintext = new Uint8Array(128).fill(61);
  const delayedArchive = await sender.signalEncryptMessage(bobJid, delayedArchivePlaintext);
  await sender.signalInstallPreKeyBundle(bobJid, {
    registrationId: 456, identityKey: bobKey.pubKey.slice(1),
    signedPreKey: { keyId: 1, publicKey: bobKey.pubKey.slice(1), signature },
  });
  const replacementPlaintext = new Uint8Array(129).fill(62);
  const replacement = await sender.signalEncryptMessage(bobJid, replacementPlaintext);
  assert.equal(replacement.type, 'pkmsg');
  assert.deepEqual(await bob.signalDecryptMessage(author, replacement.type, replacement.ciphertext), replacementPlaintext);
  await bob.disconnect();
  bob = await client(bobRows);
  assert.deepEqual(await bob.signalDecryptMessage(author, delayedArchive.type, delayedArchive.ciphertext), delayedArchivePlaintext,
    'a delayed message must promote the matching persisted archive after reload');
  for (const reload of [false, true]) {
    if (reload) { await bob.disconnect(); bob = await client(bobRows); }
    const beforeReplay = [...bobRows].filter(([key]) => key.startsWith('session:'));
    await assert.rejects(bob.signalDecryptMessage(author, delayedArchive.type, delayedArchive.ciphertext));
    assert.deepEqual([...bobRows].filter(([key]) => key.startsWith('session:')), beforeReplay,
      'prekey replay must preserve the selected session and persisted archives');
  }
  const afterPromotion = await sender.signalEncryptMessage(bobJid, new Uint8Array(127).fill(63));
  assert.deepEqual(await bob.signalDecryptMessage(author, afterPromotion.type, afterPromotion.ciphertext), new Uint8Array(127).fill(63),
    'the newer session must remain usable after archive promotion and rejected replay');
  const history = JSON.parse(readFileSync(new URL('./signal-group-historical.json', import.meta.url), 'utf8'));
  const historicalRows = new Map(history.rows.map(([key, value]) => [key, new Uint8Array(Buffer.from(value, 'base64'))]));
  const historicalClient = await client(historicalRows);
  const redistribution = new Uint8Array(Buffer.from(history.redistribution, 'base64'));
  const historicalCiphertext = new Uint8Array(Buffer.from(history.delayed[0].ciphertext, 'base64'));
  const legacyRow = history.rows.find(([key]) => key.startsWith('sender_key:'));
  await assert.rejects(historicalClient.signalDecryptGroupMessage(history.group, history.author, historicalCiphertext));
  await historicalClient.signalProcessSenderKeyDistribution(history.group, history.author, redistribution);
  for (const message of history.delayed) {
    const opened = await historicalClient.signalDecryptGroupMessage(history.group, history.author, new Uint8Array(Buffer.from(message.ciphertext, 'base64')));
    assert.deepEqual(opened, new Uint8Array(Buffer.from(message.plaintext, 'base64')));
  }
  assert.deepEqual(historicalRows.get(legacyRow[0]), new Uint8Array(Buffer.from(legacyRow[1], 'base64')),
    'authenticated migration must preserve the historical bare-address row');
  await historicalClient.signalProcessSenderKeyDistribution(history.group, history.author, redistribution);
  await assert.rejects(historicalClient.signalDecryptGroupMessage(history.group, history.author, historicalCiphertext));
  console.log('synthetic group/pairwise roundtrip, restart, skipped keys, provider isolation and failure rollback passed');
} finally {
  await Promise.all(clients.map(value => value.disconnect()));
}
