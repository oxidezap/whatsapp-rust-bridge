import assert from "node:assert/strict";
import { beforeAll, describe, test } from "bun:test";
import {
  decodeSessionRecordComponents,
  importLegacySessionRecordV1,
  initWasmEngine,
  projectLegacySessionRecordV1,
  type LegacySessionRecordV1,
} from "../dist/index.js";

const bytes = (length: number, value: number) => new Uint8Array(length).fill(value);
const publicKey = (value: number) => {
  const key = bytes(33, value);
  key[0] = 0x05;
  return key;
};

const legacyRecord = (withSkippedKey = false): LegacySessionRecordV1 => {
  const baseKey = publicKey(1);
  const senderRatchet = publicKey(2);
  const receiverRatchet = publicKey(3);
  return {
    sessions: [
      {
        indexKey: baseKey,
        session: {
          registrationId: 4,
          ratchet: {
            keyPair: { public: senderRatchet, private: bytes(32, 5) },
            lastRemoteEphemeralKey: receiverRatchet,
            previousCounter: 6,
            rootKey: bytes(32, 7),
          },
          index: {
            baseKey,
            baseKeyRole: 2,
            closedTimestamp: -1,
            usedAtMs: 8,
            createdAtMs: 9,
            remoteIdentityKey: publicKey(10),
          },
          chains: [
            {
              ratchetKey: senderRatchet,
              role: 1,
              chainKey: { counter: -1, key: bytes(32, 11) },
              messageKeys: [],
            },
            {
              ratchetKey: receiverRatchet,
              role: 2,
              chainKey: { counter: 1, key: bytes(32, 12) },
              messageKeys: withSkippedKey ? [{ index: 0, seed: bytes(32, 13) }] : [],
            },
          ],
          pendingPreKey: undefined,
        },
      },
    ],
  };
};

const context = { identityKey: publicKey(14), registrationId: 15 };

beforeAll(() => initWasmEngine());

describe("legacy session v1 boundary", () => {
  test("delegates role, counter and skipped-key conversion to the core", () => {
    const canonical = decodeSessionRecordComponents(
      importLegacySessionRecordV1(legacyRecord(true), context),
    ).currentSession!;

    assert.equal(canonical.senderChain?.chainKey.index, 0);
    assert.equal(canonical.receiverChains[0]?.chainKey?.index, 2);
    assert.equal(canonical.receiverChains[0]?.messageKeys[0]?.material.kind, "derived");
    assert.deepEqual(canonical.localIdentityPublic, context.identityKey);
    assert.equal(canonical.localRegistrationId, context.registrationId);
  });

  test("returns an operational projection without reconstructing metadata in the bridge", () => {
    const canonical = importLegacySessionRecordV1(legacyRecord(), context);
    const projection = projectLegacySessionRecordV1(canonical);

    assert.equal(projection.status, "projected");
    if (projection.status !== "projected") return;
    const session = projection.record.sessions[0]!.session;
    assert.equal(session.index.baseKeyRole, 2);
    assert.equal(session.index.closedTimestamp, -1);
    assert.equal(session.index.createdAtMs, 0);
    assert.equal(session.chains.find(chain => chain.role === 1)?.chainKey.counter, -1);
  });

  test("reports non-reversible skipped keys as a typed projection result", () => {
    const canonical = importLegacySessionRecordV1(legacyRecord(true), context);
    const projection = projectLegacySessionRecordV1(canonical);

    assert.equal(projection.status, "unrepresentable");
    if (projection.status !== "unrepresentable") return;
    assert.equal(projection.issue.field, "derived_message_key");
    assert.equal(projection.issue.session, 0);
    assert.equal(projection.issue.chain, 1);
  });

  test("accepts the v1 never-used sending-chain counter across the boundary", () => {
    const record = legacyRecord();
    record.sessions[0]!.session.ratchet.previousCounter = -1;
    const canonical = decodeSessionRecordComponents(
      importLegacySessionRecordV1(record, context),
    ).currentSession!;

    assert.equal(canonical.previousCounter, 0);
  });

  test("keeps previous-counter range validation inside the core", () => {
    const record = legacyRecord();
    record.sessions[0]!.session.ratchet.previousCounter = -2;
    assert.throws(
      () => importLegacySessionRecordV1(record, context),
      /legacy chain counter -2/,
    );
  });

  test("keeps role validation inside the core", () => {
    const invalid = legacyRecord();
    invalid.sessions[0]!.session.chains[0]!.role = 99;
    assert.throws(
      () => importLegacySessionRecordV1(invalid, context),
      /unknown legacy chain role 99/,
    );
  });
});
