// Read-only, snapshot-bound relationship lookup on already validated Code IR.
// Search discovers a unit; this layer supplies evidence sites separately from
// FTS/vector text. Shadow facts remain experimental semantic candidates.
import { createHash } from "node:crypto";
import { lstat, readFile, realpath } from "node:fs/promises";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

const decoder = new TextDecoder("utf-8", { fatal: true });

function sha256(raw) {
  return createHash("sha256").update(raw).digest("hex");
}

function hexDigest(value, field) {
  if (typeof value !== "string" || !/^[a-f0-9]{64}$/u.test(value))
    throw new Error(`missing/invalid ${field}`);
  return value;
}

function safeRelativePath(path) {
  if (typeof path !== "string" || !path || isAbsolute(path) || path.includes("\\") ||
      path.split("/").some((part) => !part || part === "." || part === ".."))
    throw new Error(`unsafe selected source path: ${String(path)}`);
  return path;
}

async function regularFileUnder(root, relativePath) {
  const relativeName = safeRelativePath(relativePath);
  const path = resolve(root, relativeName);
  const actual = await realpath(path);
  const fromRoot = relative(root, actual);
  const info = await lstat(path);
  if (fromRoot === ".." || fromRoot.startsWith(`..${sep}`) || isAbsolute(fromRoot) ||
      !info.isFile() || info.isSymbolicLink())
    throw new Error(`selected source escaped attested root: ${relativeName}`);
  return readFile(path);
}

/** Identical to scripts/scip_spike.py::digest; paths + source hashes, length-framed. */
export function sourceSetDigest(files) {
  const digest = createHash("sha256");
  for (const [name, content] of [...files].sort(([a], [b]) => a.localeCompare(b, "en"))) {
    const path = Buffer.from(safeRelativePath(name), "utf8");
    for (const field of [path, createHash("sha256").update(content).digest()]) {
      const length = Buffer.alloc(8);
      length.writeBigUInt64BE(BigInt(field.length));
      digest.update(length);
      digest.update(field);
    }
  }
  return digest.digest("hex");
}

function sortFacts(left, right, paths) {
  return paths.get(left.site.file_id).relative_path.localeCompare(paths.get(right.site.file_id).relative_path, "en") ||
    left.site.start_byte - right.site.start_byte || left.id.localeCompare(right.id, "en");
}

export async function openShadowRelationships({
  root, manifestPath, shadowPath, indexPath, configRelativePath,
  expectedShadowSha256, expectedSourceSetSha256, expectedConfigSha256,
  modulePath, expectedModuleSha256, expectedImplementationSha256,
  producerBinaryPath, expectedBinarySha256,
}) {
  const canonicalRoot = await realpath(root);
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const paths = [...new Set(manifest.units?.map((unit) => safeRelativePath(unit.path)) ?? [])].sort();
  if (!manifest.fixture_id || !manifest.language || !paths.length)
    throw new Error("manifest lacks a selected language/file set");
  const shadowBytes = await readFile(shadowPath);
  if (sha256(shadowBytes) !== hexDigest(expectedShadowSha256, "shadow SHA-256"))
    throw new Error("shadow artifact digest mismatch");
  const shadow = JSON.parse(shadowBytes.toString("utf8"));
  const snapshot = shadow.snapshot;
  if (!snapshot || snapshot.schema !== "zvec-grep.code-ir" || snapshot.schema_version !== 1 ||
      snapshot.root_set.length !== 1 || snapshot.root_set[0] !== manifest.fixture_id ||
      snapshot.files.length !== paths.length ||
      [...snapshot.files.map((file) => file.relative_path)].sort().some((name, i) => name !== paths[i]))
    throw new Error("shadow IR and manifest selected source sets differ");
  if (snapshot.files.some((file) => file.language !== manifest.language || file.extraction.status !== "complete"))
    throw new Error("shadow IR has an incomplete or wrong-language source");

  const selected = await Promise.all(paths.map(async (name) => [name, await regularFileUnder(canonicalRoot, name)]));
  const actualSourceSetSha256 = sourceSetDigest(selected);
  if (actualSourceSetSha256 !== hexDigest(expectedSourceSetSha256, "selected source-set SHA-256"))
    throw new Error("stale selected source snapshot");
  const fileByPath = new Map(snapshot.files.map((file) => [file.relative_path, file]));
  const fileById = new Map(snapshot.files.map((file) => [file.file_id, file]));
  const rawById = new Map(selected.map(([name, raw]) => [fileByPath.get(name).file_id, raw]));
  const configBytes = await regularFileUnder(canonicalRoot, configRelativePath);
  const actualConfigSha256 = sha256(configBytes);
  if (actualConfigSha256 !== hexDigest(expectedConfigSha256, "producer config SHA-256"))
    throw new Error("stale producer configuration");
  const actualIndexSha256 = sha256(await readFile(indexPath));
  const recordedIndexSha256 = shadow.producer_artifact_sha256 ?? shadow.scip_sha256 ?? shadow.scip_artifact_sha256;
  if (actualIndexSha256 !== hexDigest(recordedIndexSha256, "shadow producer index SHA-256"))
    throw new Error("SCIP index differs from the validated shadow artifact");

  if (Boolean(producerBinaryPath) !== Boolean(expectedBinarySha256))
    throw new Error("producer binary and pinned SHA-256 must be supplied together");
  if (producerBinaryPath && sha256(await readFile(producerBinaryPath)) !== hexDigest(expectedBinarySha256, "producer binary SHA-256"))
    throw new Error("producer binary digest mismatch");

  const implementationFile = resolve(dirname(modulePath), "../extraction/code/ir.js");
  const actualModuleSha256 = sha256(await readFile(modulePath));
  if (actualModuleSha256 !== hexDigest(expectedModuleSha256, "Code IR module SHA-256"))
    throw new Error("Code IR module changed");
  const actualImplementationSha256 = sha256(await readFile(implementationFile));
  if (actualImplementationSha256 !== hexDigest(expectedImplementationSha256, "Code IR implementation SHA-256"))
    throw new Error("Code IR validator implementation changed");
  const { validateSnapshot, FRONTEND_VERSION } = await import(pathToFileURL(resolve(modulePath)).href);
  if (snapshot.frontend_versions[manifest.language] !== FRONTEND_VERSION)
    throw new Error("Code IR frontend generation mismatch");
  validateSnapshot(snapshot, rawById);

  const unitById = new Map(snapshot.units.map((unit) => [unit.id, unit]));
  const inbound = new Map();
  const outbound = new Map();
  const bySite = new Map();
  for (const fact of snapshot.facts) {
    if (fact.kind !== "references" || fact.status !== "type_resolved" ||
        !fact.object_id || fact.provenance.frontend !== "scip-shadow-v1") continue;
    const target = unitById.get(fact.object_id);
    const subject = unitById.get(fact.subject_id);
    if (!target || !subject) throw new Error("SCIP shadow fact lacks a local endpoint");
    for (const [index, key] of [[inbound, target.id], [outbound, subject.id],
                                [bySite, `${fact.site.file_id}:${fact.site.start_byte}`]]) {
      const values = index.get(key) ?? [];
      values.push(fact);
      index.set(key, values);
    }
  }
  for (const index of [inbound, outbound, bySite])
    for (const facts of index.values()) facts.sort((a, b) => sortFacts(a, b, fileById));

  const provenance = {
    schema: "code-ir-scip-shadow-relationships-v1",
    status: "validated_shadow_candidate",
    repository_id: snapshot.repository_id,
    snapshot_id: snapshot.snapshot_id,
    root: canonicalRoot,
    language: manifest.language,
    selected_files: paths.length,
    source_set_sha256: actualSourceSetSha256,
    config_sha256: actualConfigSha256,
    scip_index_sha256: actualIndexSha256,
    shadow_sha256: sha256(shadowBytes),
    producer_binary_sha256: producerBinaryPath ? expectedBinarySha256 : null,
    frontend_module_sha256: actualModuleSha256,
    frontend_implementation_sha256: actualImplementationSha256,
    verified_local_references: [...inbound.values()].reduce((count, facts) => count + facts.length, 0),
  };

  function unitDetail(unit) {
    const file = fileById.get(unit.source.file_id);
    return {
      id: unit.id,
      kind: unit.kind,
      name: unit.name ?? null,
      qualified_name: unit.qualified_name ?? null,
      path: file.relative_path,
      start_byte: unit.source.start_byte,
      end_byte: unit.source.end_byte,
      line: unit.source.start.line,
    };
  }
  function factDetail(fact) {
    const site = fileById.get(fact.site.file_id);
    const raw = rawById.get(fact.site.file_id);
    return {
      kind: "references", // A reference alone never proves a call or data flow.
      site: {
        path: site.relative_path,
        start_byte: fact.site.start_byte,
        end_byte: fact.site.end_byte,
        line: fact.site.start.line,
        column_byte: fact.site.start.column_byte,
        source: decoder.decode(raw.subarray(fact.site.start_byte, fact.site.end_byte)),
      },
      subject: unitDetail(unitById.get(fact.subject_id)),
      target: unitDetail(unitById.get(fact.object_id)),
      provenance: fact.provenance,
    };
  }
  function queryUnit({ unitId, path, symbol, limit = 50 }) {
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 500)
      throw new Error("relationship query limit must be 1..500");
    const unit = unitId ? unitById.get(unitId) : (() => {
      const file = fileByPath.get(safeRelativePath(path));
      if (!file || !symbol) throw new Error("select a known unit ID or a path and symbol");
      const matches = snapshot.units.filter((candidate) =>
        candidate.source.file_id === file.file_id &&
        (candidate.qualified_name === symbol || candidate.name === symbol));
      if (matches.length > 1) throw new Error("ambiguous source symbol; select an exact Code IR unit ID");
      return matches[0];
    })();
    if (!unit) throw new Error("unknown Code IR unit in selected snapshot");
    const incoming = inbound.get(unit.id) ?? [];
    const outgoing = outbound.get(unit.id) ?? [];
    return { ...provenance, unit: unitDetail(unit), incoming_total: incoming.length,
             incoming: incoming.slice(0, limit).map(factDetail),
             outgoing_total: outgoing.length,
             outgoing: outgoing.slice(0, limit).map(factDetail),
             truncated: incoming.length > limit || outgoing.length > limit };
  }
  function querySite({ path, startByte, endByte }) {
    const file = fileByPath.get(safeRelativePath(path));
    if (!file) throw new Error("site outside selected source set");
    const raw = rawById.get(file.file_id);
    if (!Number.isSafeInteger(startByte) || startByte < 0 || startByte >= raw.length || (raw[startByte] & 0xc0) === 0x80)
      throw new Error("invalid reference site byte offset");
    if (endByte !== undefined && (!Number.isSafeInteger(endByte) || endByte <= startByte ||
        endByte > raw.length || (endByte < raw.length && (raw[endByte] & 0xc0) === 0x80)))
      throw new Error("invalid reference site end byte offset");
    const facts = (bySite.get(`${file.file_id}:${startByte}`) ?? [])
      .filter((fact) => endByte === undefined || fact.site.end_byte === endByte);
    if (facts.length > 1) throw new Error("ambiguous relationship at source site; abstaining");
    return { ...provenance, site: { path, start_byte: startByte, ...(endByte === undefined ? {} : { end_byte: endByte }) },
             binding: facts[0] ? factDetail(facts[0]) : null };
  }
  return { provenance, queryUnit, querySite };
}
