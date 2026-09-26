import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  publishIR,
  readPublishedIR,
} from "../../dist/engine/code-ir/sidecar.js";

test("IR sidecar publishes immutable generations with a pinned manifest and rejects stale records", async () => {
  const dir = await mkdtemp(join(tmpdir(), "zg-ir-"));
  try {
    const input = (text) => [
      {
        root_id: "main",
        relative_path: "add.ts",
        language: "typescript",
        bytes: Buffer.from(text),
      },
    ];
    assert.equal(
      await publishIR(dir, "demo", input("function add() {}"), "off"),
      null,
    );
    const first = await publishIR(
      dir,
      "demo",
      input("function add() {}"),
      "shadow",
    );
    assert.equal((await readPublishedIR(dir)).projection, undefined);
    const second = await publishIR(
      dir,
      "demo",
      input("function add() { save(); }"),
      "projection",
      "local/potion-code-16m-v2",
    );
    assert.notEqual(first.ir_snapshot_id, second.ir_snapshot_id);
    const read = await readPublishedIR(dir);
    assert.equal(read.projection.ir_snapshot_id, read.snapshot.snapshot_id);
    const record = read.projection.records.find((r) =>
      r.lexical_text.includes("save"),
    );
    assert.ok(record.vector_input.endsWith("function add() { save(); }"));
    assert.equal(record.source.start_byte, 0);
    assert.equal(
      read.manifest.projection_model_identity,
      "local/potion-code-16m-v2",
    );
    const active = join(dir, "active.json");
    const manifest = JSON.parse(await readFile(active, "utf8"));
    await writeFile(
      active,
      JSON.stringify({ ...manifest, ir_snapshot_id: first.ir_snapshot_id }),
    );
    await assert.rejects(() => readPublishedIR(dir));
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("candidate projection windows round-trip exact Unicode source and split deterministically", async () => {
  const dir = await mkdtemp(join(tmpdir(), "zg-ir-windows-"));
  try {
    const text = `function render() { return "${"😀".repeat(1_800)}"; }\r\n`;
    const files = [
      {
        root_id: "main",
        relative_path: "render.ts",
        language: "typescript",
        bytes: Buffer.from(text),
      },
    ];
    await publishIR(dir, "demo", files, "projection", "model-a");
    const first = await readPublishedIR(dir);
    const unit = first.snapshot.units.find(
      (candidate) => candidate.name === "render",
    );
    const windows = first.projection.records
      .filter((record) => record.unit_id === unit.id)
      .sort((a, b) => a.window_index - b.window_index);
    assert.ok(windows.length > 1);
    assert.equal(windows[0].source.start_byte, unit.source.start_byte);
    assert.equal(windows.at(-1).source.end_byte, unit.source.end_byte);
    const slices = windows.map((window) => {
      const bytes = files[0].bytes.subarray(
        window.source.start_byte,
        window.source.end_byte,
      );
      const source = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      assert.ok(window.vector_input.endsWith(source));
      assert.ok(window.vector_input.length <= 3_600);
      assert.ok(window.lexical_text.length <= 3_600);
      return source;
    });
    const roundTrip = slices.join("");
    assert.equal(
      roundTrip,
      Buffer.from(text)
        .subarray(unit.source.start_byte, unit.source.end_byte)
        .toString("utf8"),
    );
    await publishIR(dir, "demo", files, "projection", "model-b");
    assert.equal(
      (await readPublishedIR(dir)).manifest.projection_model_identity,
      "model-b",
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
