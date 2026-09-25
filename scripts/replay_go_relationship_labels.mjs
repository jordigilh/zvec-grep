#!/usr/bin/env node
// Adjudicate this read-only relationship lookup against independently authored
// source sites, without feeding the labels into the SCIP/Code IR import.
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { openShadowRelationships } from "./shadow_ir_relationships.mjs";

const args = new Map();
for (let i = 2; i < process.argv.length; i += 2) {
  if (!process.argv[i]?.startsWith("--") || !process.argv[i + 1] || args.has(process.argv[i]))
    throw new Error(`invalid or repeated option: ${process.argv[i]}`);
  args.set(process.argv[i], process.argv[i + 1]);
}
const required = (name) => {
  if (!args.get(name)) throw new Error(`missing ${name}`);
  return args.get(name);
};
const root = required("--root");
const options = {
  root, manifestPath: required("--manifest"), shadowPath: required("--shadow"),
  indexPath: required("--scip-index"), configRelativePath: required("--config"),
  expectedShadowSha256: required("--shadow-sha256"),
  expectedSourceSetSha256: required("--source-set-sha256"),
  expectedConfigSha256: required("--config-sha256"),
  modulePath: required("--module"),
  expectedModuleSha256: required("--module-sha256"),
  expectedImplementationSha256: required("--implementation-sha256"),
  producerBinaryPath: required("--producer-binary"),
  expectedBinarySha256: required("--producer-binary-sha256"),
};
const index = await openShadowRelationships(options);
if (index.provenance.language !== "go") throw new Error("Go authored sites require Go shadow facts");
const labels = JSON.parse(await readFile(required("--labels"), "utf8"));
const negatives = JSON.parse(await readFile(required("--negatives"), "utf8"));
if (labels.fixture_id !== negatives.fixture_id || labels.fixture_id !== "go-workflow-discovery-v1" ||
    negatives.selected_source_set_sha256 !== index.provenance.source_set_sha256)
  throw new Error("authored labels do not match the attested Go file set");

const sourceCache = new Map();
async function anchor([path, line, token, nth]) {
  if (typeof path !== "string" || path.startsWith("/") || path.includes("\\") ||
      path.split("/").some((part) => !part || part === "." || part === "..") ||
      !Number.isSafeInteger(line) || line < 1 || !Number.isSafeInteger(nth) || nth < 0 ||
      !/^[A-Za-z_][A-Za-z_0-9]*$/u.test(token) || !index.provenance.selected_files)
    throw new Error("invalid authored reference site");
  if (!sourceCache.has(path)) sourceCache.set(path, await readFile(join(root, path)));
  const raw = sourceCache.get(path);
  const lines = raw.toString("utf8").split("\n");
  if (line > lines.length || !lines[line - 1].includes(token)) throw new Error(`authored token missing: ${path}:${line}`);
  const text = lines[line - 1];
  let character = -1;
  for (let n = 0; n <= nth; n++) character = text.indexOf(token, character + 1);
  if (character < 0 || /[A-Za-z_0-9]/u.test(text[character - 1] ?? "") ||
      /[A-Za-z_0-9]/u.test(text[character + token.length] ?? ""))
    throw new Error(`ambiguous authored token: ${path}:${line}`);
  const startByte = Buffer.byteLength(lines.slice(0, line - 1).join("\n") + (line > 1 ? "\n" : "") + text.slice(0, character));
  if (raw.subarray(startByte, startByte + token.length).toString("utf8") !== token)
    throw new Error(`authored byte slice differs: ${path}:${line}`);
  return { path, startByte, endByte: startByte + Buffer.byteLength(token) };
}

const positive = [];
for (const row of labels.go) {
  const site = await anchor(row.site);
  const target = await anchor(row.definition);
  const binding = index.querySite({ path: site.path, startByte: site.startByte, endByte: site.endByte }).binding;
  const correct = Boolean(binding && binding.target.path === target.path &&
    binding.target.start_byte <= target.startByte && target.endByte <= binding.target.end_byte &&
    binding.target.name === row.definition[2] && binding.site.source === row.site[2]);
  positive.push({ site: row.site, target: row.definition, found: Boolean(binding), correct,
    resolved_target: binding?.target.qualified_name ?? null });
}
const negative = [];
for (const row of negatives.go) {
  const site = await anchor(row.site);
  const binding = index.querySite({ path: site.path, startByte: site.startByte, endByte: site.endByte }).binding;
  negative.push({ site: row.site, reason: row.reason, selected_local_binding: Boolean(binding) });
}
const summary = {
  schema: "go-shadow-relationship-label-replay-v1",
  snapshot_id: index.provenance.snapshot_id,
  source_set_sha256: index.provenance.source_set_sha256,
  scip_index_sha256: index.provenance.scip_index_sha256,
  strict_local_reference_facts: index.provenance.verified_local_references,
  positive_correct: positive.filter((row) => row.correct).length,
  positive_total: positive.length,
  negative_abstentions: negative.filter((row) => !row.selected_local_binding).length,
  negative_total: negative.length,
  positive,
  negative,
};
if (args.has("--output")) await writeFile(args.get("--output"), JSON.stringify(summary, null, 2) + "\n", { flag: "wx" });
process.stdout.write(JSON.stringify({ ...summary, positive: undefined, negative: undefined }, null, 2) + "\n");
if (summary.positive_correct !== summary.positive_total || summary.negative_abstentions !== summary.negative_total)
  process.exitCode = 1;
