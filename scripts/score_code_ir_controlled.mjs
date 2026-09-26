// Read-only scorer for a fresh controlled search run. Raw hits may not amend truth.
import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { readPublishedIR } from "../dist/engine/code-ir/sidecar.js";
import { createWorkspaceIndexStorage } from "../dist/engine/storage/index.js";
import {
  citeControlledRun,
  groundBaseline,
  scoreControlledRun,
  verifyControlledSnapshot,
  verifyControlledTruth,
} from "./code_ir_controlled_suite.mjs";

const [fixtureArg, workArg, outputArg, model] = process.argv.slice(2);
if (!fixtureArg || !workArg || !outputArg || !model)
  throw new Error(
    "Usage: score_code_ir_controlled.mjs FIXTURE WORK OUTPUT MODEL",
  );
const fixture = resolve(fixtureArg);
const work = resolve(workArg);
const output = resolve(outputArg);
const truth = JSON.parse(await readFile(join(fixture, "truth.json"), "utf8"));
const sources = new Map(
  await Promise.all(
    Object.keys(truth.files).map(async (path) => {
      const original = await readFile(join(fixture, path));
      for (const staged of ["baseline", "candidate"]) {
        const copy = await readFile(join(work, staged, path));
        if (!copy.equals(original))
          throw new Error("staged source differs from truth");
      }
      return [path, original];
    }),
  ),
);
verifyControlledTruth(truth, sources);
const raw = JSON.parse(await readFile(join(output, "raw-runs.json"), "utf8"));
const { manifest, snapshot, projection } = await readPublishedIR(
  join(work, "ir-sidecar"),
);
if (
  !raw.provenance?.controlled_suite ||
  raw.provenance?.source_set_sha256 !== truth.source_set_sha256 ||
  raw.provenance?.model_identity !== model ||
  raw.provenance?.code_ir?.snapshot_id !== snapshot.snapshot_id ||
  raw.provenance?.code_ir?.projection_policy !== "source-metadata-entity-v1" ||
  raw.provenance?.scip !== null ||
  manifest.ir_snapshot_id !== snapshot.snapshot_id ||
  manifest.projection_model_identity !== model ||
  projection?.version !== 2
)
  throw new Error("controlled run provenance or published IR differs");
const audit = verifyControlledSnapshot(truth, sources, snapshot);
const storage = createWorkspaceIndexStorage({
  storagePath: join(work, "baseline/.zvec-grep"),
  readOnly: true,
});
let stored;
try {
  const files = storage.listFiles();
  if (
    files.length !== sources.size ||
    files.some(
      (file) =>
        file.contentHash !== truth.files[file.relativePath] ||
        file.absolutePath !== join(work, "baseline", file.relativePath),
    )
  )
    throw new Error("baseline index differs from pinned source selection");
  stored = files.flatMap((file) =>
    storage.listEntitiesByFile(file.id).map(({ entity }) => ({ file, entity })),
  );
} finally {
  storage.close();
}
const baseline = groundBaseline({ truth, sources, stored, audit });
const report = scoreControlledRun({ truth, raw, audit, baseline, projection });
citeControlledRun(report, sources, snapshot, audit);
report.provenance = {
  fixture_id: truth.fixture_id,
  source_set_sha256: truth.source_set_sha256,
  snapshot_id: snapshot.snapshot_id,
  model_identity: model,
  syntax_entities: baseline.size,
  grounded_entities: [...baseline.values()].filter((row) => row.unit_id).length,
  projected_records: projection.records.length,
  scip: null,
};
await writeFile(
  join(output, "strict-results.json"),
  JSON.stringify(report, null, 2) + "\n",
);
console.log(
  JSON.stringify({
    language: truth.language,
    aggregate: report.aggregate,
    audit: report.audit,
  }),
);
