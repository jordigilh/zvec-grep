import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import {
  publishIR,
  readPublishedIR,
} from "../../dist/engine/code-ir/sidecar.js";

const policy = "source-metadata-entity-v1";

test("v2 schema pins its policy without changing the v1 record contract", async () => {
  const v1 = JSON.parse(
    await readFile(
      new URL(
        "../../schemas/code-ir-projection-v1.schema.json",
        import.meta.url,
      ),
    ),
  );
  const v2 = JSON.parse(
    await readFile(
      new URL(
        "../../schemas/code-ir-projection-v2.schema.json",
        import.meta.url,
      ),
    ),
  );
  assert.equal(v1.properties.version.const, 1);
  assert.equal(v2.properties.version.const, 2);
  assert.equal(v2.properties.policy.const, policy);
  assert.ok(v2.required.includes("policy"));
  assert.deepEqual(v2.$defs.record, v1.$defs.record);
});

const fixtureRoot = resolve(
  process.env.CODE_IR_FROZEN_FIXTURES ??
    "../engram/benchmarks/semantic_search/fixtures",
);

test(
  "read-only frozen lanes retain all IR endpoints while ranking only v2-selected units",
  {
    skip:
      !process.env.CODE_IR_FROZEN_FIXTURES && !existsSync(fixtureRoot)
        ? "set CODE_IR_FROZEN_FIXTURES to replay the external fixture checkout"
        : false,
  },
  async () => {
    for (const [language, unitCount, rankedCount, excludedValues] of [
      ["go", 68, 38, 20],
      ["python", 53, 33, 12],
      ["rust", 63, 49, 0],
      ["typescript", 46, 38, 0],
    ]) {
      const fixture = resolve(fixtureRoot, `${language}-workflow-discovery-v1`);
      const manifest = JSON.parse(
        await readFile(resolve(fixture, "manifest.json")),
      );
      const files = await Promise.all(
        [...new Set(manifest.units.map((unit) => unit.path))]
          .sort()
          .map(async (path) => ({
            root_id: manifest.fixture_id,
            relative_path: path,
            language,
            bytes: await readFile(resolve(fixture, path)),
          })),
      );
      const dir = await mkdtemp(join(tmpdir(), "zg-ir-policy-frozen-"));
      try {
        await publishIR(
          dir,
          manifest.source.repository,
          files,
          "projection",
          "model",
          policy,
        );
        const { snapshot, projection } = await readPublishedIR(dir);
        const ranked = new Set(
          projection.records.map((record) => record.unit_id),
        );
        assert.equal(snapshot.units.length, unitCount, language);
        assert.equal(ranked.size, rankedCount, language);
        const excluded = snapshot.units.filter(
          (unit) =>
            unit.kind === "value" &&
            ((language === "go" && unit.subtype === "struct_field") ||
              (language === "python" && unit.subtype === "class_attribute")),
        );
        assert.equal(excluded.length, excludedValues, language);
        assert.ok(
          excluded.every((unit) => !ranked.has(unit.id)),
          language,
        );
      } finally {
        await rm(dir, { recursive: true, force: true });
      }
    }
  },
);

const cases = [
  {
    language: "go",
    path: "gadget.go",
    source:
      "package demo\n// Gadget docs\ntype Gadget struct { field string }\n// Run docs\nfunc (g *Gadget) Run() {}\n",
    symbol: "Gadget",
    expectedExcludedSubtype: "struct_field",
  },
  {
    language: "python",
    path: "gadget.py",
    source:
      "# Gadget docs\nclass Gadget:\n    field = 1\n    def run(self):\n        return self.field\n",
    symbol: "Gadget",
    expectedExcludedSubtype: "class_attribute",
  },
  {
    language: "rust",
    path: "gadget.rs",
    source:
      "/// Gadget docs\n#[derive(Clone)]\npub struct Gadget { field: i32 }\nimpl Gadget { pub fn run(&self) {} }\n",
    symbol: "Gadget",
  },
  {
    language: "typescript",
    path: "gadget.ts",
    source:
      "/** Gadget docs */\nexport class Gadget { field = 1; run(): void {} }\n",
    symbol: "Gadget",
  },
];

test("explicit v2 policy ranks selected units while preserving every semantic IR endpoint", async () => {
  for (const row of cases) {
    const dir = await mkdtemp(join(tmpdir(), "zg-ir-policy-v2-"));
    try {
      const bytes = Buffer.from(row.source);
      const input = [
        {
          root_id: "main",
          relative_path: row.path,
          language: row.language,
          bytes,
        },
      ];
      const legacy = await publishIR(dir, "demo", input, "projection", "model");
      const generation = join(dir, "generations", legacy.ir_snapshot_id);
      const v1File = (await readdir(generation)).find((name) =>
        name.startsWith("projection-v1-"),
      );
      const v1Bytes = await readFile(join(generation, v1File));
      const old = await readPublishedIR(dir);

      const active = await publishIR(
        dir,
        "demo",
        input,
        "projection",
        "model",
        policy,
      );
      const current = await readPublishedIR(dir);
      assert.equal(active.ir_snapshot_id, legacy.ir_snapshot_id, row.language);
      assert.equal(active.projection_version, 2, row.language);
      assert.equal(active.projection_policy, policy, row.language);
      assert.equal(current.projection.version, 2, row.language);
      assert.equal(current.projection.policy, policy, row.language);
      assert.deepEqual(
        current.snapshot.units,
        old.snapshot.units,
        row.language,
      );
      assert.deepEqual(
        current.snapshot.facts,
        old.snapshot.facts,
        row.language,
      );
      assert.deepEqual(await readFile(join(generation, v1File)), v1Bytes);

      const ranked = new Set(current.projection.records.map((r) => r.unit_id));
      const named = current.snapshot.units.find(
        (unit) => unit.name === row.symbol && unit.signature,
      );
      assert.ok(named?.documentation, `${row.language}: source-backed docs`);
      assert.ok(ranked.has(named.id), `${row.language}: declaration ranks`);
      const record = current.projection.records.find(
        (item) => item.unit_id === named.id,
      );
      assert.ok(
        record.lexical_text.includes(`signature: ${named.signature.text}`),
        row.language,
      );
      assert.ok(
        record.vector_input.includes(`doc: ${named.documentation.text}`),
        row.language,
      );
      for (const unit of current.snapshot.units) {
        if (unit.kind === "file" || unit.kind === "opaque") {
          assert.ok(
            !ranked.has(unit.id),
            `${row.language}: no whole-file rank`,
          );
        }
        if (unit.subtype === row.expectedExcludedSubtype) {
          assert.ok(
            !ranked.has(unit.id),
            `${row.language}: semantic-only field`,
          );
        }
      }
      if (row.expectedExcludedSubtype) {
        assert.ok(
          current.snapshot.units.some(
            (unit) => unit.subtype === row.expectedExcludedSubtype,
          ),
          `${row.language}: exclusion is exercised, not vacuous`,
        );
      }
      if (row.language === "typescript") {
        const field = current.snapshot.units.find(
          (unit) => unit.name === "field",
        );
        assert.ok(
          field && ranked.has(field.id),
          "TypeScript value remains ranked",
        );
        const fieldText = current.projection.records.find(
          (item) => item.unit_id === field.id,
        ).lexical_text;
        assert.ok(!fieldText.includes("signature:"));
        assert.ok(!fieldText.includes("doc:"));
      }
      for (const item of current.projection.records) {
        const slice = bytes.subarray(
          item.source.start_byte,
          item.source.end_byte,
        );
        assert.ok(
          item.lexical_text.endsWith(slice.toString("utf8")),
          row.language,
        );
        assert.ok(
          item.vector_input.endsWith(slice.toString("utf8")),
          row.language,
        );
        assert.ok(item.lexical_text.length <= 3_600, row.language);
        assert.ok(item.vector_input.length <= 3_600, row.language);
      }
      await publishIR(dir, "demo", input, "projection", "model");
      const reverted = await readPublishedIR(dir);
      assert.equal(reverted.projection.version, 1, row.language);
      assert.deepEqual(reverted.projection.records, old.projection.records);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  }
});

test("v2 windows cover original Unicode/CRLF bytes without citing the retrieval prefix", async () => {
  const dir = await mkdtemp(join(tmpdir(), "zg-ir-policy-unicode-"));
  try {
    const text = `/** guía 🚀 */\r\nexport function render() { return "${"😀".repeat(1_800)}"; }\r\n`;
    const bytes = Buffer.from(text);
    await publishIR(
      dir,
      "demo",
      [
        {
          root_id: "main",
          relative_path: "render.ts",
          language: "typescript",
          bytes,
        },
      ],
      "projection",
      "model",
      policy,
    );
    const { snapshot, projection } = await readPublishedIR(dir);
    const unit = snapshot.units.find(
      (candidate) => candidate.name === "render",
    );
    const windows = projection.records
      .filter((record) => record.unit_id === unit.id)
      .sort((a, b) => a.window_index - b.window_index);
    assert.ok(windows.length > 1);
    assert.ok(
      windows.every((record) =>
        record.lexical_text.includes("doc: /** guía 🚀 */"),
      ),
    );
    const raw = windows.map((window) =>
      new TextDecoder("utf8", { fatal: true }).decode(
        bytes.subarray(window.source.start_byte, window.source.end_byte),
      ),
    );
    assert.equal(
      raw.join(""),
      bytes
        .subarray(unit.source.start_byte, unit.source.end_byte)
        .toString("utf8"),
    );
    assert.ok(windows.every((record) => record.vector_input.length <= 3_600));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("v2 readback rejects a policy mismatch and excludes semantically retained fields", async () => {
  const dir = await mkdtemp(join(tmpdir(), "zg-ir-policy-tamper-"));
  try {
    const row = cases[0];
    await publishIR(
      dir,
      "demo",
      [
        {
          root_id: "main",
          relative_path: row.path,
          language: row.language,
          bytes: Buffer.from(row.source),
        },
      ],
      "projection",
      "model",
      policy,
    );
    const activePath = join(dir, "active.json");
    const active = JSON.parse(await readFile(activePath, "utf8"));
    await writeFile(
      activePath,
      JSON.stringify({ ...active, projection_policy: "other" }),
    );
    await assert.rejects(() => readPublishedIR(dir), /projection mismatch/i);
    await writeFile(activePath, JSON.stringify(active));

    const generation = join(dir, "generations", active.ir_snapshot_id);
    const v2File = (await readdir(generation)).find((name) =>
      name.startsWith("projection-v2-"),
    );
    const projectionPath = join(generation, v2File);
    const projection = JSON.parse(await readFile(projectionPath, "utf8"));
    const original = structuredClone(projection);
    const field = (await readPublishedIR(dir)).snapshot.units.find(
      (unit) => unit.subtype === "struct_field",
    );
    assert.ok(field, "Go field remains in IR");
    projection.records.push({
      ...projection.records[0],
      unit_id: field.id,
      group_id: field.id,
      unit_source: field.source,
    });
    await writeFile(projectionPath, JSON.stringify(projection));
    await assert.rejects(() => readPublishedIR(dir), /projection mismatch/i);
    original.records[0].lexical_text = `doc: invented\n${original.records[0].lexical_text}`;
    await writeFile(projectionPath, JSON.stringify(original));
    await assert.rejects(() => readPublishedIR(dir), /projection mismatch/i);
    const staleRef = structuredClone(projection);
    staleRef.records.pop();
    staleRef.records[0].source.sha256 = "0".repeat(64);
    await writeFile(projectionPath, JSON.stringify(staleRef));
    await assert.rejects(() => readPublishedIR(dir), /projection mismatch/i);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("CLI requires explicit policy only in projection mode and defaults to v1", async () => {
  const dir = await mkdtemp(join(tmpdir(), "zg-ir-policy-cli-"));
  try {
    await writeFile(join(dir, "example.ts"), "function run() {}\n");
    await writeFile(
      join(dir, "manifest.json"),
      JSON.stringify({
        fixture_id: "demo",
        language: "typescript",
        source: { repository: "demo" },
        units: [{ path: "example.ts" }],
      }),
    );
    const cli = "scripts/code-ir-snapshot.mjs";
    const base = [
      cli,
      "--manifest",
      join(dir, "manifest.json"),
      "--output",
      join(dir, "sidecar"),
    ];
    const old = JSON.parse(
      execFileSync(process.execPath, [...base, "--mode", "projection"], {
        encoding: "utf8",
      }),
    );
    assert.equal(old.projection_version, 1);
    const v2 = JSON.parse(
      execFileSync(
        process.execPath,
        [...base, "--mode", "projection", "--projection-policy", policy],
        { encoding: "utf8" },
      ),
    );
    assert.equal(v2.projection_version, 2);
    assert.throws(() =>
      execFileSync(
        process.execPath,
        [...base, "--mode", "shadow", "--projection-policy", policy],
        { stdio: "ignore" },
      ),
    );
    assert.throws(() =>
      execFileSync(
        process.execPath,
        [...base, "--mode", "projection", "--projection-policy", "unknown"],
        { stdio: "ignore" },
      ),
    );
    assert.throws(() =>
      execFileSync(
        process.execPath,
        [...base, "--mode", "projection", "--projection-policy"],
        { stdio: "ignore" },
      ),
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
