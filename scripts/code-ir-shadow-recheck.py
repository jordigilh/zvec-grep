"""Read-only, fail-closed replay of the independent semantic producer join.

Used by code-ir-shadow-recheck.mjs. No files are written in the spike checkout.
The separate spike's source-set/config checks and strict span+kind join remain
the authority for this diagnostic; candidates never enter production search.
"""

import copy
import hashlib
import json
import pathlib
import sys

mode, root_arg, language, spike_arg = sys.argv[1:5]
root = pathlib.Path(root_arg).resolve(strict=True)
sys.path.insert(0, spike_arg)
from scripts.lsp_ir_shadow_join import (KIND, LABELS, anchor, shadow_join,
                                        source_ref, strict_symbol_binding)  # noqa: E402

ir_record = json.load(sys.stdin)
artifact_sha256 = None
if mode == "lsp":
    artifact = (root / f"lsp-full-{language}.json").read_bytes()
    artifact_sha256 = hashlib.sha256(artifact).hexdigest()
    record = json.loads(artifact)
elif mode == "scip-go" and language == "go":
    sys.path.insert(0, str(root))
    import scip_pb2  # noqa: E402
    from scripts.scip_compat_reassessment import normalize  # noqa: E402

    index = scip_pb2.Index()
    artifact = (root / "go" / "index.scip").read_bytes()
    artifact_sha256 = hashlib.sha256(artifact).hexdigest()
    expected = next((arg.split(":", 1)[1] for arg in sys.argv[5:]
                     if arg.startswith("sha256:")), None)
    if expected and expected != artifact_sha256:
        raise ValueError("SCIP artifact differs from pinned producer output")
    index.ParseFromString(artifact)
    record, producer_counts = normalize(root, language, index, ir_record, scip_pb2)
else:
    raise ValueError(f"unsupported diagnostic mode: {mode}/{language}")

enriched, counts = shadow_join(root / language, language, record, ir_record)
files = {f["relative_path"]: f for f in ir_record["snapshot"]["files"]}


def authored_matches(shadow):
    labels = json.loads(LABELS.read_text())[language]
    paths = {file["file_id"]: file["relative_path"] for file in shadow["files"]}
    units = {unit["id"]: unit for unit in shadow["units"]}
    correct = 0
    for row in labels:
        site, target = anchor(root / language, row["site"]), anchor(root / language, row["definition"])

        def offset(location):
            lines = (root / language / location["path"]).read_bytes().splitlines(keepends=True)
            return sum(map(len, lines[:location["line"]])) + location["start"]

        correct += any(fact["kind"] == "references" and
                       paths[fact["site"]["file_id"]] == site["path"] and
                       fact["site"]["start_byte"] == offset(site) and
                       fact.get("object_id") in units and
                       paths[units[fact["object_id"]]["source"]["file_id"]] == target["path"] and
                       units[fact["object_id"]]["source"]["start_byte"] <= offset(target) <
                       units[fact["object_id"]]["source"]["end_byte"] for fact in shadow["facts"])
    return [correct, len(labels)]


alternate = None
if len(sys.argv) > 5 and sys.argv[5] == "aliases" and mode == "lsp":
    alternate_record = copy.deepcopy(record)
    resolved = []
    for symbol in alternate_record["symbols"]:
        token = symbol["token"]
        file = files[token["path"]]
        raw = (root / language / token["path"]).read_bytes()
        candidates = []
        for unit in ir_record["snapshot"]["units"]:
            if (unit["source"]["file_id"] != file["file_id"] or
                unit.get("name") != token["slice"] or unit["kind"] != KIND.get(symbol["kind"])):
                continue
            extension = unit.get("extensions", {}).get("data", {})
            if language == "go" and unit["kind"] == "type":
                key = "syntax_source"
            elif language == "python" and unit.get("subtype") == "class_attribute":
                key = "identifier_source"
            else:
                continue
            alias = extension.get(key)
            if alias is None or [alias["start_byte"], alias["end_byte"]] != [symbol["body"]["start_byte"], symbol["body"]["end_byte"]]:
                continue
            if (alias != source_ref(file, raw, alias["start_byte"], alias["end_byte"]) or
                alias["file_id"] != unit["source"]["file_id"] or
                not unit["source"]["start_byte"] <= alias["start_byte"] < alias["end_byte"] <= unit["source"]["end_byte"] or
                not unit["source"]["start_byte"] <= token["start_byte"] < token["end_byte"] <= unit["source"]["end_byte"] or
                raw[token["start_byte"]:token["end_byte"]].decode("utf-8") != unit["name"]):
                continue
            if key == "identifier_source" and [token["start_byte"], token["end_byte"]] != [alias["start_byte"], alias["end_byte"]]:
                continue
            candidates.append((unit, key))
        if len(candidates) == 1:
            unit, key = candidates[0]
            symbol["body"] = {"start_byte": unit["source"]["start_byte"],
                              "end_byte": unit["source"]["end_byte"]}
            resolved.append({"name": token["slice"], "kind": key, "path": token["path"]})
    alternate_snapshot, alternate_counts = shadow_join(root / language, language, alternate_record, ir_record)
    alternate = {"snapshot": alternate_snapshot, "counts": alternate_counts,
                 "source_verified_aliases": resolved, "authored_matches": authored_matches(alternate_snapshot)}

unmatched = []
for symbol in record["symbols"]:
    _, status = strict_symbol_binding(symbol, ir_record["snapshot"]["units"], root / language, files)
    if status != "strict":
        token = symbol["token"]
        candidates = [u for u in ir_record["snapshot"]["units"]
                      if u["source"]["file_id"] == files[token["path"]]["file_id"]
                      and u.get("name") == token["slice"]]
        if any(u.get("subtype") == "class_attribute" for u in candidates) or (
            language == "go" and any(u["kind"] == "type" for u in candidates)):
            unmatched.append({"name": token["slice"], "status": status, "kind": symbol["kind"],
                              "path": token["path"], "body": symbol["body"],
                              "candidate_units": [{"kind": u["kind"], "subtype": u.get("subtype"),
                                                   "source": [u["source"]["start_byte"], u["source"]["end_byte"]],
                                                   "identifier": u.get("extensions", {}).get("data", {}).get("identifier_source")}
                                                  for u in candidates]})
print(json.dumps({"snapshot": enriched, "counts": counts,
                  "producer_artifact_sha256": artifact_sha256,
                  "producer_counts": dict(producer_counts) if mode == "scip-go" else None,
                  "unmatched_focus": unmatched, "authored_matches": authored_matches(enriched),
                  "alternate": alternate}))
