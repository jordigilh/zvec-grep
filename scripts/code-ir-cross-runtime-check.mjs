import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { validateSnapshot } from "../dist/engine/code-ir/index.js";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const output = execFileSync(
  "cargo",
  ["run", "--quiet", "-p", "zg-code-ir", "--example", "dump_snapshot"],
  { cwd: resolve(root, "rust"), encoding: "utf8" },
);
const { source, snapshot } = JSON.parse(output);
const bytes = Buffer.from(source, "utf8");
const file = snapshot.files[0];
validateSnapshot(snapshot, new Map([[file.file_id, bytes]]));
for (const unit of snapshot.units) {
  const actual = bytes
    .subarray(unit.source.start_byte, unit.source.end_byte)
    .toString("utf8");
  if (!actual) throw new Error(`empty Rust-produced source slice ${unit.id}`);
}
console.log(
  JSON.stringify({
    validated: true,
    producer: "Rust zg-code-ir",
    consumer: "TypeScript validator",
    snapshot_id: snapshot.snapshot_id,
    units: snapshot.units.length,
    facts: snapshot.facts.length,
  }),
);
