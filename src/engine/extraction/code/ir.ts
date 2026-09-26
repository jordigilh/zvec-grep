import { createHash } from "node:crypto";
import { TextDecoder } from "node:util";
import { resolveAdapter } from "./adapter.js";
import { withParser } from "./tree-sitter/parser.js";
import type { TSNode } from "./tree-sitter/nodes.js";

export type SourceRef = {
  file_id: string;
  sha256: string;
  start_byte: number;
  end_byte: number;
  start: { line: number; column_byte: number };
  end: { line: number; column_byte: number };
};
export type IRFile = {
  file_id: string;
  root_id: string;
  relative_path: string;
  language: string;
  sha256: string;
  byte_length: number;
  extraction: {
    status: "complete" | "partial" | "opaque" | "failed";
    reason?: string;
    frontend_version: string;
  };
};
export type Unit = {
  id: string;
  kind: "file" | "module" | "type" | "function" | "method" | "value" | "opaque";
  subtype?: string;
  name?: string;
  qualified_name?: string;
  parent_id?: string;
  scope_id?: string;
  signature?: { text: string; source: SourceRef };
  documentation?: { text: string; source: SourceRef };
  source: SourceRef;
  origin: { language: string; frontend: string; syntax_kind: string };
  extensions?: { language: string; data: Record<string, unknown> };
};
export type Fact = {
  id: string;
  kind:
    | "contains"
    | "calls"
    | "references"
    | "reads"
    | "writes"
    | "returns"
    | "throws"
    | "implements"
    | "tests"
    | "guards"
    | "imports";
  subject_id: string;
  object_id?: string;
  target_spelling?: string;
  candidate_ids?: string[];
  site: SourceRef;
  status:
    | "observed"
    | "name_candidate"
    | "ambiguous"
    | "type_resolved"
    | "unresolved";
  provenance: {
    frontend: string;
    method: string;
    resolver?: string;
    resolver_version?: string;
  };
  extensions?: { language: string; data: Record<string, unknown> };
};
export type Snapshot = {
  schema: "zvec-grep.code-ir";
  schema_version: 1;
  repository_id: string;
  root_set: string[];
  revision?: string;
  snapshot_id: string;
  frontend_versions: Record<string, string>;
  files: IRFile[];
  units: Unit[];
  facts: Fact[];
};
export type InputFile = {
  root_id: string;
  relative_path: string;
  language: string;
  bytes: Uint8Array;
};
export const FRONTEND_VERSION = "web-tree-sitter-ir-v1.6";
const LANGUAGES = new Set(["go", "python", "rust", "typescript", "tsx"]);
const encoder = new TextEncoder();
const lineStartCache = new WeakMap<Uint8Array, number[]>();

export function digest(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

// IDs hash UTF-8 JSON arrays, with no locale-dependent sorting. A content edit or
// rename changes identity; coincident source spans use encounter-order ordinals.
export function identity(...parts: (string | number | string[])[]): string {
  return digest(encoder.encode(JSON.stringify(parts)));
}

export function position(
  bytes: Uint8Array,
  offset: number,
): { line: number; column_byte: number } {
  if (!Number.isSafeInteger(offset) || offset < 0 || offset > bytes.length)
    throw new Error("IR byte offset out of bounds");
  let starts = lineStartCache.get(bytes);
  if (!starts) {
    starts = [0];
    for (let i = 0; i < bytes.length; i++)
      if (bytes[i] === 10) starts.push(i + 1);
    lineStartCache.set(bytes, starts);
  }
  let low = 0;
  let high = starts.length;
  while (low + 1 < high) {
    const middle = (low + high) >>> 1;
    if (starts[middle] <= offset) low = middle;
    else high = middle;
  }
  return { line: low + 1, column_byte: offset - starts[low] };
}

export function sourceRef(
  file: IRFile,
  bytes: Uint8Array,
  start_byte: number,
  end_byte: number,
): SourceRef {
  if (
    !Number.isSafeInteger(start_byte) ||
    !Number.isSafeInteger(end_byte) ||
    start_byte < 0 ||
    start_byte >= end_byte ||
    end_byte > bytes.length ||
    (bytes[start_byte] & 0xc0) === 0x80 ||
    (end_byte < bytes.length && (bytes[end_byte] & 0xc0) === 0x80)
  )
    throw new Error("IR invalid UTF-8 source range");
  return {
    file_id: file.file_id,
    sha256: file.sha256,
    start_byte,
    end_byte,
    start: position(bytes, start_byte),
    end: position(bytes, end_byte),
  };
}

export function validateSnapshot(
  snapshot: Snapshot,
  sources: ReadonlyMap<string, Uint8Array>,
): void {
  if (snapshot.schema !== "zvec-grep.code-ir" || snapshot.schema_version !== 1)
    throw new Error("unsupported IR schema/version");
  if (
    !snapshot.repository_id ||
    new Set(snapshot.root_set).size !== snapshot.root_set.length
  )
    throw new Error("invalid IR repository/root identity");
  const files = new Map<string, IRFile>();
  for (const file of snapshot.files) {
    const pathParts = file.relative_path.split("/");
    if (
      files.has(file.file_id) ||
      !file.root_id ||
      !file.relative_path ||
      file.relative_path.startsWith("/") ||
      file.relative_path.includes("\\") ||
      pathParts.some((part) => part === "" || part === "." || part === "..") ||
      file.file_id !==
        identity(snapshot.repository_id, file.root_id, file.relative_path) ||
      !snapshot.root_set.includes(file.root_id)
    )
      throw new Error("duplicate or invalid IR file ID/root");
    if (
      !["complete", "partial", "opaque", "failed"].includes(
        file.extraction.status,
      )
    )
      throw new Error("invalid IR extraction status");
    if (
      snapshot.frontend_versions[file.language] !==
        file.extraction.frontend_version ||
      !file.extraction.frontend_version
    )
      throw new Error("IR frontend version mismatch");
    const bytes = sources.get(file.file_id);
    if (
      !bytes ||
      digest(bytes) !== file.sha256 ||
      bytes.length !== file.byte_length
    )
      throw new Error("stale IR file source");
    files.set(file.file_id, file);
  }
  if (
    sources.size !== files.size ||
    [...sources.keys()].some((fileId) => !files.has(fileId))
  )
    throw new Error("IR source set does not match snapshot file set");
  if (snapshot.snapshot_id !== snapshotIdentity(snapshot))
    throw new Error("IR snapshot ID mismatch");
  const check = (ref: SourceRef): void => {
    const file = files.get(ref.file_id);
    const bytes = sources.get(ref.file_id);
    if (!file || !bytes || file.sha256 !== ref.sha256)
      throw new Error("stale IR source reference");
    if (
      !sourceRefsEqual(
        sourceRef(file, bytes, ref.start_byte, ref.end_byte),
        ref,
      )
    )
      throw new Error("IR source map mismatch");
    new TextDecoder("utf-8", { fatal: true }).decode(
      bytes.subarray(ref.start_byte, ref.end_byte),
    );
  };
  const units = new Map<string, Unit>();
  for (const unit of snapshot.units) {
    if (
      units.has(unit.id) ||
      !unit.id ||
      ![
        "file",
        "module",
        "type",
        "function",
        "method",
        "value",
        "opaque",
      ].includes(unit.kind)
    )
      throw new Error("duplicate or invalid IR unit");
    check(unit.source);
    if (!unit.origin.frontend || !unit.origin.syntax_kind)
      throw new Error("IR unit origin is required");
    if (
      unit.kind === "file" &&
      (unit.source.start_byte !== 0 ||
        unit.source.end_byte !== files.get(unit.source.file_id)?.byte_length ||
        unit.parent_id !== undefined)
    )
      throw new Error("IR file unit must cover the complete source");
    if (
      unit.origin.language !== files.get(unit.source.file_id)?.language ||
      (unit.extensions && unit.extensions.language !== unit.origin.language)
    )
      throw new Error("IR language mismatch");
    if (unit.extensions?.data.identifier_source !== undefined) {
      const ref = unit.extensions.data.identifier_source as SourceRef;
      check(ref);
      if (
        !unit.name ||
        ref.file_id !== unit.source.file_id ||
        ref.start_byte < unit.source.start_byte ||
        ref.end_byte > unit.source.end_byte ||
        new TextDecoder("utf-8", { fatal: true }).decode(
          sources.get(ref.file_id)!.subarray(ref.start_byte, ref.end_byte),
        ) !== unit.name
      )
        throw new Error("IR identifier is not source-backed by its unit");
    }
    if (unit.extensions?.data.syntax_source !== undefined) {
      const ref = unit.extensions.data.syntax_source as SourceRef;
      check(ref);
      const name = Buffer.from(unit.name ?? "", "utf8");
      if (
        unit.origin.language !== "go" ||
        unit.kind !== "type" ||
        !["type_spec", "type_alias"].includes(unit.origin.syntax_kind) ||
        name.length === 0 ||
        ref.file_id !== unit.source.file_id ||
        ref.start_byte < unit.source.start_byte ||
        ref.end_byte !== unit.source.end_byte ||
        !Buffer.from(
          sources
            .get(ref.file_id)!
            .subarray(ref.start_byte, ref.start_byte + name.length),
        ).equals(name)
      )
        throw new Error(
          "IR alternate syntax source is not contained by a Go type",
        );
    }
    for (const anchored of [unit.signature, unit.documentation])
      if (anchored) {
        check(anchored.source);
        if (
          anchored.source.file_id !== unit.source.file_id ||
          new TextDecoder("utf-8", { fatal: true }).decode(
            sources
              .get(anchored.source.file_id)!
              .subarray(anchored.source.start_byte, anchored.source.end_byte),
          ) !== anchored.text
        )
          throw new Error("IR synthetic anchored text");
      }
    units.set(unit.id, unit);
  }
  for (const unit of snapshot.units) {
    const parent = unit.parent_id && units.get(unit.parent_id);
    if (
      unit.parent_id &&
      (!parent ||
        parent.source.file_id !== unit.source.file_id ||
        parent.source.start_byte > unit.source.start_byte ||
        parent.source.end_byte < unit.source.end_byte ||
        parent.id === unit.id)
    )
      throw new Error("IR invalid parent");
    if (unit.scope_id && !units.has(unit.scope_id))
      throw new Error("IR missing scope");
    if (
      unit.scope_id &&
      units.get(unit.scope_id)?.source.file_id !== unit.source.file_id
    )
      throw new Error("IR scope crosses files");
  }
  for (const file of snapshot.files) {
    const fileUnits = snapshot.units.filter(
      (unit) => unit.kind === "file" && unit.source.file_id === file.file_id,
    );
    if (
      (file.byte_length > 0 &&
        file.extraction.status !== "failed" &&
        fileUnits.length !== 1) ||
      (file.byte_length === 0 && fileUnits.length !== 0)
    )
      throw new Error("IR file source-unit coverage mismatch");
  }
  const factIds = new Set<string>();
  for (const fact of snapshot.facts) {
    if (
      factIds.has(fact.id) ||
      ![
        "contains",
        "calls",
        "references",
        "reads",
        "writes",
        "returns",
        "throws",
        "implements",
        "tests",
        "guards",
        "imports",
      ].includes(fact.kind) ||
      ![
        "observed",
        "name_candidate",
        "ambiguous",
        "type_resolved",
        "unresolved",
      ].includes(fact.status)
    )
      throw new Error("duplicate or invalid IR fact");
    factIds.add(fact.id);
    check(fact.site);
    const subject = units.get(fact.subject_id);
    const object = fact.object_id ? units.get(fact.object_id) : undefined;
    if (
      !subject ||
      (fact.object_id && !units.has(fact.object_id)) ||
      fact.candidate_ids?.some((id) => !units.has(id)) ||
      (fact.candidate_ids &&
        new Set(fact.candidate_ids).size !== fact.candidate_ids.length)
    )
      throw new Error("IR missing fact endpoint");
    if (
      fact.site.file_id !== subject.source.file_id ||
      fact.site.start_byte < subject.source.start_byte ||
      fact.site.end_byte > subject.source.end_byte ||
      (fact.kind === "contains" &&
        (!object ||
          object.source.file_id !== subject.source.file_id ||
          object.source.start_byte < subject.source.start_byte ||
          object.source.end_byte > subject.source.end_byte ||
          object.source.start_byte !== fact.site.start_byte ||
          object.source.end_byte !== fact.site.end_byte))
    )
      throw new Error("IR fact site is outside subject/source containment");
    if (
      fact.extensions &&
      fact.extensions.language !== files.get(fact.site.file_id)?.language
    )
      throw new Error("IR fact language mismatch");
    if (!fact.provenance.frontend || !fact.provenance.method)
      throw new Error("IR fact provenance is required");
    if (
      (fact.status === "type_resolved" &&
        (!fact.object_id || fact.kind === "contains")) ||
      (fact.kind !== "contains" &&
        fact.status !== "type_resolved" &&
        fact.object_id) ||
      (fact.kind === "contains" &&
        (fact.status !== "observed" || !fact.object_id)) ||
      (fact.status === "name_candidate" && fact.candidate_ids?.length !== 1) ||
      (fact.status === "ambiguous" && (fact.candidate_ids?.length ?? 0) < 2) ||
      ((fact.status === "name_candidate" || fact.status === "ambiguous") &&
        !fact.target_spelling) ||
      (fact.status === "type_resolved" &&
        (!fact.provenance.resolver || !fact.provenance.resolver_version)) ||
      ((fact.status === "unresolved" || fact.status === "observed") &&
        fact.candidate_ids?.length)
    )
      throw new Error("IR invalid relationship status");
  }
}

export function sourceRefsEqual(left: SourceRef, right: SourceRef): boolean {
  return (
    left.file_id === right.file_id &&
    left.sha256 === right.sha256 &&
    left.start_byte === right.start_byte &&
    left.end_byte === right.end_byte &&
    left.start.line === right.start.line &&
    left.start.column_byte === right.start.column_byte &&
    left.end.line === right.end.line &&
    left.end.column_byte === right.end.column_byte
  );
}

export function snapshotIdentity(
  snapshot: Pick<
    Snapshot,
    "repository_id" | "root_set" | "frontend_versions" | "files"
  >,
): string {
  return identity(
    "zvec-grep.code-ir",
    1,
    snapshot.repository_id,
    [...snapshot.root_set].sort(),
    Object.entries(snapshot.frontend_versions)
      .sort()
      .map(([key, value]) => `${key}:${value}`),
    snapshot.files
      .map((f) => `${f.root_id}\0${f.relative_path}\0${f.sha256}`)
      .sort(),
  );
}

// Go's field names are sibling nodes sharing one declaration span. A separate
// ordinal keeps `A, B string` distinct; the token ref disambiguates the names.
function goDeclaredNames(node: TSNode): TSNode[] {
  const names = node.namedChildren.filter((child) =>
    node.type === "field_declaration"
      ? child.type === "field_identifier"
      : child.type === "identifier",
  );
  if (names.length || node.type !== "field_declaration") return names;
  // An embedded field has no `name` field; the terminal type identifier is
  // its syntactic name. Abstain on unsupported/anonymous embedded types.
  const type = node.childForFieldName("type");
  const named =
    type?.type === "qualified_type" ? type.childForFieldName("name") : type;
  return named?.type === "type_identifier" ? [named] : [];
}

function attachedDeclarationStart(
  node: TSNode,
  text: string,
  language: string,
): number {
  const siblings = node.parent?.namedChildren ?? [];
  const index = siblings.findIndex(
    (sibling) =>
      sibling.startIndex === node.startIndex &&
      sibling.endIndex === node.endIndex &&
      sibling.type === node.type,
  );
  let start = node.startIndex;
  for (let i = index - 1; i >= 0; i--) {
    const previous = siblings[i];
    const attached =
      language === "go"
        ? previous.type === "comment" && /^(?:\/\/|\/\*)/u.test(previous.text)
        : previous.type === "attribute_item" ||
          (["line_comment", "block_comment"].includes(previous.type) &&
            /^(?:\/\/\/|\/\*\*)/u.test(previous.text));
    if (
      !attached ||
      !/^\r?\n[\t ]*$/u.test(text.slice(previous.endIndex, start))
    )
      break;
    start = previous.startIndex;
  }
  return start;
}

// Keep metadata as original source slices, not the adapters' normalized
// display strings. All positions refer to the single tree built for this file.
function signatureRange(
  node: TSNode,
  language: string,
): [number, number] | null {
  const declaration =
    language === "python" && node.type === "decorated_definition"
      ? (node.namedChildren.find(
          (child) =>
            child.type === "class_definition" ||
            child.type === "function_definition",
        ) ?? node)
      : node;
  const bodyStart = (candidate: TSNode): number | undefined => {
    const body =
      candidate.childForFieldName("body") ??
      candidate.namedChildren.find((child) =>
        [
          "block",
          "statement_block",
          "class_body",
          "declaration_list",
          "field_declaration_list",
        ].includes(child.type),
      );
    if (body) return body.startIndex;
    const wrapper =
      candidate.type === "export_statement"
        ? (candidate.childForFieldName("declaration") ??
          candidate.namedChildren.find((child) =>
            child.type.endsWith("_declaration"),
          ))
        : language === "go" && candidate.type === "type_declaration"
          ? candidate.namedChildren.find((child) =>
              ["type_spec", "type_alias"].includes(child.type),
            )
          : language === "go" &&
              ["type_spec", "type_alias"].includes(candidate.type)
            ? candidate.childForFieldName("type")
            : undefined;
    return wrapper ? bodyStart(wrapper) : undefined;
  };
  const body = bodyStart(declaration);
  let end = body ?? declaration.endIndex;
  if (body === undefined) {
    const lineBreak = declaration.text.search(/\r?\n/u);
    // A first line is not a complete signature when the parser provides no
    // body boundary. Keep the unit, but omit uncertain metadata.
    if (lineBreak >= 0) return null;
  }
  const candidate = declaration.text.slice(0, end - declaration.startIndex);
  end -=
    candidate.length -
    candidate
      .trimEnd()
      .replace(/[;{]\s*$/u, "")
      .trimEnd().length;
  return end > declaration.startIndex && end - declaration.startIndex <= 1_200
    ? [declaration.startIndex, end]
    : null;
}

function documentationRange(
  node: TSNode,
  text: string,
  language: string,
): [number, number] | null {
  const isDoc = (sibling: TSNode): boolean => {
    if (!["comment", "line_comment", "block_comment"].includes(sibling.type))
      return false;
    if (language === "rust")
      return /^(?:\/\/\/|\/\/!|\/\*\*|\/\*!)/u.test(sibling.text);
    if (language === "typescript" || language === "tsx")
      return /^\/\*\*/u.test(sibling.text);
    return language === "python"
      ? sibling.text.startsWith("#")
      : /^(?:\/\/|\/\*)/u.test(sibling.text);
  };
  let earliest: number | undefined;
  let latest: number | undefined;
  let start = node.startIndex;
  let sibling = node.previousNamedSibling;
  while (
    sibling &&
    /^\r?\n[\t ]*$/u.test(text.slice(sibling.endIndex, start))
  ) {
    if (
      language === "rust" &&
      sibling.type === "attribute_item" &&
      latest === undefined
    ) {
      start = sibling.startIndex;
      sibling = sibling.previousNamedSibling;
      continue;
    }
    if (!isDoc(sibling)) break;
    earliest = sibling.startIndex;
    // Some grammars include the CR preceding a CRLF in a line-comment node.
    latest ??= sibling.endIndex - (text[sibling.endIndex - 1] === "\r" ? 1 : 0);
    start = sibling.startIndex;
    sibling = sibling.previousNamedSibling;
  }
  return earliest !== undefined && latest !== undefined
    ? [earliest, latest]
    : null;
}

// web-tree-sitter 0.20 indexes JS UTF-16 code units. Reject any midpoint in a
// surrogate pair; then convert to UTF-8 offsets in the *original* byte buffer.
function createByteOffsetMapper(text: string): (index: number) => number {
  const charCheckpoints = [0];
  const byteCheckpoints = [0];
  let charIndex = 0;
  let byteOffset = 0;
  for (const character of text) {
    charIndex += character.length;
    byteOffset += Buffer.byteLength(character, "utf8");
    if (charIndex - charCheckpoints.at(-1)! >= 256) {
      charCheckpoints.push(charIndex);
      byteCheckpoints.push(byteOffset);
    }
  }
  return (index) => {
    if (
      !Number.isSafeInteger(index) ||
      index < 0 ||
      index > text.length ||
      (index > 0 &&
        index < text.length &&
        /[\uD800-\uDBFF]/u.test(text[index - 1]) &&
        /[\uDC00-\uDFFF]/u.test(text[index]))
    )
      throw new Error("IR parser index splits character");
    let low = 0;
    let high = charCheckpoints.length;
    while (low + 1 < high) {
      const middle = (low + high) >>> 1;
      if (charCheckpoints[middle] <= index) low = middle;
      else high = middle;
    }
    return (
      byteCheckpoints[low] +
      Buffer.byteLength(text.slice(charCheckpoints[low], index), "utf8")
    );
  };
}

export async function extractSnapshot(
  repository_id: string,
  files: readonly InputFile[],
): Promise<{ snapshot: Snapshot; sources: Map<string, Uint8Array> }> {
  const sources = new Map<string, Uint8Array>();
  const snapshot: Snapshot = {
    schema: "zvec-grep.code-ir",
    schema_version: 1,
    repository_id,
    root_set: [...new Set(files.map((f) => f.root_id))].sort(),
    snapshot_id: "",
    frontend_versions: {},
    files: [],
    units: [],
    facts: [],
  };
  for (const input of [...files].sort((a, b) =>
    `${a.root_id}\0${a.relative_path}`.localeCompare(
      `${b.root_id}\0${b.relative_path}`,
      "en",
    ),
  )) {
    const file: IRFile = {
      file_id: identity(repository_id, input.root_id, input.relative_path),
      root_id: input.root_id,
      relative_path: input.relative_path,
      language: input.language,
      sha256: digest(input.bytes),
      byte_length: input.bytes.length,
      extraction: { status: "opaque", frontend_version: FRONTEND_VERSION },
    };
    sources.set(file.file_id, input.bytes);
    snapshot.files.push(file);
    snapshot.frontend_versions[input.language] = FRONTEND_VERSION;
    if (!input.bytes.length) {
      file.extraction.reason = "empty file";
      continue;
    }
    const text = (() => {
      try {
        return new TextDecoder("utf-8", { fatal: true }).decode(input.bytes);
      } catch {
        return null;
      }
    })();
    if (text === null) {
      file.extraction.status = "failed";
      file.extraction.reason = "invalid UTF-8";
      continue;
    }
    const byteOffset = createByteOffsetMapper(text);
    const anchoredText = (range: [number, number] | null) => {
      if (!range) return undefined;
      const source = sourceRef(
        file,
        input.bytes,
        byteOffset(range[0]),
        byteOffset(range[1]),
      );
      const raw = text.slice(range[0], range[1]);
      if (
        new TextDecoder("utf-8", { fatal: true }).decode(
          input.bytes.subarray(source.start_byte, source.end_byte),
        ) !== raw
      )
        throw new Error("IR metadata source slice mismatch");
      return { text: raw, source };
    };
    const makeUnit = (
      kind: Unit["kind"],
      start: number,
      end: number,
      ordinal: number,
      syntax_kind: string,
      name?: string,
    ): Unit => ({
      id: identity(file.file_id, file.sha256, kind, start, end, ordinal),
      kind,
      ...(name ? { name } : {}),
      source: sourceRef(file, input.bytes, start, end),
      origin: {
        language: input.language,
        frontend: FRONTEND_VERSION,
        syntax_kind,
      },
    });
    const fileUnit = makeUnit("file", 0, input.bytes.length, 0, "source_file");
    snapshot.units.push(fileUnit);
    if (!LANGUAGES.has(input.language) || !resolveAdapter(input.language)) {
      file.extraction.reason = "unsupported grammar";
      continue;
    }
    const adapter = resolveAdapter(input.language)!;
    const parsed = await withParser(text, input.language, (tree) => {
      const errors: { start: number; end: number }[] = [];
      const visitErrors = (node: TSNode): void => {
        if (node.type === "ERROR" || node.isMissing())
          errors.push({ start: node.startIndex, end: node.endIndex });
        else for (const child of node.children) visitErrors(child);
      };
      visitErrors(tree.rootNode);
      const occurrences = new Map<string, number>();
      const emitFact = (
        kind: Fact["kind"],
        subject: Unit,
        site: SourceRef,
        object?: Unit,
        spelling?: string,
      ): void => {
        const key = JSON.stringify([
          subject.id,
          kind,
          site.start_byte,
          site.end_byte,
          object?.id,
          spelling,
        ]);
        const ordinal = occurrences.get(key) ?? 0;
        occurrences.set(key, ordinal + 1);
        snapshot.facts.push({
          id: identity(key, ordinal),
          kind,
          subject_id: subject.id,
          ...(object ? { object_id: object.id } : {}),
          ...(spelling ? { target_spelling: spelling } : {}),
          site,
          status: object ? "observed" : "unresolved",
          provenance: { frontend: FRONTEND_VERSION, method: "syntax" },
        });
      };
      const walk = (
        node: TSNode,
        parent: Unit,
        breadcrumb: string[],
        uncertain = false,
      ): void => {
        for (const child of node.namedChildren) {
          const bad = errors.some(
            (error) =>
              child.startIndex < error.end && error.start < child.endIndex,
          );
          const uncertainRegion = uncertain || bad;
          let owner = parent;
          const typescriptOverload =
            (input.language === "typescript" || input.language === "tsx") &&
            child.type === "function_declaration" &&
            child.childForFieldName("body") === null;
          const typescriptTopLevelValue =
            (input.language === "typescript" || input.language === "tsx") &&
            child.type === "variable_declarator" &&
            ["file", "module"].includes(parent.kind);
          const typescriptFieldValue =
            (input.language === "typescript" || input.language === "tsx") &&
            ["field_definition", "public_field_definition"].includes(
              child.type,
            );
          const goField =
            input.language === "go" &&
            child.type === "field_declaration" &&
            child.parent?.type === "field_declaration_list" &&
            ["type", "value"].includes(parent.kind);
          const goPackageValue =
            input.language === "go" &&
            ["var_spec", "const_spec"].includes(child.type) &&
            parent.kind === "file";
          const pythonClassAttribute =
            input.language === "python" &&
            child.type === "assignment" &&
            child.parent?.type === "expression_statement" &&
            child.parent.parent?.type === "block" &&
            parent.kind === "type" &&
            child.childForFieldName("left")?.type === "identifier";
          const isEntity =
            !uncertainRegion &&
            (adapter.entityTypes.has(child.type) ||
              goField ||
              goPackageValue ||
              pythonClassAttribute ||
              ((input.language === "typescript" || input.language === "tsx") &&
                child.type === "function_signature") ||
              (input.language === "rust" && child.type === "mod_item")) &&
            (adapter.shouldIndexEntity?.(child) !== false ||
              typescriptOverload ||
              typescriptTopLevelValue ||
              typescriptFieldValue ||
              goField ||
              goPackageValue ||
              pythonClassAttribute) &&
            !(
              input.language === "python" &&
              parent.origin.syntax_kind === "decorated_definition" &&
              parent.name === adapter.extractName(child) &&
              ["class_definition", "function_definition"].includes(child.type)
            );
          if (isEntity)
            for (const { entity, identifier } of goField || goPackageValue
              ? goDeclaredNames(child).map((identifier) => ({
                  entity: child,
                  identifier,
                }))
              : pythonClassAttribute
                ? [
                    {
                      entity: child,
                      identifier: child.childForFieldName("left")!,
                    },
                  ]
                : (
                    adapter.resolveEntities?.(child) ?? [
                      adapter.resolveEntity?.(child) ?? child,
                    ]
                  ).map((entity) => ({ entity, identifier: undefined }))) {
              const name = identifier?.text ?? adapter.extractName(entity);
              const entityBreadcrumb =
                adapter.scopeBreadcrumb?.(entity, breadcrumb) ?? breadcrumb;
              const symbol = adapter.classifyNode?.(entity, breadcrumb);
              const pythonDefinition =
                child.type === "decorated_definition"
                  ? child.namedChildren.find(
                      (n) =>
                        n.type === "class_definition" ||
                        n.type === "function_definition",
                    )
                  : undefined;
              const kind: Unit["kind"] =
                goField || goPackageValue || pythonClassAttribute
                  ? "value"
                  : child.type.includes("method") ||
                      child.type.includes("constructor") ||
                      child.type === "method_spec" ||
                      child.type === "function_signature_item" ||
                      (input.language === "python" &&
                        parent.kind === "type" &&
                        (child.type.includes("function") ||
                          pythonDefinition?.type === "function_definition")) ||
                      (input.language === "rust" &&
                        parent.kind === "type" &&
                        child.type === "function_item")
                    ? "method"
                    : pythonDefinition?.type === "class_definition" ||
                        symbol === "class" ||
                        symbol === "interface" ||
                        symbol === "alias" ||
                        child.type.includes("type_") ||
                        child.type.includes("class") ||
                        child.type.includes("interface") ||
                        child.type.includes("struct") ||
                        child.type.includes("trait") ||
                        child.type.includes("enum") ||
                        child.type.includes("union") ||
                        child.type.includes("alias") ||
                        child.type === "impl_item"
                      ? "type"
                      : symbol === "module" || child.type === "mod_item"
                        ? "module"
                        : pythonDefinition?.type === "function_definition" ||
                            symbol === "function" ||
                            child.type.includes("function")
                          ? "function"
                          : "value";
              const declaration =
                input.language === "go" &&
                ["type_spec", "type_alias"].includes(entity.type) &&
                entity.parent?.type === "type_declaration" &&
                entity.parent.namedChildren.filter((node) =>
                  ["type_spec", "type_alias"].includes(node.type),
                ).length === 1
                  ? entity.parent
                  : entity;
              const rustAttributed =
                input.language === "rust" &&
                [
                  "struct_item",
                  "enum_item",
                  "trait_item",
                  "type_item",
                  "union_item",
                  "function_item",
                ].includes(entity.type);
              const declarationStart =
                (input.language === "go" && declaration !== entity) ||
                rustAttributed
                  ? attachedDeclarationStart(declaration, text, input.language)
                  : (input.language === "typescript" ||
                        input.language === "tsx") &&
                      entity.parent?.type === "export_statement" &&
                      entity.parent.childForFieldName("declaration")
                        ?.startIndex === entity.startIndex
                    ? entity.parent.startIndex
                    : declaration.startIndex;
              const start = byteOffset(declarationStart),
                end = byteOffset(declaration.endIndex);
              const key = `${kind}:${start}:${end}`;
              const ordinal = occurrences.get(key) ?? 0;
              occurrences.set(key, ordinal + 1);
              const unit = makeUnit(
                kind,
                start,
                end,
                ordinal,
                entity.type,
                name,
              );
              unit.parent_id = parent.id;
              unit.scope_id = parent.id;
              if (name)
                unit.qualified_name = [...entityBreadcrumb, name].join("::");
              unit.subtype = goField
                ? "struct_field"
                : goPackageValue
                  ? child.type
                  : pythonClassAttribute
                    ? "class_attribute"
                    : (symbol ?? entity.type);
              if (identifier) {
                unit.extensions = {
                  language: input.language,
                  data: {
                    identifier_source: sourceRef(
                      file,
                      input.bytes,
                      byteOffset(identifier.startIndex),
                      byteOffset(identifier.endIndex),
                    ),
                  },
                };
              }
              if (input.language === "go" && declaration !== entity) {
                unit.extensions = {
                  language: "go",
                  data: {
                    syntax_source: sourceRef(
                      file,
                      input.bytes,
                      byteOffset(entity.startIndex),
                      byteOffset(entity.endIndex),
                    ),
                  },
                };
              }
              if (
                input.language === "go" &&
                child.type === "method_declaration" &&
                entityBreadcrumb.length > breadcrumb.length
              ) {
                unit.extensions = {
                  language: "go",
                  data: { receiver: entityBreadcrumb.at(-1) },
                };
              } else if (
                input.language === "python" &&
                child.type === "decorated_definition"
              ) {
                unit.extensions = {
                  language: "python",
                  data: { decorated_definition: true },
                };
              } else if (
                input.language === "rust" &&
                child.type === "impl_item"
              ) {
                unit.extensions = {
                  language: "rust",
                  data: { impl: true },
                };
              } else if (
                (input.language === "typescript" || input.language === "tsx") &&
                child.type === "function_signature"
              ) {
                unit.extensions = {
                  language: input.language,
                  data: { overload_signature: true },
                };
              }
              const metadataNode =
                (input.language === "typescript" || input.language === "tsx") &&
                entity.parent?.type === "export_statement" &&
                entity.parent.childForFieldName("declaration")?.startIndex ===
                  entity.startIndex
                  ? entity.parent
                  : declaration;
              const signature =
                ["type", "function", "method"].includes(kind) &&
                ![
                  "field_definition",
                  "public_field_definition",
                  "variable_declarator",
                  "pair",
                ].includes(entity.type)
                  ? anchoredText(signatureRange(metadataNode, input.language))
                  : undefined;
              if (signature) unit.signature = signature;
              const documentation = anchoredText(
                documentationRange(metadataNode, text, input.language),
              );
              if (documentation) unit.documentation = documentation;
              snapshot.units.push(unit);
              emitFact("contains", parent, unit.source, unit);
              if (
                entity.startIndex === child.startIndex &&
                entity.endIndex === child.endIndex
              )
                owner = unit;
            }
          if (
            !uncertainRegion &&
            ["call", "call_expression", "function_call_expression"].includes(
              child.type,
            )
          ) {
            const target =
              child.childForFieldName("function") ?? child.namedChildren[0];
            if (target?.text && target.text.length < 180)
              emitFact(
                "calls",
                parent,
                sourceRef(
                  file,
                  input.bytes,
                  byteOffset(child.startIndex),
                  byteOffset(child.endIndex),
                ),
                undefined,
                target.text,
              );
          }
          // Preserve nested callable ownership, including closures, independently of
          // whether the old retrieval extractor indexes their enclosing declaration.
          // Decorated Python definitions wrap the actual declaration; the
          // wrapper is the canonical range, but its body still contains facts.
          if (
            input.language === "python" &&
            child.type === "class_definition" &&
            parent.origin.syntax_kind === "decorated_definition" &&
            parent.name === adapter.extractName(child)
          ) {
            walk(child, parent, breadcrumb, uncertainRegion);
          } else {
            walk(
              child,
              owner,
              owner === parent
                ? breadcrumb
                : [...breadcrumb, owner.name ?? "<anonymous>"],
              uncertainRegion,
            );
          }
        }
      };
      walk(tree.rootNode, fileUnit, []);
      if (errors.length) {
        file.extraction.status = "partial";
        file.extraction.reason = `${errors.length} parser error/missing regions`;
        for (const error of errors) {
          const start = byteOffset(error.start),
            end = byteOffset(error.end);
          if (start < end) {
            const unit = makeUnit("opaque", start, end, 0, "ERROR");
            unit.parent_id = fileUnit.id;
            snapshot.units.push(unit);
            emitFact("contains", fileUnit, unit.source, unit);
          }
        }
      } else file.extraction.status = "complete";
    });
    if (parsed === null) {
      file.extraction.reason = "parser unavailable";
    }
  }
  snapshot.snapshot_id = snapshotIdentity(snapshot);
  validateSnapshot(snapshot, sources);
  return { snapshot, sources };
}
