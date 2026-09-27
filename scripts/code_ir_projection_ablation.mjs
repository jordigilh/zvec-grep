// Pure, fail-closed 2x2 retrieval-view experiment. Inputs MUST be published,
// read-validated v1 and v2 sidecars for the same source/model snapshot.
// No source bytes, qrels or canonical IR records are edited here.
const v2Policy = "source-metadata-entity-v1";

function excludedByV2(unit) {
  return (
    (unit.kind === "value" &&
      unit.origin.language === "go" &&
      unit.subtype === "struct_field") ||
    (unit.kind === "value" &&
      unit.origin.language === "python" &&
      unit.subtype === "class_attribute")
  );
}

function key(record) {
  return `${record.unit_id}\0${record.window_index}`;
}

export function buildProjectionAblations(snapshot, v1, v2) {
  if (
    v1.version !== 1 ||
    v2.version !== 2 ||
    v1.policy !== undefined ||
    v2.policy !== v2Policy ||
    v1.ir_snapshot_id !== snapshot.snapshot_id ||
    v2.ir_snapshot_id !== snapshot.snapshot_id ||
    !v1.model_identity ||
    v1.model_identity !== v2.model_identity
  )
    throw new Error(
      "ablation requires matching validated v1/v2 snapshots and model",
    );

  const units = new Map(snapshot.units.map((unit) => [unit.id, unit]));
  if (units.size !== snapshot.units.length)
    throw new Error("ablation has duplicate snapshot unit IDs");
  const eligible = v1.records.filter((record) => {
    const unit = units.get(record.unit_id);
    if (!unit) throw new Error(`unknown v1 unit ${record.unit_id}`);
    return unit.kind !== "file" && unit.kind !== "opaque";
  });
  const filtered = eligible.filter(
    (record) => !excludedByV2(units.get(record.unit_id)),
  );
  const v1Keys = new Set(v1.records.map(key));
  if (v1Keys.size !== v1.records.length) throw new Error("duplicate v1 window");
  const selected = new Map();
  for (const record of v2.records) {
    const unit = units.get(record.unit_id);
    if (
      !unit ||
      unit.kind === "file" ||
      unit.kind === "opaque" ||
      excludedByV2(unit)
    )
      throw new Error(`invalid v2 standalone unit: ${record.unit_id}`);
    if (selected.has(key(record))) throw new Error("duplicate v2 window");
    selected.set(key(record), record);
  }
  if (
    filtered.length !== v2.records.length ||
    filtered.some((left, index) => {
      const right = v2.records[index];
      return (
        key(left) !== key(right) ||
        left.group_id !== right.group_id ||
        JSON.stringify(left.source) !== JSON.stringify(right.source) ||
        JSON.stringify(left.unit_source) !== JSON.stringify(right.unit_source)
      );
    })
  )
    throw new Error(
      "ablation v1/v2 unit/window refs differ; cannot isolate text policy",
    );

  return [
    { name: "code-ir-v1", records: eligible },
    { name: "code-ir-policy-only", records: filtered },
    {
      name: "code-ir-metadata-only",
      records: eligible.map((record) => selected.get(key(record)) ?? record),
    },
    { name: "code-ir-v2", records: v2.records },
  ];
}
