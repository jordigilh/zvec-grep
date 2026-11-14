import { createHash } from "node:crypto";
import { readdir, readFile, stat } from "node:fs/promises";
import { join, relative, resolve } from "node:path";

type ArtifactFile = {
  path: string;
  size_bytes: number;
  sha256: string;
};

const digestCache = new Map<
  string,
  { generationKey: string; digest: Promise<string> }
>();

/**
 * Compute one artifact fingerprint per published index generation.
 *
 * The promise is cached by generation, so traced searches never synchronously
 * traverse or read the index. Lock state is runtime data, not index content,
 * and is deliberately excluded from the fingerprint.
 */
export function cachedArtifactDigest(
  path: string,
  generationKey: string,
  fallback: Record<string, unknown>,
): Promise<string> {
  const cacheKey = resolve(path);
  const cached = digestCache.get(cacheKey);
  if (cached?.generationKey === generationKey) {
    return cached.digest;
  }

  const digest = computeArtifactDigest(cacheKey, fallback);
  digestCache.set(cacheKey, { generationKey, digest });
  return digest;
}

export function invalidateArtifactDigest(path: string): void {
  digestCache.delete(resolve(path));
}

async function computeArtifactDigest(
  path: string,
  fallback: Record<string, unknown>,
): Promise<string> {
  try {
    const details = await stat(path);
    if (details.isFile()) {
      return sha256Bytes(await readFile(path));
    }
    if (details.isDirectory()) {
      const files: ArtifactFile[] = [];
      await collectArtifactFiles(path, path, files);
      return sha256Value(files);
    }
  } catch {
    // A diagnostic trace must not make an otherwise usable search fail when a
    // test double or a legacy index path is unavailable.
  }
  return sha256Value(fallback);
}

async function collectArtifactFiles(
  root: string,
  current: string,
  files: ArtifactFile[],
): Promise<void> {
  const entries = (await readdir(current, { withFileTypes: true })).sort(
    (left, right) => compareStable(left.name, right.name),
  );

  for (const entry of entries) {
    if (current === root && entry.isDirectory() && entry.name === "locks") {
      continue;
    }

    const path = join(current, entry.name);
    if (entry.isDirectory()) {
      await collectArtifactFiles(root, path, files);
      continue;
    }
    if (!entry.isFile()) {
      continue;
    }

    const bytes = await readFile(path);
    files.push({
      path: relative(root, path).split("\\").join("/"),
      size_bytes: bytes.byteLength,
      sha256: sha256Bytes(bytes),
    });
  }
}

function sha256Value(value: unknown): string {
  return sha256Bytes(Buffer.from(canonicalJson(value), "utf8"));
}

function sha256Bytes(bytes: Uint8Array): string {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function canonicalJson(value: unknown): string {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value) ?? "null";
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => canonicalJson(item)).join(",")}]`;
  }
  const record = value as Record<string, unknown>;
  return `{${Object.keys(record)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(record[key])}`)
    .join(",")}}`;
}

function compareStable(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}
