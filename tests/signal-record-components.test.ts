import assert from "node:assert/strict";
import { beforeAll, describe, test } from "bun:test";
import {
  decodeSenderKeyRecordComponents,
  decodeSessionRecordComponents,
  encodeSenderKeyRecordComponents,
  encodeSessionRecordComponents,
  initWasmEngine,
  proto,
  type SessionRecordComponents,
} from "../dist/index.js";

const bytes = (length: number, value: number) => new Uint8Array(length).fill(value);
const publicKey = (value: number) => {
  const key = bytes(33, value);
  key[0] = 0x05;
  return key;
};

const sessionRecord = (): SessionRecordComponents => ({
  currentSession: {
    sessionVersion: 3,
    localIdentityPublic: publicKey(1),
    remoteIdentityPublic: publicKey(2),
    rootKey: bytes(32, 3),
    previousCounter: 0,
    senderChain: {
      senderRatchetKey: publicKey(4),
      senderRatchetKeyPrivate: bytes(32, 5),
      chainKey: { index: 0, key: bytes(32, 6) },
      messageKeys: [],
    },
    receiverChains: [
      {
        senderRatchetKey: publicKey(7),
        chainKey: { index: 1, key: bytes(32, 8) },
        messageKeys: [],
      },
    ],
    pendingKeyExchange: undefined,
    pendingPreKey: undefined,
    remoteRegistrationId: 9,
    localRegistrationId: 10,
    needsRefresh: false,
    aliceBaseKey: publicKey(11),
  },
  previousSessions: [],
});

beforeAll(() => initWasmEngine());

describe("Signal record component boundary", () => {
  test("round-trips a complete session through the canonical record codec", () => {
    const decoded = decodeSessionRecordComponents(encodeSessionRecordComponents(sessionRecord()));

    assert.equal(decoded.currentSession?.senderChain?.chainKey.index, 0);
    assert.deepEqual(decoded.currentSession?.rootKey, bytes(32, 3));
    assert.equal("senderRatchetKeyPrivate" in decoded.currentSession!.receiverChains[0]!, false);
  });

  test("rejects role-invalid component input at the WASM boundary", () => {
    const missingSenderPrivate = sessionRecord();
    const sender = missingSenderPrivate.currentSession?.senderChain as unknown as {
      senderRatchetKeyPrivate: Uint8Array | undefined;
    };
    sender.senderRatchetKeyPrivate = undefined;
    assert.throws(
      () => encodeSessionRecordComponents(missingSenderPrivate),
      /senderRatchetKeyPrivate|sender ratchet private key/,
    );

    for (const privateMaterial of [new Uint8Array(), bytes(32, 12)]) {
      const receiverWithPrivate = sessionRecord();
      const receiver = receiverWithPrivate.currentSession?.receiverChains[0] as unknown as {
        senderRatchetKeyPrivate: Uint8Array | undefined;
      };
      receiver.senderRatchetKeyPrivate = privateMaterial;
      assert.throws(
        () => encodeSessionRecordComponents(receiverWithPrivate),
        /senderRatchetKeyPrivate|receiver ratchet private key/,
      );
    }
  });

  test("discards extrinsic receiver private material from persisted records", () => {
    for (const privateMaterial of [new Uint8Array(), bytes(1, 12), bytes(32, 13), bytes(33, 14)]) {
      const record = sessionRecord();
      const current = record.currentSession!;
      const persisted = proto.RecordStructure.encode(
        proto.RecordStructure.create({
          currentSession: proto.SessionStructure.create({
            ...current,
            receiverChains: [
              {
                ...current.receiverChains[0]!,
                senderRatchetKeyPrivate: privateMaterial,
              },
            ],
          }),
          previousSessions: [],
        }),
      ).finish();

      const receiver = decodeSessionRecordComponents(persisted).currentSession?.receiverChains[0];
      assert.equal(receiver && "senderRatchetKeyPrivate" in receiver, false);
    }
  });

  test("keeps skipped-message seeds through the core", () => {
    const seed = bytes(32, 5);
    const record = sessionRecord();
    record.currentSession!.senderChain = undefined;
    record.currentSession!.receiverChains = [
      {
        senderRatchetKey: publicKey(4),
        chainKey: undefined,
        messageKeys: [{ index: 11, material: { kind: "seed", seed } }],
      },
    ];

    const material = decodeSessionRecordComponents(encodeSessionRecordComponents(record)).currentSession
      ?.receiverChains[0]?.messageKeys[0]?.material;

    // The core retains the v1 seed alongside the keys it derives from it, so a
    // record that carried one still projects back to the legacy shape. Records
    // persisted before it did so hold only the derived keys and come back as
    // `derived`, which has no inverse.
    assert.equal(material?.kind, "seed");
    assert.deepEqual(material?.kind === "seed" ? material.seed : undefined, seed);
  });

  test("round-trips sender-key state and rejects invalid public keys", () => {
    const record = {
      states: [
        {
          keyId: 10,
          chainKey: { iteration: 0, seed: bytes(32, 1) },
          signingKey: { public: publicKey(2), private: bytes(32, 3) },
          messageKeys: [],
        },
      ],
    };
    const decoded = decodeSenderKeyRecordComponents(encodeSenderKeyRecordComponents(record));

    assert.equal(decoded.states[0]?.keyId, 10);
    assert.throws(
      () =>
        encodeSenderKeyRecordComponents({
          states: [
            {
              ...record.states[0]!,
              signingKey: { ...record.states[0]!.signingKey, public: bytes(31, 2) },
            },
          ],
        }),
      /sender signing public key/,
    );
  });
});
