import { createHash } from "node:crypto";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const registry = "https://registry.npmjs.org";
export const integrity = (bytes) => `sha512-${createHash("sha512").update(bytes).digest("base64")}`;

export async function request(url, options = {}, fetcher = fetch) {
  const response = await fetcher(url, options);
  if (response.status === 404) return null;
  if (!response.ok) {
    const error = new Error(`${options.method || "GET"} ${url}: HTTP ${response.status}`);
    error.status = response.status;
    throw error;
  }
  return response.json();
}

export function githubClient(repository, token, fetcher = fetch) {
  return (path, options = {}) => request(`https://api.github.com/repos/${repository}/${path}`, {
    ...options,
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "X-GitHub-Api-Version": "2022-11-28",
      "Content-Type": "application/json",
    },
  }, fetcher);
}

export async function assertTag(github, tag, sha) {
  let object = (await github(`git/ref/tags/${encodeURIComponent(tag)}`))?.object;
  for (let depth = 0; object?.type === "tag" && depth < 10; depth++) {
    object = (await github(`git/tags/${object.sha}`))?.object;
  }
  if (object?.type !== "commit" || object.sha !== sha) {
    throw new Error(`${tag} does not point at event commit ${sha}`);
  }
}

const recoveryPaths = new Set([
  ".github/workflows/ci.yml",
  ".github/workflows/release.yml",
  ".github/scripts/release.mjs",
  ".github/scripts/release.test.mjs",
  "release-please-config.json",
  "tests/newsletter-surface.test.ts",
]);

export async function assertRecoveryInputs(github, tagSha, sha) {
  const comparison = await github(`compare/${tagSha}...${sha}`);
  if (!["identical", "ahead"].includes(comparison?.status) || comparison.merge_base_commit?.sha !== tagSha) {
    throw new Error("Recovery must descend from the original release commit");
  }
  // GitHub returns at most 300 changed files. At the cap, completeness cannot
  // be proved. A rename also names an old path that must not escape the gate.
  if (!Array.isArray(comparison.files) || comparison.files.length >= 300) {
    throw new Error("Recovery diff is missing or may be truncated");
  }
  for (const file of comparison.files) {
    if (!recoveryPaths.has(file.filename) || file.previous_filename ||
        !["added", "modified"].includes(file.status)) {
      throw new Error(`Recovery changes a package or build input: ${file.filename}`);
    }
  }
}

export async function resolveRelease(github, { version, sha, event, refType, refName, recoveryTag = "" }) {
  const tag = `v${version}`;
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error(`Invalid stable release version: ${version}`);
  const recovery = Boolean(recoveryTag);
  if (recovery) {
    if (event !== "workflow_dispatch" || refType !== "branch" || refName !== "main" || recoveryTag !== tag) {
      throw new Error("Recovery must run on main and name the tag matching package.json");
    }
  } else if (event === "workflow_dispatch" && (refType !== "tag" || refName !== tag)) {
    throw new Error("Dispatch must select the release tag matching package.json");
  }
  const matches = [];
  for (let page = 1; ; page++) {
    const releases = await github(`releases?per_page=100&page=${page}`);
    if (!Array.isArray(releases)) throw new Error("Could not list releases");
    matches.push(...releases.filter((release) => release.tag_name === tag));
    if (releases.length < 100) break;
  }
  if (matches.length === 0 && event === "push") return null;
  if (matches.length !== 1) throw new Error(`Expected one release for ${tag}, found ${matches.length}`);
  const release = matches[0];
  // Ordinary commits retain the last version until the next release PR merge.
  if (event === "push" && release.target_commitish !== sha) {
    if (release.draft) console.warn(`::warning::${tag} targets an earlier commit; dispatch this workflow on ${tag} to preserve provenance`);
    return null;
  }
  if (!recovery && release.target_commitish !== sha) throw new Error(`${tag} release does not target ${sha}`);
  const tagSha = release.target_commitish;
  if (!/^[a-f0-9]{40}$/.test(tagSha)) throw new Error("Release target must be an immutable commit SHA");
  if (!release.draft && event === "push") return null;
  if (release.prerelease) throw new Error("This workflow only publishes stable releases");
  const comparison = await github(`compare/main...${sha}`);
  if (!["identical", "behind"].includes(comparison?.status)) {
    throw new Error(`${sha} is not reachable from main`);
  }
  await assertTag(github, tag, tagSha);
  if (recovery) await assertRecoveryInputs(github, tagSha, sha);
  return { id: release.id, tag, sha, tagSha };
}

export function validatePackage(metadata, bytes, manifest, sha) {
  if (metadata.name !== manifest.name || metadata.version !== manifest.version || metadata.sha !== sha) {
    throw new Error("Packed package does not match the checked-out release");
  }
  if (metadata.filename !== "release-package.tgz" || metadata.integrity !== integrity(bytes)) {
    throw new Error("Packed package integrity mismatch");
  }
}

export function alreadyPublished(published, metadata) {
  if (published === null) return false;
  if (published.name !== metadata.name || published.version !== metadata.version ||
      published.dist?.integrity !== metadata.integrity) {
    throw new Error("npm already contains different bytes for this version; refusing to finalize the release");
  }
  return true;
}

export function assertNotOlder(version, latest) {
  if (!latest) return;
  if (!/^\d+\.\d+\.\d+$/.test(latest)) throw new Error(`Unexpected npm latest version: ${latest}`);
  const target = version.split(".").map(BigInt);
  const current = latest.split(".").map(BigInt);
  for (let index = 0; index < 3; index++) {
    if (target[index] > current[index]) return;
    if (target[index] < current[index]) throw new Error(`npm latest ${latest} is newer than ${version}`);
  }
}

export async function publishPackage(metadata, {
  getVersion, getLatest, publish, sleep, report = () => {},
}) {
  if (alreadyPublished(await getVersion(), metadata)) return;
  assertNotOlder(metadata.version, (await getLatest())?.version);
  let publishError;
  try { await publish(); } catch (error) { publishError = error; }
  // A failed response can still mean npm accepted the immutable version.
  // npm can accept the upload before registry processing exposes the version.
  // Keep the same archive and wait up to ten minutes within the job's budget.
  for (let attempt = 0; attempt <= 60; attempt++) {
    let published;
    try {
      published = await getVersion();
    } catch (error) {
      if (!(error instanceof TypeError) && error.status !== 429 &&
          !(error.status >= 500 && error.status < 600)) throw error;
      report(`npm visibility check will retry: ${error.message}`);
    }
    if (published !== undefined && alreadyPublished(published, metadata)) return;
    if (attempt === 0) report("Waiting up to ten minutes for npm to expose the exact tarball integrity");
    if (attempt < 60) await sleep(10_000);
  }
  throw publishError || new Error("npm did not expose the published tarball integrity; retry failed jobs to retain the verified archive");
}

export async function finalizeRelease(github, candidate, metadata, getVersion, getLatest) {
  if (!alreadyPublished(await getVersion(), metadata)) throw new Error("Package is absent from npm");
  await assertTag(github, candidate.tag, candidate.tagSha);
  const release = await github(`releases/${candidate.id}`);
  if (release?.tag_name !== candidate.tag || release.target_commitish !== candidate.tagSha || release.prerelease) {
    throw new Error("Release metadata changed after verification");
  }
  if (!release.draft) return;
  const latest = await getLatest();
  const result = await github(`releases/${candidate.id}`, {
    method: "PATCH",
    body: JSON.stringify({ draft: false, make_latest: latest?.version === metadata.version ? "true" : "false" }),
  });
  if (result?.draft !== false || result.tag_name !== candidate.tag) {
    throw new Error("GitHub did not confirm release publication");
  }
}

async function main(command) {
  const manifest = JSON.parse(readFileSync("package.json", "utf8"));
  const sha = process.env.GITHUB_SHA;
  const github = githubClient(process.env.GITHUB_REPOSITORY, process.env.GH_TOKEN);
  const output = (name, value) => appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${value}\n`);
  if (command === "resolve") {
    const candidate = await resolveRelease(github, {
      version: manifest.version, sha, event: process.env.GITHUB_EVENT_NAME,
      refType: process.env.GITHUB_REF_TYPE, refName: process.env.GITHUB_REF_NAME,
      recoveryTag: process.env.RECOVERY_TAG,
    });
    output("ready", Boolean(candidate));
    if (candidate) for (const [key, value] of Object.entries(candidate)) output(key, value);
    else console.log("No draft release for this event commit");
    return;
  }
  if (command === "pack") {
    const raw = JSON.parse(execFileSync("npm", ["pack", "--json", "--ignore-scripts"], { encoding: "utf8" }));
    const records = Array.isArray(raw) ? raw : Object.values(raw);
    if (records.length !== 1) throw new Error("Expected exactly one npm tarball");
    const bytes = readFileSync(records[0].filename);
    const metadata = { name: manifest.name, version: manifest.version, sha, filename: "release-package.tgz", integrity: integrity(bytes) };
    writeFileSync(metadata.filename, bytes);
    writeFileSync("release-package.json", JSON.stringify(metadata));
    return;
  }
  const metadata = JSON.parse(readFileSync("release-package.json", "utf8"));
  validatePackage(metadata, readFileSync("release-package.tgz"), manifest, sha);
  const url = `${registry}/${encodeURIComponent(metadata.name)}`;
  const getVersion = () => request(`${url}/${metadata.version}`);
  const getLatest = () => request(`${url}/latest`);
  if (command === "publish") {
    await publishPackage(metadata, {
      getVersion, getLatest,
      publish: () => execFileSync("npm", ["publish", "./release-package.tgz", "--ignore-scripts", "--access", "public"], { stdio: "inherit" }),
      sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
      report: console.log,
    });
  } else if (command === "finalize") {
    await finalizeRelease(github, { id: process.env.RELEASE_ID, tag: process.env.RELEASE_TAG, sha, tagSha: process.env.RELEASE_TAG_SHA }, metadata, getVersion, getLatest);
  } else throw new Error(`Unknown release command: ${command}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main(process.argv[2]);
}
