/**
 * Consuming the package's opt-in VoIP subpaths exercises both actual WASMs.
 * No server is present: a real media call and its frame callbacks cannot be
 * established here. The test proves package resolution, module init, OZVP
 * handshake, core construction with the returned plugin, and codec exports.
 */
import { describe, expect, test } from "bun:test";
import { createWhatsAppClient, initWasmEngine } from "@oxidezap/whatsapp-rust-bridge";
import {
  loadVoip,
  MlowAudioDecoder,
  packetizeOpusForMlow,
  depacketizeOpusFromMlow,
} from "@oxidezap/whatsapp-rust-bridge/voip";
import { initVoipSync } from "@oxidezap/whatsapp-rust-bridge/voip/host";
import { readFileSync } from "node:fs";
import { createHttp } from "./helpers.js";

const noRelay = { connect: async () => { throw new Error("offline"); } };

describe("published VoIP facade", () => {
  test("loads the separate engine and installs its handshake on a real client", async () => {
    initWasmEngine();
    const { voipBackend } = loadVoip(noRelay);
    const client = await createWhatsAppClient(
      { connect() {}, send() {}, disconnect() {} },
      createHttp(), null, null, null, null, null, null, null,
      { voipBackend },
    );
    try {
      expect(typeof client.acceptCall).toBe("function");
      expect(typeof client.dialCall).toBe("function");
      expect(typeof client.acceptCallPcm).toBe("function");
      expect(typeof client.dialCallPcm).toBe("function");
      expect(typeof client.callPushPcm16).toBe("function");
      expect(typeof client.callPushAudio).toBe("function");
      expect(typeof client.callPushVideo).toBe("function");
      expect(typeof client.endCall).toBe("function");
      expect(typeof client.getCallMediaStats).toBe("function");
      expect(typeof client.getActiveCalls).toBe("function");
      expect(typeof client.rejectCall).toBe("function");
      expect(typeof client.terminateCall).toBe("function");
      await expect(client.acceptCall("NO-SUCH-OFFER", "mlow")).rejects.toMatchObject({
        kind: "invalid-argument", field: "callId",
      });
      await expect(client.acceptCallPcm("NO-SUCH-OFFER")).rejects.toMatchObject({
        kind: "invalid-argument", field: "callId",
      });
      await expect(client.dialCallPcm("not-a-jid")).rejects.toMatchObject({
        kind: "invalid-argument", field: "peer",
      });
      expect(() => client.callPushPcm16("NO-SUCH-CALL", new Int16Array(960)))
        .toThrowError(expect.objectContaining({ kind: "invalid-argument", field: "callId" }));
    } finally {
      client.free();
    }
  });

  test("host init accepts provided bytes without loading the core entrypoint", async () => {
    const bytes = readFileSync(new URL("../dist/whatsapp_rust_voip_bg.wasm", import.meta.url));
    const engine = initVoipSync(bytes, noRelay);
    const hello = new Uint8Array([79, 90, 86, 80, 1, 0, 1, 0, 0, 5, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
    const response = await engine.voipBackend.sendFrame(hello);
    expect(response.slice(0, 7)).toEqual(hello.slice(0, 7));
    expect(response[7]).toBe(1); // RESPONSE
    expect(response[13]! & 0x01).toBe(0x01); // PCM capability
  });

  test("codec helpers and decoder are live in voip.wasm", () => {
    loadVoip(noRelay);
    const original = new Uint8Array([0xbb, 3, 1, 2]);
    const escaped = packetizeOpusForMlow(original);
    expect(escaped[0]).toBe(0xdd);
    expect(depacketizeOpusFromMlow(escaped)).toEqual(original);
    const decoder = new MlowAudioDecoder();
    expect(decoder.decode(new Uint8Array(), undefined).length).toBe(960);
    decoder.free();
  });
});
