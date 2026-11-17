#!/usr/bin/env node

import { createHash } from "node:crypto";
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import {
  existsSync,
  readdirSync,
  readFileSync,
  realpathSync,
  statSync,
} from "node:fs";
import {
  dirname,
  extname,
  isAbsolute,
  relative,
  resolve,
  sep,
} from "node:path";
import process from "node:process";
import ts from "typescript";

const SCHEMA = "zvec-grep.typescript-callfacts";
const VERSION = 1;
const OUTPUT_FILE = "typescript-callfacts-v1.json";
const SOURCE_EXTENSIONS = new Set([".ts", ".tsx"]);
const SOURCE_SKIPPED_DIRECTORIES = new Set([
  ".git",
  ".zvec-grep",
  "node_modules",
]);
const CONTEXT_SKIPPED_DIRECTORIES = new Set([
  ".git",
  ".zvec-grep",
  "node_modules",
  "target",
  "dist",
]);

function usage() {
  return `usage: node tools/typescript-callfacts/generate.mjs --root ROOT [--project TSCONFIG] [--output FILE]`;
}

function parseArgs(argv) {
  const values = { root: "", project: "", output: "" };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (
      argument === "--root" ||
      argument === "--project" ||
      argument === "--output"
    ) {
      const value = argv[index + 1];
      if (!value) throw new Error(`${argument} requires a value\n${usage()}`);
      values[argument.slice(2)] = value;
      index += 1;
    } else if (argument === "--help" || argument === "-h") {
      console.log(usage());
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${argument}\n${usage()}`);
    }
  }
  if (!values.root) throw new Error(`--root is required\n${usage()}`);
  return values;
}

function canonicalRoot(value) {
  const root = realpathSync(resolve(value));
  if (!statSync(root).isDirectory())
    throw new Error(`root is not a directory: ${root}`);
  return root;
}

function relativePath(root, value) {
  const relativeValue = relative(root, value).split(sep).join("/");
  if (
    !relativeValue ||
    relativeValue.startsWith("../") ||
    relativeValue === ".." ||
    isAbsolute(relativeValue)
  ) {
    throw new Error(`path is outside root: ${value}`);
  }
  return relativeValue;
}

function isInside(root, value) {
  try {
    relativePath(root, value);
    return true;
  } catch {
    return false;
  }
}

function walkFiles(
  root,
  predicate,
  skippedDirectories = SOURCE_SKIPPED_DIRECTORIES,
) {
  const output = [];
  const visit = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (entry.isDirectory() && skippedDirectories.has(entry.name)) continue;
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile() && predicate(path)) output.push(path);
    }
  };
  visit(root);
  return output.sort();
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function sourceFiles(root) {
  return walkFiles(root, (path) => SOURCE_EXTENSIONS.has(extname(path)));
}

function contextFiles(root, projectPath) {
  const paths = walkFiles(
    root,
    (path) => {
      const name = path.split(sep).at(-1);
      return (
        name === "package.json" ||
        name === "package-lock.json" ||
        name === "npm-shrinkwrap.json" ||
        name === "yarn.lock" ||
        name === "pnpm-lock.yaml" ||
        name === "tsconfig.json" ||
        (name.startsWith("tsconfig.") && name.endsWith(".json"))
      );
    },
    CONTEXT_SKIPPED_DIRECTORIES,
  );
  if (projectPath && !paths.includes(projectPath)) paths.push(projectPath);
  const pending = paths.filter((path) => {
    const name = path.split(sep).at(-1);
    return (
      name === "tsconfig.json" ||
      (name.startsWith("tsconfig.") && name.endsWith(".json"))
    );
  });
  for (let index = 0; index < pending.length; index += 1) {
    const configPath = pending[index];
    let config;
    try {
      config = JSON.parse(readFileSync(configPath, "utf8"));
    } catch {
      continue;
    }
    const extendsValue = config?.extends;
    if (typeof extendsValue !== "string") continue;
    if (!extendsValue.startsWith(".")) {
      throw new Error(
        `external TypeScript config extends is not attested: ${extendsValue}`,
      );
    }
    const base = resolve(dirname(configPath), extendsValue);
    const candidates = [base, `${base}.json`, resolve(base, "tsconfig.json")];
    const extended = candidates.find(
      (candidate) => existsSync(candidate) && statSync(candidate).isFile(),
    );
    if (!extended) {
      throw new Error(
        `TypeScript config extends file does not exist: ${extendsValue}`,
      );
    }
    if (!isInside(root, extended)) {
      throw new Error(
        `TypeScript config extends outside root: ${extendsValue}`,
      );
    }
    const canonical = realpathSync(extended);
    if (!pending.includes(canonical)) {
      pending.push(canonical);
      paths.push(canonical);
    }
  }
  return paths.sort();
}

async function hashedFiles(root, paths) {
  const files = [];
  for (const path of paths) {
    const bytes = await readFile(path);
    files.push({ path: relativePath(root, path), sha256: sha256(bytes) });
  }
  return files.sort((left, right) => left.path.localeCompare(right.path));
}

function enumName(enumObject, value, fallback = "unknown") {
  if (typeof value !== "number") return fallback;
  return enumObject[value] ?? fallback;
}

function stableJson(value) {
  if (value === undefined) return "null";
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`)
    .join(",")}}`;
}

function hashPart(hash, value) {
  hash.update(String(value));
  hash.update(Buffer.from([0]));
}

function contextFingerprint(context) {
  const hash = createHash("sha256");
  hashPart(hash, "zvec-grep.typescript-callfacts-context-v1");
  hashPart(hash, context.typescript_version);
  hashPart(hash, context.node_version);
  hashPart(hash, context.project_path);
  hashPart(hash, context.target);
  hashPart(hash, context.module);
  hashPart(hash, context.jsx);
  for (const name of Object.keys(context.settings).sort()) {
    hashPart(hash, name);
    hashPart(hash, context.settings[name]);
  }
  for (const file of [...context.context_files].sort((left, right) =>
    left.path.localeCompare(right.path),
  )) {
    hashPart(hash, file.path);
    hashPart(hash, file.sha256);
  }
  return hash.digest("hex");
}

function sourcePosition(sourceFile, position) {
  const lineAndCharacter = ts.getLineAndCharacterOfPosition(
    sourceFile,
    position,
  );
  const lineStart = sourceFile.getLineStarts()[lineAndCharacter.line];
  const lineText = sourceFile.text.slice(lineStart, position);
  return {
    byte: Buffer.byteLength(sourceFile.text.slice(0, position), "utf8"),
    line: lineAndCharacter.line + 1,
    column: Buffer.byteLength(lineText, "utf8"),
  };
}

function rangeFor(sourceFile, start, end) {
  const begin = sourcePosition(sourceFile, start);
  const finish = sourcePosition(sourceFile, end);
  return {
    start_byte: begin.byte,
    end_byte: finish.byte,
    start_line: begin.line,
    end_line: finish.line,
    start_column: begin.column,
    end_column: finish.column,
  };
}

function symbolFor(root, declaration) {
  if (!declaration || !declaration.getSourceFile) return null;
  const sourceFile = declaration.getSourceFile();
  const absolute = resolve(sourceFile.fileName);
  if (!isInside(root, absolute) || !SOURCE_EXTENSIONS.has(extname(absolute)))
    return null;
  const start = declaration.getStart(sourceFile);
  const end = declaration.getEnd();
  if (start >= end) return null;
  return {
    path: relativePath(root, absolute),
    start_byte: Buffer.byteLength(sourceFile.text.slice(0, start), "utf8"),
    end_byte: Buffer.byteLength(sourceFile.text.slice(0, end), "utf8"),
  };
}

function supportedDeclaration(declaration) {
  return (
    ts.isFunctionDeclaration(declaration) ||
    ts.isMethodDeclaration(declaration) ||
    ts.isConstructorDeclaration(declaration)
  );
}

function enclosingCaller(root, node) {
  for (let current = node.parent; current; current = current.parent) {
    if (supportedDeclaration(current)) return symbolFor(root, current);
  }
  return null;
}

function declarationKey(symbol) {
  return `${symbol.path}:${symbol.start_byte}:${symbol.end_byte}`;
}

function uniqueSymbols(symbols) {
  const byKey = new Map();
  for (const symbol of symbols.filter(Boolean))
    byKey.set(declarationKey(symbol), symbol);
  return [...byKey.values()].sort((left, right) =>
    declarationKey(left).localeCompare(declarationKey(right)),
  );
}

function symbolAtDeclaration(checker, expression) {
  let symbol = checker.getSymbolAtLocation(expression);
  if (symbol && symbol.flags & ts.SymbolFlags.Alias) {
    try {
      symbol = checker.getAliasedSymbol(symbol);
    } catch {
      return null;
    }
  }
  return symbol;
}

function directCallableReference(checker, expression) {
  let reference = expression;
  while (ts.isParenthesizedExpression(reference))
    reference = reference.expression;
  if (
    !ts.isIdentifier(reference) &&
    !ts.isPropertyAccessExpression(reference)
  ) {
    return false;
  }
  const symbol = symbolAtDeclaration(checker, reference);
  return Boolean(
    symbol?.declarations?.some((declaration) =>
      supportedDeclaration(declaration),
    ),
  );
}

function localDeclarations(root, checker, call) {
  const declarations = [];
  const signature = checker.getResolvedSignature(call);
  if (signature?.declaration) declarations.push(signature.declaration);
  const symbol = symbolAtDeclaration(checker, call.expression);
  if (symbol) declarations.push(...(symbol.declarations ?? []));
  let type;
  try {
    type = checker.getTypeAtLocation(call.expression);
    for (const candidate of checker.getSignaturesOfType(
      type,
      ts.SignatureKind.Call,
    )) {
      if (candidate.declaration) declarations.push(candidate.declaration);
    }
  } catch {
    type = undefined;
  }
  const local = uniqueSymbols(
    declarations
      .filter((declaration) => supportedDeclaration(declaration))
      .map((declaration) => symbolFor(root, declaration)),
  );
  const external = declarations.some((declaration) => {
    const sourceFile = declaration.getSourceFile?.();
    return sourceFile && !isInside(root, resolve(sourceFile.fileName));
  });
  const isUnknown =
    type &&
    (type.flags &
      (ts.TypeFlags.Any | ts.TypeFlags.Unknown | ts.TypeFlags.Never)) !==
      0;
  const isUnion = Boolean(type?.isUnion?.());
  return { local, external, isUnknown, isUnion, signature };
}

function targetName(sourceFile, expression, checker) {
  const text = expression.getText(sourceFile).trim();
  const match = text.match(/[A-Za-z_$][\w$]*$/u);
  if (match) return match[0];
  const symbol = symbolAtDeclaration(checker, expression);
  return symbol?.getName() || text || "<call>";
}

function classifyCall(root, checker, sourceFile, call) {
  const { local, external, isUnknown, isUnion, signature } = localDeclarations(
    root,
    checker,
    call,
  );
  const name = targetName(sourceFile, call.expression, checker);
  const direct = directCallableReference(checker, call.expression);
  if (isUnknown) {
    return {
      target_name: name,
      target: null,
      possible_targets: [],
      resolution: "unresolved",
    };
  }
  if (local.length === 1 && !isUnion && signature?.declaration && direct) {
    return {
      target_name: name,
      target: local[0],
      possible_targets: [],
      resolution: "static",
    };
  }
  if (local.length > 1 || isUnion) {
    return {
      target_name: name,
      target: null,
      possible_targets: local,
      resolution: local.length > 1 ? "ambiguous" : "possible",
    };
  }
  if (!direct && signature?.declaration) {
    return {
      target_name: name,
      target: null,
      possible_targets: [],
      resolution: "function-value",
    };
  }
  return {
    target_name: name,
    target: null,
    possible_targets: [],
    resolution: external
      ? "external"
      : signature?.declaration
        ? "possible"
        : "function-value",
  };
}

function collectCalls(root, checker, sourceFile) {
  const calls = [];
  const visit = (node) => {
    if (ts.isCallExpression(node)) {
      const caller = enclosingCaller(root, node);
      if (caller) {
        const range = rangeFor(
          sourceFile,
          node.getStart(sourceFile),
          node.getEnd(),
        );
        const classification = classifyCall(root, checker, sourceFile, node);
        calls.push({
          path: relativePath(root, resolve(sourceFile.fileName)),
          ...range,
          caller,
          ...classification,
        });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sourceFile);
  return calls;
}

function parseProject(root, project) {
  const projectPath = project
    ? resolve(root, project)
    : resolve(root, "tsconfig.json");
  if (!existsSync(projectPath))
    throw new Error(`TypeScript project file does not exist: ${projectPath}`);
  if (!isInside(root, projectPath))
    throw new Error(
      `TypeScript project file must be under root: ${projectPath}`,
    );
  const projectName = projectPath.split(sep).at(-1);
  if (
    projectName !== "tsconfig.json" &&
    !(projectName.startsWith("tsconfig.") && projectName.endsWith(".json"))
  ) {
    throw new Error(
      `TypeScript project file must be named tsconfig*.json: ${projectPath}`,
    );
  }
  const config = ts.readConfigFile(projectPath, ts.sys.readFile);
  if (config.error)
    throw new Error(
      ts.flattenDiagnosticMessageText(config.error.messageText, "\n"),
    );
  const parsed = ts.parseJsonConfigFileContent(
    config.config,
    ts.sys,
    dirname(projectPath),
    undefined,
    projectPath,
  );
  if (parsed.errors.length) {
    throw new Error(
      parsed.errors
        .map((error) =>
          ts.flattenDiagnosticMessageText(error.messageText, "\n"),
        )
        .join("\n"),
    );
  }
  return { projectPath, config, parsed };
}

function compilerContext(root, projectPath, config, parsed, contextFileList) {
  const options = parsed.options;
  const settings = {
    compiler_options: stableJson(config.config.compilerOptions ?? {}),
    module_resolution: enumName(
      ts.ModuleResolutionKind,
      options.moduleResolution,
    ),
  };
  return {
    typescript_version: ts.version,
    node_version: process.version,
    project_path: relativePath(root, projectPath),
    target: enumName(ts.ScriptTarget, options.target),
    module: enumName(ts.ModuleKind, options.module),
    jsx: enumName(ts.JsxEmit, options.jsx),
    settings,
    context_files: contextFileList,
  };
}

async function generate(values) {
  const root = canonicalRoot(values.root);
  const { projectPath, config, parsed } = parseProject(root, values.project);
  const sourcePaths = sourceFiles(root);
  const contextPathList = contextFiles(root, projectPath);
  const files = await hashedFiles(root, sourcePaths);
  const contextFileList = await hashedFiles(root, contextPathList);
  const context = compilerContext(
    root,
    projectPath,
    config,
    parsed,
    contextFileList,
  );
  const program = ts.createProgram({
    rootNames: parsed.fileNames,
    options: parsed.options,
    projectReferences: parsed.projectReferences,
  });
  const checker = program.getTypeChecker();
  const sourceSet = new Set(
    sourcePaths.map((path) => relativePath(root, path)),
  );
  const calls = [];
  for (const sourceFile of program.getSourceFiles()) {
    const absolute = resolve(sourceFile.fileName);
    if (!isInside(root, absolute) || !SOURCE_EXTENSIONS.has(extname(absolute)))
      continue;
    if (!sourceSet.has(relativePath(root, absolute))) continue;
    calls.push(...collectCalls(root, checker, sourceFile));
  }
  calls.sort((left, right) =>
    `${left.path}:${left.start_byte}:${left.end_byte}`.localeCompare(
      `${right.path}:${right.start_byte}:${right.end_byte}`,
    ),
  );
  const artifact = {
    schema: SCHEMA,
    version: VERSION,
    context,
    context_sha256: contextFingerprint(context),
    files,
    calls,
  };
  const output = values.output
    ? resolve(root, values.output)
    : resolve(root, ".zvec-grep", OUTPUT_FILE);
  if (!isInside(root, output))
    throw new Error(`output must be under root: ${output}`);
  await mkdir(dirname(output), { recursive: true });
  const temporary = `${output}.tmp-${process.pid}`;
  await writeFile(temporary, `${JSON.stringify(artifact)}\n`, "utf8");
  await rename(temporary, output);
  return {
    output,
    calls: calls.length,
    files: files.length,
    context_sha256: artifact.context_sha256,
  };
}

try {
  const result = await generate(parseArgs(process.argv.slice(2)));
  console.error(
    `TypeScript callfacts: ${result.calls} calls, ${result.files} files -> ${result.output} (${result.context_sha256})`,
  );
} catch (error) {
  console.error(
    `typescript-callfacts: ${error instanceof Error ? error.message : String(error)}`,
  );
  process.exitCode = 1;
}
