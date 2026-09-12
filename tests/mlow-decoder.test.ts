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

  test("120 ms frame (TOC 0x58) decodes to 1920 samples of 16 kHz mono PCM", () => {
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
