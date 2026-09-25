// Code IR carries fine-grained semantic endpoints independently of which units
// are indexed as standalone answerable search results. This policy is an
// experiment to match the previous extractor's retrieval granularity.
export function applyRetrievalUnitPolicy(records, units, policy) {
  if (policy !== "all" && policy !== "legacy-entity-parity")
    throw new Error(`unsupported IR retrieval policy: ${policy}`);
  const byId = new Map(units.map((unit) => [unit.id, unit]));
  const excluded = { go_struct_fields: 0, python_class_attributes: 0 };
  const filtered = records.filter((record) => {
    const unit = byId.get(record.unit_id);
    if (!unit) throw new Error(`IR projection refers to a missing unit: ${record.unit_id}`);
    if (unit.kind === "file" || unit.kind === "opaque") return false;
    if (policy === "legacy-entity-parity" && unit.kind === "value") {
      if (unit.origin.language === "go" && unit.subtype === "struct_field") {
        excluded.go_struct_fields++;
        return false;
      }
      if (unit.origin.language === "python" && unit.subtype === "class_attribute") {
        excluded.python_class_attributes++;
        return false;
      }
    }
    return true;
  });
  return { records: filtered, excluded };
}
