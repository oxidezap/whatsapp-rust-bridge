import { expect, test } from "bun:test";
import { createWhatsAppClient } from "../dist/index.js";

test("static media host configuration rejects malformed JS before constructing adapters", async () => {
  for (const hosts of [true, "cdn.example", {}, [42], [undefined], new Array(1)]) {
    await expect(createWhatsAppClient(
      {} as never, {} as never, null, null, null, null, null, null,
      { additionalStaticMediaHosts: hosts } as never,
    )).rejects.toMatchObject({ kind: "invalid-argument", field: "policies" });
  }
  const revoked = Proxy.revocable([], {});
  revoked.revoke();
  for (const hosts of [
    new Proxy([], { get() { throw new Error("length"); } }),
    new Proxy([], { get: (value, key) => key === "length" ? NaN : Reflect.get(value, key) }),
    new Proxy([], { get: (value, key) => key === "length" ? 0.5 : Reflect.get(value, key) }),
    revoked.proxy,
  ]) {
    await expect(createWhatsAppClient(
      {} as never, {} as never, null, null, null, null, null, null,
      { additionalStaticMediaHosts: hosts },
    )).rejects.toMatchObject({ kind: "invalid-argument", field: "policies" });
  }
  const revokedPolicies = Proxy.revocable({}, {});
  revokedPolicies.revoke();
  await expect(createWhatsAppClient(
    {} as never, {} as never, null, null, null, null, null, null, revokedPolicies.proxy,
  )).rejects.toMatchObject({ kind: "invalid-argument", field: "policies" });
  const policies = { get additionalStaticMediaHosts(): string[] { throw new Error("getter"); } };
  await expect(createWhatsAppClient(
    {} as never, {} as never, null, null, null, null, null, null, policies,
  )).rejects.toMatchObject({ kind: "invalid-argument", field: "policies" });
});

test("static media authorities are optional and accept readonly consumer arrays", async () => {
  const hosts = ["cdn.example:8443", "[::1]:8443"] as const;
  const client = await createWhatsAppClient(
    { connect() {}, send() {}, disconnect() {} },
    { async execute() { throw new Error("unexpected HTTP"); } },
    null, null, null, null, null, null, { additionalStaticMediaHosts: hosts },
  );
  await client.disconnect();
  client.free();
});
