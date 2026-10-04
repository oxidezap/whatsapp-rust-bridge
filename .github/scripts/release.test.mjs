import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { integrity, request, resolveRelease, validatePackage, alreadyPublished,
  assertNotOlder, publishPackage, finalizeRelease, assertRecoveryInputs } from "./release.mjs";

const sha = "a".repeat(40);
const release = { id: 123, tag_name: "v0.25.0", target_commitish: sha, draft: true, prerelease: false };
const event = { version: "0.25.0", sha, event: "push", refType: "branch", refName: "main" };
const bytes = Buffer.from("the verified tarball");
const metadata = { name: "@oxidezap/whatsapp-rust-bridge", version: "0.25.0", sha,
  filename: "release-package.tgz", integrity: integrity(bytes) };
const published = { name: metadata.name, version: metadata.version, dist: { integrity: metadata.integrity } };
const candidate = { id: release.id, tag: release.tag_name, sha, tagSha: sha };

function api(overrides = {}, calls = []) {
  const responses = {
    "releases?per_page=100&page=1": [release],
    [`compare/main...${sha}`]: { status: "identical" },
    "git/ref/tags/v0.25.0": { object: { type: "commit", sha } },
    "releases/123": release,
    ...overrides,
  };
  return async (path, options = {}) => {
    calls.push({ path, ...options });
    assert.ok(Object.hasOwn(responses, path), `unexpected GitHub API request ${path}`);
    if (options.method === "PATCH") return { ...responses[path], ...JSON.parse(options.body) };
    return responses[path];
  };
}

test("a rerun resolves the existing draft without release-please outputs", async () => {
  assert.deepEqual(await resolveRelease(api(), event), candidate);
});
test("ordinary pushes without a release do not publish", async () => {
  assert.equal(await resolveRelease(api({ "releases?per_page=100&page=1": [] }), event), null);
  assert.equal(await resolveRelease(api({ "releases?per_page=100&page=1": [{ ...release, target_commitish: "b".repeat(40) }] }), event), null);
  assert.equal(await resolveRelease(api({ "releases?per_page=100&page=1": [{ ...release, draft: false }] }), event), null);
});
test("draft lookup follows pagination", async () => {
  assert.deepEqual(await resolveRelease(api({
    "releases?per_page=100&page=1": Array.from({ length: 100 }, (_, id) => ({ tag_name: `other-${id}` })),
    "releases?per_page=100&page=2": [release],
  }), event), candidate);
});
test("duplicate release records fail closed", async () => {
  await assert.rejects(resolveRelease(api({ "releases?per_page=100&page=1": [release, release] }), event), /Expected one release/);
});
test("dispatch accepts only its own release tag and exact commit", async () => {
  const dispatch = { ...event, event: "workflow_dispatch", refType: "tag", refName: release.tag_name };
  assert.deepEqual(await resolveRelease(api(), dispatch), candidate);
  await assert.rejects(resolveRelease(api(), { ...dispatch, refType: "branch" }), /Dispatch/);
  await assert.rejects(resolveRelease(api(), { ...dispatch, refName: "v0.24.1" }), /Dispatch/);
  await assert.rejects(resolveRelease(api({ "releases?per_page=100&page=1": [] }), dispatch), /Expected one release/);
  await assert.rejects(resolveRelease(api({ "releases?per_page=100&page=1": [{ ...release, target_commitish: "main" }] }), dispatch), /does not target/);
});
test("unreachable commits and moved tags cannot publish", async () => {
  await assert.rejects(resolveRelease(api({ [`compare/main...${sha}`]: { status: "ahead" } }), event), /not reachable/);
  await assert.rejects(resolveRelease(api({ "git/ref/tags/v0.25.0": { object: { type: "commit", sha: "b".repeat(40) } } }), event), /does not point/);
});
test("annotated tags are peeled to the event commit", async () => {
  assert.deepEqual(await resolveRelease(api({
    "git/ref/tags/v0.25.0": { object: { type: "tag", sha: "tag-object" } },
    "git/tags/tag-object": { object: { type: "commit", sha } },
  }), event), candidate);
});
test("main recovery keeps the old tag and attests the real event commit", async () => {
  const oldSha = "b".repeat(40);
  const recovered = await resolveRelease(api({
    "releases?per_page=100&page=1": [{ ...release, target_commitish: oldSha, draft: false }],
    "git/ref/tags/v0.25.0": { object: { type: "commit", sha: oldSha } },
    [`compare/${oldSha}...${sha}`]: {
      status: "ahead", merge_base_commit: { sha: oldSha },
      files: [{ filename: "tests/newsletter-surface.test.ts", status: "modified" }],
    },
  }), { ...event, event: "workflow_dispatch", recoveryTag: "v0.25.0" });
  assert.deepEqual(recovered, { ...candidate, tagSha: oldSha });
});
test("recovery rejects branches, versions and events outside its explicit scope", async () => {
  for (const bad of [
    { event: "push" }, { refName: "feature" }, { refType: "tag" }, { recoveryTag: "v0.24.1" },
  ]) {
    await assert.rejects(resolveRelease(api(), {
      ...event, event: "workflow_dispatch", recoveryTag: "v0.25.0", ...bad,
    }), /Recovery must run/);
  }
});
test("recovery proves package and build input equivalence with a complete diff", async () => {
  const original = "b".repeat(40);
  const comparison = { status: "ahead", merge_base_commit: { sha: original }, files: [] };
  for (const change of [
    { files: [{ filename: "Cargo.lock", status: "modified" }] },
    { files: [{ filename: "tests/newsletter-surface.test.ts", status: "removed" }] },
    { files: [{ filename: "package.json", status: "modified" }] },
    { files: [{ filename: "README.md", status: "modified" }] },
    { files: [{ filename: "src/lib.rs", status: "modified" }] },
    { files: [{ filename: "scripts/build-shared-entrypoints.ts", status: "modified" }] },
    { files: [{ filename: "tests/newsletter-surface.test.ts", status: "renamed", previous_filename: "src/lib.rs" }] },
    { files: Array(300).fill({ filename: "tests/newsletter-surface.test.ts", status: "modified" }) },
    { files: undefined }, { status: "diverged" }, { merge_base_commit: { sha: "other" } },
  ]) {
    await assert.rejects(assertRecoveryInputs(async () => ({ ...comparison, ...change }), original, sha));
  }
  await assertRecoveryInputs(async () => ({ ...comparison,
    files: [{ filename: ".github/scripts/release.mjs", status: "added" }],
  }), original, sha);
});
test("archive identity includes name, version, source SHA and bytes", () => {
  validatePackage(metadata, bytes, metadata, sha);
  for (const key of ["name", "version", "sha", "filename", "integrity"]) {
    assert.throws(() => validatePackage({ ...metadata, [key]: "wrong" }, bytes, metadata, sha));
  }
  assert.throws(() => validatePackage(metadata, Buffer.from("other build"), metadata, sha), /integrity/);
});
test("an existing version must match exact SHA512, not just name and version", () => {
  assert.equal(alreadyPublished(null, metadata), false);
  assert.equal(alreadyPublished(published, metadata), true);
  for (const other of [{ ...published, version: "0.24.1" }, { ...published, name: "other" },
    { ...published, dist: {} }, { ...published, dist: { integrity: "other-build" } }]) {
    assert.throws(() => alreadyPublished(other, metadata), /different bytes/);
  }
});
test("registry outages are not treated as absent versions", async () => {
  assert.equal(await request("https://example.test", {}, async () => ({ status: 404 })), null);
  for (const status of [401, 403, 429, 500]) {
    await assert.rejects(request("https://example.test", {}, async () => ({ status, ok: false })), /HTTP/);
  }
  await assert.rejects(request("https://example.test", {}, async () => ({ status: 200, ok: true, json() { throw new Error("bad JSON"); } })), /bad JSON/);
});
test("version ordering is numeric and fails closed on an unexpected latest", () => {
  assertNotOlder("0.25.0", "0.9.0");
  assertNotOlder("0.25.0", "0.25.0");
  assert.throws(() => assertNotOlder("0.25.0", "0.26.0"), /newer/);
  assert.throws(() => assertNotOlder("0.25.0", "unexpected"), /Unexpected/);
});
test("identical version retries neither publish nor change latest", async () => {
  await publishPackage(metadata, {
    getVersion: async () => published,
    getLatest() { throw new Error("must not inspect latest for existing bytes"); },
    publish() { throw new Error("must not republish"); },
  });
});
test("accepted publish with a lost response recovers by exact registry integrity", async () => {
  let reads = 0, publishes = 0;
  await publishPackage(metadata, {
    getVersion: async () => ++reads > 2 ? published : null,
    getLatest: async () => ({ version: "0.24.1" }),
    publish() { publishes++; throw new Error("connection lost"); }, sleep: async () => {},
  });
  assert.equal(publishes, 1);
});
test("accepted uploads may need minutes before npm exposes their integrity", async () => {
  let reads = 0, publishes = 0, waited = 0;
  await publishPackage(metadata, {
    getVersion: async () => ++reads >= 15 ? published : null,
    getLatest: async () => ({ version: "0.24.1" }),
    publish: async () => { publishes++; }, sleep: async (ms) => { waited += ms; },
  });
  assert.equal(publishes, 1);
  assert.equal(waited, 130_000);
});
test("an absent version never becomes a successful publish", async () => {
  await assert.rejects(publishPackage(metadata, {
    getVersion: async () => null, getLatest: async () => ({ version: "0.24.1" }),
    publish: async () => {}, sleep: async () => {},
  }), /did not expose/);
});
test("a publish race with different registry bytes fails", async () => {
  let reads = 0;
  await assert.rejects(publishPackage(metadata, {
    getVersion: async () => ++reads === 1 ? null : { ...published, dist: { integrity: "other" } },
    getLatest: async () => ({ version: "0.24.1" }), publish: async () => {}, sleep: async () => {},
  }), /different bytes/);
});
test("a newer npm latest blocks a new older publish", async () => {
  await assert.rejects(publishPackage(metadata, {
    getVersion: async () => null, getLatest: async () => ({ version: "0.26.0" }),
    publish() { assert.fail("must not publish"); },
  }), /newer/);
});
test("GitHub publication follows confirmed npm bytes", async () => {
  const calls = [];
  await finalizeRelease(api({}, calls), candidate, metadata, async () => published, async () => published);
  assert.deepEqual(JSON.parse(calls.find((call) => call.method === "PATCH").body), { draft: false, make_latest: "true" });
});
test("failed or mismatching npm publication never makes a draft public", async () => {
  for (const value of [null, { ...published, dist: { integrity: "different" } }]) {
    const calls = [];
    await assert.rejects(finalizeRelease(api({}, calls), candidate, metadata, async () => value, async () => published));
    assert.equal(calls.filter((call) => call.method === "PATCH").length, 0);
  }
});
test("finalization retry does not displace a newer latest release", async () => {
  const calls = [];
  await finalizeRelease(api({}, calls), candidate, metadata, async () => published, async () => ({ version: "0.26.0" }));
  assert.equal(JSON.parse(calls.find((call) => call.method === "PATCH").body).make_latest, "false");
});
test("missing or still-draft PATCH responses cannot claim GitHub publication", async () => {
  for (const result of [null, release]) {
    const github = api();
    await assert.rejects(finalizeRelease(
      (path, options) => options?.method === "PATCH" ? result : github(path, options),
      candidate, metadata, async () => published, async () => published,
    ), /did not confirm/);
  }
});
test("already public finalization is idempotent", async () => {
  const calls = [];
  await finalizeRelease(api({ "releases/123": { ...release, draft: false } }, calls), candidate, metadata, async () => published, async () => published);
  assert.equal(calls.filter((call) => call.method === "PATCH").length, 0);
});
test("moved tag or edited draft blocks finalization", async () => {
  for (const overrides of [
    { "git/ref/tags/v0.25.0": null },
    { "releases/123": { ...release, target_commitish: "other" } },
    { "releases/123": { ...release, prerelease: true } },
  ]) {
    const calls = [];
    await assert.rejects(finalizeRelease(api(overrides, calls), candidate, metadata, async () => published, async () => published));
    assert.equal(calls.filter((call) => call.method === "PATCH").length, 0);
  }
});
test("workflow gates and config preserve draft -> verify -> npm -> public ordering", () => {
  const workflow = readFileSync(new URL("../workflows/release.yml", import.meta.url), "utf8");
  const config = JSON.parse(readFileSync(new URL("../../release-please-config.json", import.meta.url), "utf8"));
  assert.equal(config.packages["."].draft, true);
  assert.equal(config.packages["."]["force-tag-creation"], true);
  assert.match(workflow, /needs: \[release-metadata, verify, publish\]/);
  assert.match(workflow, /needs\.publish\.result == 'success'/);
  assert.match(workflow, /npm-tarball-\$\{\{ github\.run_id \}\}-\$\{\{ github\.run_attempt \}\}/);
  const finalize = workflow.split("\n  finalize:")[1];
  assert.ok(finalize);
  assert.doesNotMatch(finalize, /id-token: write|release\.mjs publish/);
  assert.match(finalize, /release\.mjs finalize/);
});
