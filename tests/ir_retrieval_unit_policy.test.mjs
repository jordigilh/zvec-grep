import assert from "node:assert/strict";
import { test } from "node:test";
import { applyRetrievalUnitPolicy } from "../scripts/ir_retrieval_unit_policy.mjs";

test("fine-grained semantic targets remain in IR but need not be standalone retrieval hits", () => {
  const units = [
    { id: "type", kind: "type", origin: { language: "go" }, subtype: "class" },
    { id: "field", kind: "value", origin: { language: "go" }, subtype: "struct_field" },
    { id: "method", kind: "method", origin: { language: "go" }, subtype: "method_declaration" },
    { id: "class", kind: "type", origin: { language: "python" }, subtype: "class" },
    { id: "attribute", kind: "value", origin: { language: "python" }, subtype: "class_attribute" },
    { id: "ts-field", kind: "value", origin: { language: "typescript" }, subtype: "public_field_definition" },
    { id: "file", kind: "file", origin: { language: "go" } },
  ];
  const records = units.map((unit) => ({ unit_id: unit.id }));
  const complete = applyRetrievalUnitPolicy(records, units, "all");
  assert.deepEqual(complete.records.map((record) => record.unit_id), ["type", "field", "method", "class", "attribute", "ts-field"]);
  const filtered = applyRetrievalUnitPolicy(records, units, "legacy-entity-parity");
  assert.deepEqual(filtered.records.map((record) => record.unit_id), ["type", "method", "class", "ts-field"]);
  assert.deepEqual(filtered.excluded, { go_struct_fields: 1, python_class_attributes: 1 });
  assert.equal(units.length, 7); // The semantic IR still retains field/attribute endpoints.
});

test("unknown source units and policy names fail closed", () => {
  assert.throws(() => applyRetrievalUnitPolicy([{ unit_id: "missing" }], [], "all"), /missing unit/u);
  assert.throws(() => applyRetrievalUnitPolicy([], [], "unknown"), /unsupported IR retrieval policy/u);
});
