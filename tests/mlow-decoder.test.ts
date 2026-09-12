/**
 * Pure-Rust stateful MLOW audio decoder: stream decode, concealment,
 * geometry handling, RED (PT 121) depacketization, state reset, and resource teardown.
 *
 * Covers what does not require a live call: MlowAudioDecoder decodes
 * inbound MLOW packets directly to Float32Array PCM at 16 kHz mono.
 */

import { describe, test, expect, beforeAll } from "bun:test";
import { initWasmEngine, MlowAudioDecoder } from "../dist/index.js";

beforeAll(() => {
  initWasmEngine();
});

describe("MlowAudioDecoder", () => {
  test("rejects invalid RTP payload types before narrowing", () => {
    const decoder = new MlowAudioDecoder();
    try {
      for (const value of [377, 121.5, -1, NaN, Infinity, "121", {}]) {
        try {
          decoder.decode(new Uint8Array(), value as number);
          throw new Error("expected invalid payload type to throw");
        } catch (error) {
          expect(error).toMatchObject({ name: "WhatsAppError", kind: "invalid-argument", field: "payloadType" });
        }
      }
    } finally {
      decoder.free();
    }
  });
  test("instantiates independently without WasmWhatsAppClient or network", () => {
    const decoder = new MlowAudioDecoder();
    try {
      expect(decoder).toBeInstanceOf(MlowAudioDecoder);
    } finally {
      decoder.free();
    }
  });

  test("empty payload conceals to 60 ms (960 samples) of Float32Array silence", () => {
    const decoder = new MlowAudioDecoder();
    try {
      const pcm = decoder.decode(new Uint8Array(0));
      expect(pcm).toBeInstanceOf(Float32Array);
      expect(pcm.length).toBe(960);
      for (let i = 0; i < pcm.length; i++) {
        expect(pcm[i]).toBe(0.0);
      }
    } finally {
      decoder.free();
    }
  });

  test("20 ms frame (TOC 0x48) decodes to 320 samples of 16 kHz mono PCM", () => {
    const decoder = new MlowAudioDecoder();
    try {
      const packet = new Uint8Array([0x48, 0xaa, 0xbb, 0xcc]);
      const pcm = decoder.decode(packet);
      expect(pcm).toBeInstanceOf(Float32Array);
      expect(pcm.length).toBe(320);
      for (let i = 0; i < pcm.length; i++) {
        expect(pcm[i]).toBeGreaterThanOrEqual(-1.0);
        expect(pcm[i]).toBeLessThanOrEqual(1.0);
      }
    } finally {
      decoder.free();
    }
  });

  test("120 ms-shaped payload produces bounded 1920-sample output", () => {
    const decoder = new MlowAudioDecoder();
    try {
      const packet = new Uint8Array([
        0x58, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x11, 0x22,
      ]);
      const pcm = decoder.decode(packet, 120);
      expect(pcm).toBeInstanceOf(Float32Array);
      expect(pcm.length).toBe(1920);
      for (let i = 0; i < pcm.length; i++) {
        expect(pcm[i]).toBeGreaterThanOrEqual(-1.0);
        expect(pcm[i]).toBeLessThanOrEqual(1.0);
      }
    } finally {
      decoder.free();
    }
  });

  test("decoder maintains state across successive packets in a stream", () => {
    const streamDecoder = new MlowAudioDecoder();
    const freshDecoder = new MlowAudioDecoder();
    try {
      const packet1 = Buffer.from(
        "50e5638cd7b84c934ad6200696fdd57ad59328d16487059c4ceba9a663aee2f5acab3abcbf296a877865651e1ca70d7f4567d13c3ab300866f5fd31dc6d808bfff1d996ee98d8a3307b9e8277a4d9cd5771d7915ebb8fd348d18b34f7102aae659577fa18959f7217d7f9f8ab2469e7a3d1a9f9daff512062639e7c32b457a4201c75ead85a3da95d42825ed3d0c7b97aad7f37fae01b58dc8f92a61cbd9fa92bef45c3180",
        "hex"
      );
      const packet2 = Buffer.from(
        "50e5ea1b94e8710736a2e91ccb2b4d22d5749f72a52de80b3311f6894ee7cf492abad60b77ad18aec84e537a8dc5bdf339b3be1e3deab8afc0afd300debc9c9b8d08fb37237d99be57f81d5285c864cbb87433ea91582c7dd267f3caafa3340546bb11f9b7e8ead0f06c6f5a2ad0c5dba80b92898759c03ae8166c59fc189f416e59feaaf67cba0319d899ab4652f4c2cc66193e5178c703f98ad874a52ed8ffbf945d595ec9d9e4e0832710",
        "hex"
      );

      // Prime streamDecoder with packet 1
      streamDecoder.decode(packet1, 120);
      // Decode packet 2 on the primed stream vs fresh
      const streamPcm = streamDecoder.decode(packet2, 120);
      const freshPcm = freshDecoder.decode(packet2, 120);

      expect(streamPcm.length).toBe(960);
      expect(freshPcm.length).toBe(960);
      // Internal predictor/CELP filter history makes subsequent packet output state-dependent
      expect(streamPcm).not.toEqual(freshPcm);

      // Resetting the primed decoder restores it to cold-start behavior
      streamDecoder.reset();
      const afterResetPcm = streamDecoder.decode(packet2, 120);
      expect(afterResetPcm).toEqual(freshPcm);
    } finally {
      streamDecoder.free();
      freshDecoder.free();
    }
  });

  test("decoder supports RED payload type 121 and does not keep sticky redundancy", () => {
    const decoder = new MlowAudioDecoder();
    const bareDecoder = new MlowAudioDecoder();
    try {
      const bareFrame = new Uint8Array([0x48, 0xaa, 0xbb, 0xcc]);
      // Construct SplitRed N=1 packet:
      // header: [0x80 | time_code, size, 0x00 (main marker)], red_data, main_data
      const redHeader = [0x80, 0x02, 0x00];
      const redData = [0x11, 0x22];
      const redPacket = new Uint8Array([
        ...redHeader,
        ...redData,
        ...bareFrame,
      ]);

      // Decode with payloadType 121: unwraps RED envelope and decodes main frame
      const redPcm = decoder.decode(redPacket, 121);
      const expectedPcm = bareDecoder.decode(bareFrame, 120);

      expect(redPcm).toBeInstanceOf(Float32Array);
      expect(redPcm.length).toBe(320);
      expect(redPcm).toEqual(expectedPcm);

      // Subsequent packet with payloadType 120 or omitted must decode bare frame
      // without lingering RED depacketization state
      const nextBare = new Uint8Array([0x48, 0x33, 0x44, 0x55]);
      const nextPcm = decoder.decode(nextBare, 120);
      const nextExpected = bareDecoder.decode(nextBare, 120);

      expect(nextPcm).toBeInstanceOf(Float32Array);
      expect(nextPcm.length).toBe(320);
      expect(nextPcm).toEqual(nextExpected);
    } finally {
      decoder.free();
      bareDecoder.free();
    }
  });

  test("reset clears stream state back to clean initial condition", () => {
    const decoder = new MlowAudioDecoder();
    try {
      const packet = new Uint8Array([0x48, 0xaa, 0xbb, 0xcc]);
      decoder.decode(packet);
      decoder.reset();

      // After reset, decoding a packet behaves like a fresh stream
      const freshDecoder = new MlowAudioDecoder();
      try {
        const afterResetPcm = decoder.decode(packet);
        const freshPcm = freshDecoder.decode(packet);
        expect(afterResetPcm.length).toBe(freshPcm.length);
        expect(afterResetPcm).toEqual(freshPcm);
      } finally {
        freshDecoder.free();
      }
    } finally {
      decoder.free();
    }
  });

  test("free invalidates the instance and prevents use-after-free", () => {
    const decoder = new MlowAudioDecoder();
    decoder.free();

    expect(() => decoder.decode(new Uint8Array(0))).toThrow();
    expect(() => decoder.reset()).toThrow();
  });

  test("Symbol.dispose delegates to free", () => {
    const decoder = new MlowAudioDecoder();
    decoder[Symbol.dispose]();

    expect(() => decoder.decode(new Uint8Array(0))).toThrow();
  });
});
