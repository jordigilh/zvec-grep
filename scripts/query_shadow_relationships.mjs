#!/usr/bin/env node
// Optional offline navigation: resolve a known Code IR unit or exact reference
// site against one pinned SCIP-enriched shadow snapshot.
import { openShadowRelationships } from "./shadow_ir_relationships.mjs";

const args = new Map();
const allowed = new Set(["--root", "--manifest", "--shadow", "--scip-index", "--config",
  "--shadow-sha256", "--source-set-sha256", "--config-sha256", "--module",
  "--module-sha256", "--implementation-sha256", "--producer-binary",
  "--producer-binary-sha256", "--site-path", "--site-start-byte", "--site-end-byte",
  "--unit-id", "--path", "--symbol", "--limit"]);
for (let index = 2; index < process.argv.length; index += 2) {
  const flag = process.argv[index];
  const value = process.argv[index + 1];
  if (!allowed.has(flag) || !value || args.has(flag))
    throw new Error(`invalid or repeated option: ${flag}`);
  args.set(flag, value);
}
function required(flag) {
  const value = args.get(flag);
  if (!value) throw new Error(`missing ${flag}`);
  return value;
}
const index = await openShadowRelationships({
  root: required("--root"),
  manifestPath: required("--manifest"),
  shadowPath: required("--shadow"),
  indexPath: required("--scip-index"),
  configRelativePath: required("--config"),
  expectedShadowSha256: required("--shadow-sha256"),
  expectedSourceSetSha256: required("--source-set-sha256"),
  expectedConfigSha256: required("--config-sha256"),
  modulePath: required("--module"),
  expectedModuleSha256: required("--module-sha256"),
  expectedImplementationSha256: required("--implementation-sha256"),
  producerBinaryPath: args.get("--producer-binary"),
  expectedBinarySha256: args.get("--producer-binary-sha256"),
});
let result;
if (args.has("--site-end-byte") && !args.has("--site-path"))
  throw new Error("--site-end-byte requires a reference site selector");
if (args.has("--site-path") || args.has("--site-start-byte")) {
  if (args.has("--unit-id") || args.has("--path") || args.has("--symbol"))
    throw new Error("select a reference site or a unit, not both");
  result = index.querySite({
    path: required("--site-path"),
    startByte: Number(required("--site-start-byte")),
    ...(args.has("--site-end-byte") ? { endByte: Number(args.get("--site-end-byte")) } : {}),
  });
} else {
  result = index.queryUnit({
    unitId: args.get("--unit-id"),
    path: args.get("--path"),
    symbol: args.get("--symbol"),
    ...(args.has("--limit") ? { limit: Number(args.get("--limit")) } : {}),
  });
}
process.stdout.write(JSON.stringify(result, null, 2) + "\n");
