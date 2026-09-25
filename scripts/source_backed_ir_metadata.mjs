// Experimental Code IR retrieval metadata. The existing code extractor parses
// the SAME attested source bytes as the IR; it contributes only metadata whose
// declaration span, name and source slices can be independently checked.
// Neither SCIP symbols nor qrels are consulted here.
import { createHash } from "node:crypto";

const decoder = new TextDecoder("utf-8", { fatal: true });

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function compact(value) {
  return value.replace(/\s+/gu, " ").trim();
}

function docRegion(text, start) {
  const lineStart = text.lastIndexOf("\n", start - 1) + 1;
  const before = text.slice(0, lineStart);
  const lines = before.split("\n");
  lines.pop(); // The source line immediately before the declaration is last.
  let index = lines.length - 1;
  const comment = /^\s*(?:\/\/|\/\*|\*|\*\/|#)/u;
  while (index >= 0 && comment.test(lines[index])) index--;
  if (index === lines.length - 1) return null;
  const regionStart = lines.slice(0, index + 1).join("\n").length + (index >= 0 ? 1 : 0);
  const regionEnd = lineStart;
  const raw = text.slice(regionStart, regionEnd);
  // Clean the syntax only for verification. The actual raw comment remains the
  // evidence and the display text is never used as a source citation.
  const cleaned = compact(raw.replace(/^\s*(?:\/\/\/?|\/\*+|\*\/?|#)\s?/gmu, "").replace(/\*\//gu, ""));
  return cleaned ? { raw, cleaned, start: regionStart, end: regionEnd } : null;
}

/** Return metadata keyed by a UNIQUE, source-validated Code IR unit ID. */
export async function collectSourceBackedMetadata(snapshot, inputs, Extractor) {
  const selected = new Map(inputs.map((file) => [file.relative_path, file]));
  const files = new Map(snapshot.files.map((file) => [file.file_id, file]));
  if (selected.size !== snapshot.files.length)
    throw new Error("source-backed projection requires the complete IR file set");
  const units = new Map(snapshot.units.map((unit) => [unit.id, unit]));
  const candidates = new Map();
  const extractor = new Extractor();
  for (const irFile of snapshot.files) {
    const input = selected.get(irFile.relative_path);
    if (!input || input.language !== irFile.language || sha256(input.bytes) !== irFile.sha256)
      throw new Error(`source-backed projection has stale/missing source: ${irFile.relative_path}`);
    const text = decoder.decode(input.bytes);
    const prepared = await extractor.extractForIndexing({
      kind: "text",
      file: {
        id: irFile.file_id,
        absolutePath: `/attested/${irFile.relative_path}`,
        relativePath: irFile.relative_path,
        rootPath: "/attested",
        kind: "code",
        format: irFile.language,
        sizeBytes: input.bytes.length,
        lastModifiedTime: 0,
        contentHash: irFile.sha256,
      },
      text,
    });
    for (const { fragment } of prepared) {
      if (fragment.group && fragment.group !== fragment.id) continue;
      const metadata = fragment.metadata;
      const range = fragment.range;
      if (!metadata || metadata.kind !== "code" || !metadata.symbolName || range.kind !== "text") continue;
      const start = Buffer.byteLength(text.slice(0, range.startOffset), "utf8");
      const end = Buffer.byteLength(text.slice(0, range.endOffset), "utf8");
      if (start >= end || fragment.content.kind !== "text" ||
          decoder.decode(input.bytes.subarray(start, end)) !== fragment.content.text) continue;
      const matchingUnits = snapshot.units.filter((unit) =>
        unit.source.file_id === irFile.file_id &&
        unit.name === metadata.symbolName &&
        unit.kind !== "file" && unit.kind !== "opaque" &&
        unit.source.start_byte <= start &&
        unit.source.end_byte === end &&
        (!metadata.scope || unit.qualified_name === `${metadata.scope}::${unit.name}` ||
          unit.qualified_name?.endsWith(`::${metadata.scope}::${unit.name}`)),
      );
      if (matchingUnits.length !== 1) continue;
      const unit = matchingUnits[0];
      const value = candidates.get(unit.id) ?? [];
      value.push({ metadata, text, start, end, raw: fragment.content.text, irFile });
      candidates.set(unit.id, value);
    }
  }
  const byUnit = new Map();
  let signatures = 0;
  let docs = 0;
  let ambiguous = 0;
  for (const [unitId, matches] of candidates) {
    if (matches.length !== 1) {
      ambiguous++;
      continue;
    }
    const unit = units.get(unitId);
    const { metadata, text, start, end, raw, irFile } = matches[0];
    if (!unit || files.get(unit.source.file_id) !== irFile) throw new Error("unattested metadata unit");
    const output = {};
    const signature = compact(metadata.signature ?? "");
    // Only the node's own source is eligible. No symbol-description text or
    // language-server-rendered signature is trusted as a source citation.
    if (signature && compact(raw.slice(0, 1_200)).includes(signature)) {
      output.signature = signature.slice(0, 320);
      output.signature_source = { file_id: irFile.file_id, sha256: irFile.sha256, start_byte: start, end_byte: end };
      signatures++;
    }
    const doc = compact(metadata.doc ?? "");
    if (doc) {
      const nodeCharStart = decoder.decode(selected.get(irFile.relative_path).bytes.subarray(0, start)).length;
      const region = docRegion(text, nodeCharStart);
      if (region && region.cleaned.includes(doc) &&
          Buffer.byteLength(text.slice(0, region.end), "utf8") <= start) {
        output.doc = doc.slice(0, 320);
        output.doc_source = {
          file_id: irFile.file_id,
          sha256: irFile.sha256,
          start_byte: Buffer.byteLength(text.slice(0, region.start), "utf8"),
          end_byte: Buffer.byteLength(text.slice(0, region.end), "utf8"),
        };
        docs++;
      }
    }
    if (output.signature || output.doc) byUnit.set(unitId, output);
  }
  return { byUnit, stats: { matched_units: candidates.size - ambiguous, ambiguous_units: ambiguous, signature_units: signatures, doc_units: docs } };
}

/** Add bounded, source-backed metadata to one derived projection window. */
export function augmentSourceProjection(record, unit, sourceBytes, attested, fields) {
  if (!attested) return record;
  const route = fields.route ?? "both";
  if (!["both", "fts", "vector"].includes(route))
    throw new Error(`unsupported source metadata route: ${route}`);
  const raw = decoder.decode(sourceBytes.subarray(record.source.start_byte, record.source.end_byte));
  if (record.source.sha256 !== sha256(sourceBytes) || record.unit_id !== unit.id ||
      record.unit_source.sha256 !== record.source.sha256 ||
      !record.lexical_text.endsWith(raw) || !record.vector_input.endsWith(raw))
    throw new Error("projection source window changed before metadata augmentation");
  const lines = [];
  if (fields.signature && attested.signature) lines.push(`signature: ${attested.signature}`);
  if (fields.doc && attested.doc) lines.push(`doc: ${attested.doc}`);
  if (lines.length === 0) return record;
  const lexicalPrefix = record.lexical_text.slice(0, -raw.length);
  const vectorPrefix = record.vector_input.slice(0, -raw.length);
  const addition = lines.join("\n") + "\n";
  // Avoid changing source-window boundaries or overflowing the existing
  // 3,600-character projection contract when a declaration is already large.
  if (Math.max(record.lexical_text.length + (route !== "vector" ? addition.length : 0),
               record.vector_input.length + (route !== "fts" ? addition.length : 0)) > 3_600)
    return record;
  return {
    ...record,
    lexical_text: route === "vector" ? record.lexical_text : `${lexicalPrefix}${addition}${raw}`,
    vector_input: route === "fts" ? record.vector_input : `${vectorPrefix}${addition}${raw}`,
  };
}
