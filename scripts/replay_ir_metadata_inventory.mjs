// Independently authored source labels, never metadata derived from the IR.
// Usage: node scripts/replay_ir_metadata_inventory.mjs --fixture DIR --labels FILE
import { createHash } from "node:crypto";
import { lstat, readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const safePath = (path) =>
  typeof path === "string" &&
  path.length > 0 &&
  !path.startsWith("/") &&
  !path.includes("\\") &&
  path.split("/").every((part) => part && part !== "." && part !== "..");

function authoredSpan(bytes, text, context) {
  if (typeof text !== "string" || !text)
    throw new Error(`empty authored ${context}`);
  const match = Buffer.from(text, "utf8");
  const start = Buffer.from(bytes).indexOf(match);
  if (start < 0 || Buffer.from(bytes).indexOf(match, start + 1) >= 0)
    throw new Error(`authored ${context} is missing or not unique in source`);
  return [start, start + match.length];
}

export async function replayMetadataInventory({
  inventory,
  manifestBytes,
  readSource,
  extractSnapshot,
}) {
  if (
    inventory?.schema_version !== 1 ||
    !Array.isArray(inventory.cases) ||
    inventory.cases.length === 0 ||
    inventory.manifest_sha256 !== sha256(manifestBytes)
  )
    throw new Error("metadata inventory or fixture manifest is stale/invalid");
  const manifest = JSON.parse(Buffer.from(manifestBytes).toString("utf8"));
  if (
    manifest.fixture_id !== inventory.fixture_id ||
    manifest.language !== inventory.language ||
    !manifest.source?.repository ||
    !Array.isArray(manifest.units)
  )
    throw new Error("metadata inventory fixture manifest identity mismatch");
  const paths = [...new Set(manifest.units.map((unit) => unit.path))].sort();
  if (!paths.length || paths.some((path) => !safePath(path)))
    throw new Error("unsafe metadata inventory source path");
  const bytesByPath = new Map();
  const inputs = [];
  for (const path of paths) {
    const bytes = await readSource(path);
    if (!(bytes instanceof Uint8Array))
      throw new Error(`missing inventory source: ${path}`);
    bytesByPath.set(path, bytes);
    inputs.push({
      root_id: manifest.fixture_id,
      relative_path: path,
      language: manifest.language,
      bytes,
    });
  }
  const sourceSet = paths.map((path) => [path, sha256(bytesByPath.get(path))]);
  if (
    sha256(Buffer.from(JSON.stringify(sourceSet))) !==
    inventory.source_set_sha256
  )
    throw new Error("stale metadata inventory source set");
  const selected = new Set();
  const expected = [];
  for (const entry of inventory.cases) {
    if (
      !safePath(entry.path) ||
      !bytesByPath.has(entry.path) ||
      sha256(bytesByPath.get(entry.path)) !== entry.file_sha256 ||
      typeof entry.name !== "string" ||
      !entry.name ||
      !["type", "function", "method", "value"].includes(entry.kind) ||
      !Object.hasOwn(entry, "signature") ||
      !Object.hasOwn(entry, "documentation")
    )
      throw new Error("invalid inventory case/source hash");
    const key = JSON.stringify([entry.path, entry.name, entry.kind]);
    if (selected.has(key)) throw new Error("duplicate inventory case");
    selected.add(key);
    const bytes = bytesByPath.get(entry.path);
    const hasUnsupportedSignature = Object.hasOwn(
      entry,
      "unsupported_signature",
    );
    if (entry.kind === "value") {
      if (entry.signature !== null || hasUnsupportedSignature)
        throw new Error(
          "value unit cannot have an authored declaration signature",
        );
    } else if ((entry.signature === null) !== hasUnsupportedSignature) {
      throw new Error("missing or conflicting signature label");
    }
    if (
      entry.documentation !== null &&
      entry.unsupported_documentation !== undefined
    )
      throw new Error("conflicting documentation labels");
    const signature =
      entry.signature === null
        ? null
        : authoredSpan(bytes, entry.signature, "signature");
    const unsupportedSignature =
      entry.unsupported_signature === undefined
        ? null
        : authoredSpan(
            bytes,
            entry.unsupported_signature,
            "unsupported signature",
          );
    const unitAnchor =
      entry.unit_anchor === undefined
        ? null
        : authoredSpan(bytes, entry.unit_anchor, "unit anchor");
    const documentation =
      entry.documentation === null
        ? null
        : authoredSpan(bytes, entry.documentation, "documentation");
    const unsupported =
      entry.unsupported_documentation === undefined
        ? null
        : authoredSpan(
            bytes,
            entry.unsupported_documentation,
            "unsupported documentation",
          );
    expected.push({
      entry,
      signature,
      unsupportedSignature,
      unitAnchor,
      documentation,
      unsupported,
    });
  }
  const { snapshot, sources } = await extractSnapshot(
    manifest.source.repository,
    inputs,
  );
  if (sources.size !== inputs.length || snapshot.files.length !== inputs.length)
    throw new Error("IR snapshot omitted selected source files");
  const files = new Map(
    snapshot.files.map((file) => [file.relative_path, file]),
  );
  const result = {
    language: inventory.language,
    selected_cases: inventory.cases.length,
    matched_units: 0,
    missing_units: 0,
    signature_expected: 0,
    signature_matches: 0,
    signature_absences: 0,
    unsupported_signatures: 0,
    documentation_expected: 0,
    documentation_matches: 0,
    documented_absences: 0,
    unsupported_documentation: 0,
  };
  for (const {
    entry,
    signature,
    unsupportedSignature,
    unitAnchor,
    documentation,
    unsupported,
  } of expected) {
    if (signature || unsupportedSignature) result.signature_expected++;
    if (documentation || unsupported) result.documentation_expected++;
    const file = files.get(entry.path);
    if (!file || file.sha256 !== entry.file_sha256)
      throw new Error(`stale IR file: ${entry.path}`);
    const selectedSpan = signature ?? unsupportedSignature ?? unitAnchor;
    const units = snapshot.units.filter(
      (unit) =>
        unit.source.file_id === file.file_id &&
        unit.name === entry.name &&
        unit.kind === entry.kind &&
        (!selectedSpan ||
          (unit.source.start_byte <= selectedSpan[0] &&
            unit.source.end_byte >= selectedSpan[1])),
    );
    if (!units.length && selectedSpan) {
      result.missing_units++;
      continue;
    }
    if (units.length !== 1)
      throw new Error(
        `ambiguous or unanchored IR unit: ${entry.path}::${entry.name}`,
      );
    result.matched_units++;
    const unit = units[0];
    if (
      unit.source.sha256 !== file.sha256 ||
      !Number.isSafeInteger(unit.source.start_byte) ||
      !Number.isSafeInteger(unit.source.end_byte) ||
      unit.source.start_byte < 0 ||
      unit.source.end_byte <= unit.source.start_byte ||
      unit.source.end_byte > bytesByPath.get(entry.path).length
    )
      throw new Error(`stale IR unit source: ${entry.path}::${entry.name}`);
    const matches = (anchor, span, text) =>
      anchor?.source.file_id === file.file_id &&
      anchor.source.sha256 === file.sha256 &&
      anchor.source.start_byte === span[0] &&
      anchor.source.end_byte === span[1] &&
      anchor.text === text;
    if (signature) {
      if (!matches(unit.signature, signature, entry.signature))
        throw new Error(
          `IR signature differs from authored source: ${entry.path}::${entry.name}`,
        );
      result.signature_matches++;
    } else if (unit.signature) {
      throw new Error(`unexpected IR signature: ${entry.path}::${entry.name}`);
    } else if (unsupportedSignature) {
      result.unsupported_signatures++;
    } else {
      result.signature_absences++;
    }
    if (documentation) {
      if (!matches(unit.documentation, documentation, entry.documentation))
        throw new Error(
          `IR documentation differs from authored source: ${entry.path}::${entry.name}`,
        );
      result.documentation_matches++;
    } else if (unit.documentation) {
      throw new Error(
        `unexpected IR documentation: ${entry.path}::${entry.name}`,
      );
    } else if (unsupported) {
      result.unsupported_documentation++;
    } else {
      result.documented_absences++;
    }
  }
  return result;
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const value = (option) => {
    const index = process.argv.indexOf(option);
    return index < 0 ? undefined : process.argv[index + 1];
  };
  const fixture = value("--fixture");
  const labels = value("--labels");
  if (!fixture || !labels)
    throw new Error("Usage: --fixture DIR --labels FILE");
  const root = resolve(fixture);
  const manifestBytes = await readFile(resolve(root, "manifest.json"));
  const manifest = JSON.parse(manifestBytes.toString("utf8"));
  const inventories = JSON.parse(await readFile(labels, "utf8"));
  const inventory = inventories.lanes?.find(
    (lane) =>
      lane.fixture_id === manifest.fixture_id &&
      lane.language === manifest.language,
  );
  if (!inventory) throw new Error("no inventory for fixture");
  const { extractSnapshot } = await import("../dist/engine/code-ir/index.js");
  const result = await replayMetadataInventory({
    inventory,
    manifestBytes,
    readSource: async (path) => {
      const file = resolve(root, path);
      if (dirname(file) !== root && !file.startsWith(`${root}/`))
        throw new Error("inventory path escapes fixture root");
      const stat = await lstat(file);
      if (!stat.isFile() || stat.isSymbolicLink())
        throw new Error("inventory source is not a regular file");
      return readFile(file);
    },
    extractSnapshot,
  });
  console.log(JSON.stringify(result, null, 2));
}
