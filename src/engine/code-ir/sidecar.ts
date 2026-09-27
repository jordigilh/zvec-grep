import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import {
  digest,
  extractSnapshot,
  sourceRef,
  sourceRefsEqual,
  validateSnapshot,
  type InputFile,
  type Snapshot,
  type SourceRef,
} from "./index.js";
import { identifierParts } from "../extraction/index.js";

export type IRMode = "off" | "shadow" | "projection";
export type ProjectionPolicy = "all-units-v1" | "source-metadata-entity-v1";
export type Projection = {
  schema: "zvec-grep.code-ir.projection";
  version: 1 | 2;
  policy?: "source-metadata-entity-v1";
  ir_snapshot_id: string;
  model_identity: string | null;
  records: {
    unit_id: string;
    group_id: string;
    window_index: number;
    unit_source: SourceRef;
    source: SourceRef;
    lexical_text: string;
    vector_input: string;
  }[];
};
export type PublishedIR = {
  ir_snapshot_id: string;
  mode: Exclude<IRMode, "off">;
  projection_version: 1 | 2 | null;
  projection_policy?: "source-metadata-entity-v1";
  projection_model_identity?: string | null;
  source_selection: string[];
  source_hashes: Record<string, string>;
};

/** An independent, opt-in sidecar. The default search/index storage never joins this
 * sidecar. Projection mode publishes candidate records for inspection, not ranks. */
export async function publishIR(
  directory: string,
  repositoryId: string,
  files: readonly InputFile[],
  mode: IRMode,
  modelIdentity: string | null = null,
  policy: ProjectionPolicy = "all-units-v1",
): Promise<PublishedIR | null> {
  if (policy !== "all-units-v1" && policy !== "source-metadata-entity-v1")
    throw new Error("unknown IR projection policy");
  if (mode !== "projection" && policy !== "all-units-v1")
    throw new Error("IR projection policy requires projection mode");
  if (mode === "off") return null;
  if (mode !== "shadow" && mode !== "projection")
    throw new Error("unknown IR mode");
  const version = policy === "source-metadata-entity-v1" ? 2 : 1;
  const { snapshot, sources } = await extractSnapshot(repositoryId, files);
  validateSnapshot(snapshot, sources);
  const manifest: PublishedIR = {
    ir_snapshot_id: snapshot.snapshot_id,
    mode,
    projection_version: mode === "projection" ? version : null,
    ...(mode === "projection" && version === 2
      ? { projection_policy: policy as "source-metadata-entity-v1" }
      : {}),
    ...(mode === "projection"
      ? { projection_model_identity: modelIdentity }
      : {}),
    source_selection: snapshot.root_set,
    source_hashes: Object.fromEntries(
      snapshot.files.map((f) => [f.file_id, f.sha256]),
    ),
  };
  const generation = join(directory, "generations", snapshot.snapshot_id);
  await mkdir(generation, { recursive: true });
  const projection: Projection = {
    schema: "zvec-grep.code-ir.projection",
    version,
    ...(version === 2 ? { policy: policy as "source-metadata-entity-v1" } : {}),
    ir_snapshot_id: snapshot.snapshot_id,
    model_identity: modelIdentity,
    records: [],
  };
  if (mode === "projection")
    projection.records = projectRecords(snapshot, sources, policy);
  // Immutable, content-addressed generation. Concurrent writers of identical
  // snapshots produce identical bytes; readers pin active.json only once.
  const payloads: [string, string][] = [
    ["snapshot.json", JSON.stringify(snapshot)],
    [
      "sources.json",
      JSON.stringify(
        Object.fromEntries(
          [...sources].map(([id, bytes]) => [
            id,
            Buffer.from(bytes).toString("base64"),
          ]),
        ),
      ),
    ],
  ];
  if (mode === "projection")
    payloads.push([
      projectionFileName(version, modelIdentity),
      JSON.stringify(projection),
    ]);
  for (const [name, content] of payloads) {
    const path = join(generation, name);
    const existing = await readFile(path, "utf8").catch(() => null);
    if (
      existing !== null &&
      digest(Buffer.from(existing)) !== digest(Buffer.from(content))
    )
      throw new Error("IR generation collision");
    if (existing === null) await writeFile(path, content, { flag: "wx" });
  }
  // Readers never see a partially written manifest. A crash before rename
  // leaves the previous active generation available.
  const temporary = join(directory, `active.${randomUUID()}.tmp`);
  await writeFile(temporary, JSON.stringify(manifest), { flag: "wx" });
  await rename(temporary, join(directory, "active.json"));
  return manifest;
}

export async function readPublishedIR(directory: string): Promise<{
  manifest: PublishedIR;
  snapshot: Snapshot;
  projection?: Projection;
}> {
  const manifest: PublishedIR = JSON.parse(
    await readFile(join(directory, "active.json"), "utf8"),
  );
  const generation = join(directory, "generations", manifest.ir_snapshot_id);
  const snapshot: Snapshot = JSON.parse(
    await readFile(join(generation, "snapshot.json"), "utf8"),
  );
  const encoded: Record<string, string> = JSON.parse(
    await readFile(join(generation, "sources.json"), "utf8"),
  );
  const sources = new Map(
    Object.entries(encoded).map(([id, base64]) => [
      id,
      Buffer.from(base64, "base64"),
    ]),
  );
  validateSnapshot(snapshot, sources);
  if (
    snapshot.snapshot_id !== manifest.ir_snapshot_id ||
    snapshot.files.some((f) => manifest.source_hashes[f.file_id] !== f.sha256)
  )
    throw new Error("IR manifest mismatch");
  if (manifest.mode !== "projection") return { manifest, snapshot };
  const version = manifest.projection_version;
  if (
    (version !== 1 && version !== 2) ||
    (version === 2 &&
      manifest.projection_policy !== "source-metadata-entity-v1") ||
    (version === 1 && manifest.projection_policy !== undefined)
  )
    throw new Error("IR projection mismatch");
  const policy: ProjectionPolicy =
    version === 2 ? "source-metadata-entity-v1" : "all-units-v1";
  const projection: Projection = JSON.parse(
    await readFile(
      join(
        generation,
        projectionFileName(version, manifest.projection_model_identity ?? null),
      ),
      "utf8",
    ),
  );
  const invalidRecord = projection.records.some((r) => {
    const unit = snapshot.units.find((candidate) => candidate.id === r.unit_id);
    const file = snapshot.files.find(
      (candidate) => candidate.file_id === r.source.file_id,
    );
    const bytes = sources.get(r.source.file_id);
    if (
      !unit ||
      !ranksStandalone(unit, policy) ||
      !file ||
      !bytes ||
      r.group_id !== unit.id ||
      !Number.isSafeInteger(r.window_index) ||
      r.window_index < 0 ||
      !sourceRefsEqual(unit.source, r.unit_source) ||
      r.source.file_id !== unit.source.file_id ||
      r.source.sha256 !== unit.source.sha256 ||
      r.source.start_byte < unit.source.start_byte ||
      r.source.end_byte > unit.source.end_byte
    )
      return true;
    try {
      const selected = new TextDecoder("utf-8", { fatal: true }).decode(
        bytes.subarray(r.source.start_byte, r.source.end_byte),
      );
      return (
        !sourceRefsEqual(
          sourceRef(file, bytes, r.source.start_byte, r.source.end_byte),
          r.source,
        ) ||
        !r.lexical_text.endsWith(selected) ||
        !r.vector_input.endsWith(selected)
      );
    } catch {
      return true;
    }
  });
  const windowsByUnit = new Map<string, Projection["records"]>();
  for (const record of projection.records) {
    const records = windowsByUnit.get(record.unit_id) ?? [];
    records.push(record);
    windowsByUnit.set(record.unit_id, records);
  }
  const invalidCoverage = snapshot.units
    .filter((unit) => ranksStandalone(unit, policy))
    .some((unit) => {
      const windows = windowsByUnit
        .get(unit.id)
        ?.sort((a, b) => a.window_index - b.window_index);
      if (!windows?.length) return true;
      let cursor = unit.source.start_byte;
      for (const [index, window] of windows.entries()) {
        if (
          window.window_index !== index ||
          window.source.start_byte !== cursor ||
          window.source.end_byte <= cursor
        )
          return true;
        cursor = window.source.end_byte;
      }
      return cursor !== unit.source.end_byte;
    });
  if (
    projection.schema !== "zvec-grep.code-ir.projection" ||
    projection.version !== version ||
    projection.policy !==
      (version === 2 ? "source-metadata-entity-v1" : undefined) ||
    projection.ir_snapshot_id !== manifest.ir_snapshot_id ||
    projection.model_identity !==
      (manifest.projection_model_identity ?? null) ||
    invalidRecord ||
    invalidCoverage
  )
    throw new Error("IR projection mismatch");
  if (
    version === 2 &&
    JSON.stringify(projection.records) !==
      JSON.stringify(projectRecords(snapshot, sources, policy))
  )
    throw new Error("IR projection mismatch");
  return { manifest, snapshot, projection };
}

function projectionFileName(
  version: 1 | 2,
  modelIdentity: string | null,
): string {
  return `projection-v${version}-${digest(Buffer.from(modelIdentity ?? "none")).slice(0, 16)}.json`;
}

function ranksStandalone(
  unit: Snapshot["units"][number],
  policy: ProjectionPolicy,
): boolean {
  if (policy === "all-units-v1") return true;
  if (unit.kind === "file" || unit.kind === "opaque") return false;
  return !(
    unit.kind === "value" &&
    ((unit.origin.language === "go" && unit.subtype === "struct_field") ||
      (unit.origin.language === "python" && unit.subtype === "class_attribute"))
  );
}

function projectRecords(
  snapshot: Snapshot,
  sources: Map<string, Uint8Array>,
  policy: ProjectionPolicy,
): Projection["records"] {
  const records: Projection["records"] = [];
  for (const unit of snapshot.units) {
    if (!ranksStandalone(unit, policy)) continue;
    const bytes = sources.get(unit.source.file_id)!;
    // Search text is derived; the original bytes and source refs alone are evidence.
    const raw = new TextDecoder("utf-8", { fatal: true }).decode(
      bytes.subarray(unit.source.start_byte, unit.source.end_byte),
    );
    const symbolType = projectionSymbolType(unit.subtype, unit.kind);
    const qualified = unit.qualified_name ?? unit.name;
    const nameParts = qualified ? identifierParts(qualified) : [];
    const sourceMetadata =
      policy === "source-metadata-entity-v1"
        ? [
            unit.signature ? `signature: ${unit.signature.text}` : null,
            unit.documentation ? `doc: ${unit.documentation.text}` : null,
          ].filter((line): line is string => line !== null)
        : [];
    const metadataLines =
      unit.kind === "file" || unit.kind === "opaque"
        ? []
        : [
            unit.name
              ? `symbol: ${symbolType} ${unit.name}`
              : `symbol: ${symbolType}`,
            qualified ? `qualified: ${qualified}` : null,
            nameParts.length ? `name_parts: ${nameParts.join(" ")}` : null,
            unit.qualified_name && unit.name
              ? `scope: ${unit.qualified_name.slice(0, -(unit.name.length + 2))}`
              : null,
            ...sourceMetadata,
          ].filter((line): line is string => line !== null);
    const metadata = fitMetadata(metadataLines.join("\n"), 900);
    const vectorMetadata = fitMetadata(
      metadataLines
        .filter(
          (line) =>
            !line.startsWith("qualified:") && !line.startsWith("name_parts:"),
        )
        .join("\n"),
      900,
    );
    const metadataLength = Math.max(metadata.length, vectorMetadata.length);
    const sourceWindowLimit = Math.max(
      1,
      3_600 - metadataLength - (metadataLength ? 1 : 0),
    );
    for (const [window_index, window] of sourceWindows(
      raw,
      sourceWindowLimit,
    ).entries()) {
      const byteStart =
        unit.source.start_byte +
        Buffer.byteLength(raw.slice(0, window.start), "utf8");
      const byteEnd =
        unit.source.start_byte +
        Buffer.byteLength(raw.slice(0, window.end), "utf8");
      const windowSource = sourceRef(
        snapshot.files.find((file) => file.file_id === unit.source.file_id)!,
        bytes,
        byteStart,
        byteEnd,
      );
      records.push({
        unit_id: unit.id,
        group_id: unit.id,
        window_index,
        unit_source: unit.source,
        source: windowSource,
        lexical_text: metadata.length
          ? `${metadata}\n${window.text}`
          : window.text,
        vector_input: vectorMetadata
          ? `${vectorMetadata}\n${window.text}`
          : window.text,
      });
    }
  }
  return records;
}

function projectionSymbolType(
  subtype: string | undefined,
  kind: string,
): string {
  if (
    ["class", "interface", "alias", "function", "module", "value"].includes(
      subtype ?? "",
    )
  )
    return subtype!;
  return kind === "type" ? "class" : kind;
}

function sourceWindows(
  text: string,
  maxChars: number,
): { start: number; end: number; text: string }[] {
  const windows: { start: number; end: number; text: string }[] = [];
  let start = 0;
  while (start < text.length) {
    let end = Math.min(text.length, start + maxChars);
    if (
      end < text.length &&
      /[\uD800-\uDBFF]/u.test(text[end - 1]) &&
      /[\uDC00-\uDFFF]/u.test(text[end])
    )
      end--;
    if (end === start) end = Math.min(text.length, start + 2);
    windows.push({ start, end, text: text.slice(start, end) });
    start = end;
  }
  return windows;
}

function fitMetadata(value: string, maxChars: number): string {
  if (value.length <= maxChars) return value;
  const suffix = "...";
  let end = maxChars - suffix.length;
  if (
    end > 0 &&
    end < value.length &&
    /[\uD800-\uDBFF]/u.test(value[end - 1]) &&
    /[\uDC00-\uDFFF]/u.test(value[end])
  )
    end--;
  return `${value.slice(0, end).trimEnd()}${suffix}`;
}
