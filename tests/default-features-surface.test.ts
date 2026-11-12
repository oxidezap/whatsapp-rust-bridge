/**
 * The published default carries every optional domain.
 *
 * The `client-*` features and `legacy-session` exist so a consumer building
 * from source can subtract from the artifact (see
 * `docs/wasm-artifact-private-memory.md`). Nothing else about them is a
 * choice: `default` has all of them, and dropping one from `default` removes
 * exports from the published package. That is a shape change to argue for, not
 * a size win to slip in — and the symptom of slipping it in is a smaller
 * `.d.ts`, which no other test here notices, because the surface sweep reads
 * the method list out of that same file.
 *
 * What a pass proves: the built declarations name one export from each gated
 * domain, and the feature list this checks still matches `default` in
 * Cargo.toml. It proves nothing about what those exports do.
 *
 * Run: bun run build && bun test tests/default-features-surface.test.ts
 */

import { test, expect } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import * as ts from "../scripts/typescript-api";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

/** One export per feature, chosen because only that feature can emit it. */
const WITNESSES: Record<string, string> = {
  "client-business": "getBusinessProfile",
  "client-chat-actions": "pinChat",
  "client-contacts": "isOnWhatsApp",
  "client-groups": "getGroupMetadata",
  "client-media": "downloadMedia",
  "client-newsletter": "newsletterFollowerMute",
  "client-signal": "signalDecryptGroupMessage",
  "legacy-session": "importLegacySessionRecordV1",
};

/** The `default = [...]` list, in declaration order. */
function defaultFeatures(): string[] {
  const manifest = readFileSync(join(ROOT, "Cargo.toml"), "utf8");
  const list = manifest.match(/^default = \[([^\]]*)\]/m);
  if (!list) throw new Error("Cargo.toml has no `default = [...]`");
  return [...list[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

test("every gated domain is in the default feature set", () => {
  // Both directions: a feature added to `default` without a witness here would
  // otherwise go unpinned, and a witness whose feature left `default` is the
  // regression this file exists for.
  expect(defaultFeatures().sort()).toEqual(Object.keys(WITNESSES).sort());
});

test("the built declarations carry every gated domain's exports", async () => {
  const dts = readFileSync(join(ROOT, "dist", "whatsapp_rust_bridge.d.ts"), "utf8");
  const source = await ts.createSourceFile("surface.d.ts", dts);
  const declared = new Set<string>();
  for (const declaration of source.statements) {
    if (ts.isFunctionDeclaration(declaration) && declaration.name) declared.add(declaration.name.text);
    if ((ts.isClassDeclaration(declaration) || ts.isInterfaceDeclaration(declaration)) &&
        declaration.name?.text === "WasmWhatsAppClient") {
      for (const member of declaration.members) {
        if ((ts.isMethodDeclaration(member) || ts.isMethodSignatureDeclaration(member)) && ts.isIdentifier(member.name)) {
          declared.add(member.name.text);
        }
      }
    }
  }
  const missing = Object.entries(WITNESSES)
    .filter(([, exported]) => !declared.has(exported))
    .map(([feature, exported]) => `${feature} (${exported})`);
  expect(missing).toEqual([]);
});
