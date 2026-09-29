#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile, mkdir } from "node:fs/promises";
import { promisify } from "node:util";
import { join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const execFileAsync = promisify(execFile);
const root = await mkdtemp(
  join(process.env.TMPDIR ?? "/tmp", "zg-ts-callfacts-test-"),
);

try {
  await mkdir(join(root, "src"), { recursive: true });
  await mkdir(join(root, "config"), { recursive: true });
  await writeFile(
    join(root, "package.json"),
    JSON.stringify({
      name: "fixture",
      extends: "./not-a-typescript-config.json",
    }),
  );
  await writeFile(
    join(root, "config", "base.json"),
    JSON.stringify({
      compilerOptions: { module: "CommonJS", strict: true, target: "ES2022" },
    }),
  );
  await writeFile(
    join(root, "tsconfig.json"),
    JSON.stringify({
      extends: "./config/base.json",
      include: ["src/**/*.ts"],
    }),
  );
  await writeFile(
    join(root, "src", "fixture.ts"),
    [
      "export function helper(value: string): string { return value; }",
      'export function caller(): string { const note = "é 😀"; return helper(note); }',
      'export function alias(): string { const fn = helper; return fn("alias"); }',
      'export function dynamic(): string { const fn: any = helper; return fn("dynamic"); }',
      "export function overloaded(value: string): string;",
      "export function overloaded(value: number): number;",
      "export function overloaded(value: string | number): string | number { return value; }",
      'export function overloadCaller(): string { return overloaded("overload"); }',
      "",
    ].join("\n"),
  );

  const generator = fileURLToPath(new URL("./generate.mjs", import.meta.url));
  await execFileAsync(process.execPath, [generator, "--root", root]);
  const output = join(root, ".zvec-grep", "typescript-callfacts-v1.json");
  const first = await readFile(output);
  const artifact = JSON.parse(first);
  const calls = new Map(
    artifact.calls.map((call) => [call.target_name + call.start_byte, call]),
  );
  assert.equal(artifact.schema, "zvec-grep.typescript-callfacts");
  assert.equal(artifact.version, 1);
  assert.equal(artifact.files.length, 1);
  assert.deepEqual(
    artifact.context.context_files.map((file) => file.path),
    ["config/base.json", "package.json", "tsconfig.json"],
  );
  assert.ok([...calls.values()].some((call) => call.resolution === "static"));
  assert.ok(
    [...calls.values()].some((call) => call.resolution === "function-value"),
  );
  assert.ok(
    [...calls.values()].some((call) => call.resolution === "unresolved"),
  );
  assert.ok(
    [...calls.values()].some(
      (call) =>
        call.resolution === "ambiguous" && call.possible_targets.length > 1,
    ),
  );
  for (const call of artifact.calls) {
    const source = await readFile(join(root, call.path));
    assert.ok(
      source
        .subarray(call.start_byte, call.end_byte)
        .toString()
        .startsWith(call.target_name),
    );
    assert.ok(call.caller.start_byte <= call.start_byte);
    assert.ok(call.end_byte <= call.caller.end_byte);
  }

  await execFileAsync(process.execPath, [generator, "--root", root]);
  assert.deepEqual(await readFile(output), first);
  console.log("TypeScript callfacts producer tests: ok");
} finally {
  await rm(root, { recursive: true, force: true });
}
