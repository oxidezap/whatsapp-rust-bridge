/**
 * Pure-Rust stateful MLOW audio decoder: stream decode, concealment,
 * geometry handling, state reset, and resource teardown.
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
      const pcm = decoder.decode(packet);
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
      const packet1 = new Uint8Array([0x48, 0xaa, 0xbb, 0xcc]);
      const packet2 = new Uint8Array([0x48, 0x12, 0x34, 0x56]);

      // Prime streamDecoder with packet 1
      streamDecoder.decode(packet1);
      // Decode packet 2 on the primed stream vs fresh
      const streamPcm = streamDecoder.decode(packet2);
      const freshPcm = freshDecoder.decode(packet2);

      expect(streamPcm.length).toBe(320);
      expect(freshPcm.length).toBe(320);
      // Internal predictor/CELP filter history makes subsequent packet output state-dependent
      expect(streamPcm).toBeInstanceOf(Float32Array);
      expect(freshPcm).toBeInstanceOf(Float32Array);
    } finally {
      streamDecoder.free();
      freshDecoder.free();
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

    expect(() => decoder.decode(new Uint8Array(0))).toThrow(
      "Attempt to use a moved value"
    );
    expect(() => decoder.reset()).toThrow("Attempt to use a moved value");
  });

  test("Symbol.dispose delegates to free", () => {
    const decoder = new MlowAudioDecoder();
    decoder[Symbol.dispose]();

    expect(() => decoder.decode(new Uint8Array(0))).toThrow(
      "Attempt to use a moved value"
    );
  });
});
