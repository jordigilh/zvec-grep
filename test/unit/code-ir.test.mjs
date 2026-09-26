import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import {
  extractSnapshot,
  snapshotIdentity,
  validateSnapshot,
} from "../../dist/engine/code-ir/index.js";

test("production-schema conformance examples serialize identically and validate in TypeScript", async () => {
  const fixture = JSON.parse(
    await readFile(
      new URL("../fixtures/code-ir-v1/examples.json", import.meta.url),
      "utf8",
    ),
  );
  assert.equal(fixture.schema, "zvec-grep.code-ir.conformance-examples");
  for (const example of fixture.examples) {
    const { snapshot, sources } = await extractSnapshot("demo", [
      {
        root_id: "demo",
        relative_path: example.relative_path,
        language: example.language,
        bytes: Buffer.from(example.source, "utf8"),
      },
    ]);
    const expected = structuredClone(example.snapshot);
    const frontend = snapshot.frontend_versions[example.language];
    assert.equal(frontend, "web-tree-sitter-ir-v1.6");
    expected.frontend_versions[example.language] = frontend;
    for (const file of expected.files)
      file.extraction.frontend_version = frontend;
    for (const unit of expected.units) unit.origin.frontend = frontend;
    for (const fact of expected.facts) fact.provenance.frontend = frontend;
    expected.snapshot_id = snapshotIdentity(expected);
    assert.deepEqual(snapshot, expected, example.language);
    validateSnapshot(JSON.parse(JSON.stringify(example.snapshot)), sources);
  }
});

const examples = [
  ["go", "package demo\nfunc add() { save() }\n", 13, 34, 26, 32],
  ["python", "def add():\n    save()\n", 0, 21, 15, 21],
  ["rust", "fn add() { save(); }\n", 0, 20, 11, 17],
  ["typescript", "function add() { save(); }\n", 0, 26, 17, 23],
];

test("four language serialized source maps match the independently specified mini-files", async () => {
  for (const [language, text, start, end, callStart, callEnd] of examples) {
    const { snapshot, sources } = await extractSnapshot("demo", [
      {
        root_id: "root",
        relative_path: `add.${language}`,
        language,
        bytes: Buffer.from(text),
      },
    ]);
    const decoded = JSON.parse(JSON.stringify(snapshot));
    validateSnapshot(decoded, sources);
    const add = decoded.units.find((unit) => unit.name === "add");
    const call = decoded.facts.find((fact) => fact.kind === "calls");
    assert.deepEqual(
      [
        add.source.start_byte,
        add.source.end_byte,
        call.site.start_byte,
        call.site.end_byte,
      ],
      [start, end, callStart, callEnd],
    );
    assert.equal(call.subject_id, add.id);
    assert.equal(call.status, "unresolved");
    assert.equal(call.target_spelling, "save");
    const corrupted = structuredClone(decoded);
    corrupted.facts[0].status = "type_resolved";
    assert.throws(() => validateSnapshot(corrupted, sources));
    corrupted.facts[0].status = "observed";
    corrupted.files[0].sha256 = "0".repeat(64);
    assert.throws(() => validateSnapshot(corrupted, sources), /stale/);
    assert.throws(
      () =>
        validateSnapshot(
          decoded,
          new Map([[decoded.files[0].file_id, Buffer.from("changed")]]),
        ),
      /stale/,
    );
    assert.throws(
      () => validateSnapshot({ ...decoded, schema_version: 2 }, sources),
      /version/,
    );
  }
});

test("UTF-8, CRLF, overload identity, uncertain syntax and unsupported bytes", async () => {
  const text =
    "// é 🚀\r\nfunction add() { save(); }\r\nfunction add(x: number) { save(); }\r\n";
  const { snapshot, sources } = await extractSnapshot("demo", [
    {
      root_id: "root",
      relative_path: "add.ts",
      language: "typescript",
      bytes: Buffer.from(text),
    },
  ]);
  const adds = snapshot.units.filter((unit) => unit.name === "add");
  assert.equal(adds.length, 2);
  assert.notEqual(adds[0].id, adds[1].id);
  assert.equal(adds[0].source.start_byte, Buffer.byteLength("// é 🚀\r\n"));
  assert.equal(adds[0].source.start.column_byte, 0);
  assert.equal(
    snapshot.facts.filter((fact) => fact.kind === "calls").length,
    2,
  );
  const duplicate = structuredClone(snapshot);
  duplicate.units[1].id = duplicate.units[0].id;
  assert.throws(() => validateSnapshot(duplicate, sources), /duplicate/);
  const badRange = structuredClone(snapshot);
  badRange.units[1].source.start_byte = 4;
  assert.throws(() => validateSnapshot(badRange, sources));
  const unsupported = await extractSnapshot("demo", [
    {
      root_id: "root",
      relative_path: "opaque.rb",
      language: "ruby",
      bytes: Buffer.from("puts 'hi'"),
    },
  ]);
  assert.equal(unsupported.snapshot.files[0].extraction.status, "opaque");
  assert.equal(unsupported.snapshot.units.length, 1);
  const invalid = await extractSnapshot("demo", [
    {
      root_id: "root",
      relative_path: "bad.ts",
      language: "typescript",
      bytes: Uint8Array.of(0xff),
    },
  ]);
  assert.equal(invalid.snapshot.files[0].extraction.status, "failed");
  assert.equal(invalid.snapshot.units.length, 0);
  const component = await extractSnapshot("demo", [
    {
      root_id: "root",
      relative_path: "App.vue",
      language: "vue",
      bytes: Buffer.from("<script>export function inline() {}</script>"),
    },
  ]);
  assert.equal(component.snapshot.files[0].extraction.status, "opaque");
  assert.equal(component.snapshot.units[0].kind, "file");
  const empty = await extractSnapshot("demo", [
    {
      root_id: "root",
      relative_path: "empty.ts",
      language: "typescript",
      bytes: Buffer.alloc(0),
    },
  ]);
  assert.equal(empty.snapshot.files[0].extraction.status, "opaque");
  assert.deepEqual(empty.snapshot.units, []);
});

test("parse-error regions remain opaque and suppress facts from uncertain ancestors", async () => {
  const text =
    "function good() { call(); }\nfunction broken( {\n  uncertain();\n}\n";
  const { snapshot } = await extractSnapshot("demo", [
    {
      root_id: "root",
      relative_path: "broken.ts",
      language: "typescript",
      bytes: Buffer.from(text),
    },
  ]);
  assert.equal(snapshot.files[0].extraction.status, "partial");
  assert.ok(
    snapshot.units.some(
      (unit) => unit.kind === "function" && unit.name === "good",
    ),
  );
  assert.ok(snapshot.units.some((unit) => unit.kind === "opaque"));
  assert.deepEqual(
    snapshot.facts
      .filter((fact) => fact.kind === "calls")
      .map((fact) => fact.target_spelling),
    ["call"],
  );
});

test("four frontends retain methods, receiver/decorator scopes, overloads and nested calls", async () => {
  const cases = [
    [
      "go",
      "package demo\ntype Service struct{}\nfunc (s *Service) Run() { save() }\n",
      (units) => {
        assert.ok(
          units.some((unit) => unit.kind === "type" && unit.name === "Service"),
        );
        assert.ok(
          units.some(
            (unit) =>
              unit.kind === "method" &&
              unit.qualified_name === "Service::Run" &&
              unit.extensions?.data.receiver === "Service",
          ),
        );
      },
    ],
    [
      "python",
      "@register\nclass Service:\n    @cached\n    def fetch(self):\n        def inner():\n            save()\n        return inner()\n",
      (units) => {
        assert.ok(
          units.some(
            (unit) =>
              unit.kind === "type" &&
              unit.name === "Service" &&
              unit.origin.syntax_kind === "decorated_definition" &&
              unit.extensions?.data.decorated_definition === true,
          ),
        );
        assert.ok(
          units.some(
            (unit) =>
              unit.kind === "method" &&
              unit.name === "fetch" &&
              unit.origin.syntax_kind === "decorated_definition",
          ),
        );
        assert.ok(
          units.some(
            (unit) => unit.kind === "function" && unit.name === "inner",
          ),
        );
      },
    ],
    [
      "rust",
      "mod api { pub struct Service; impl Service { fn run(&self) { save(); } } }\n",
      (units) => {
        assert.ok(
          units.some((unit) => unit.kind === "module" && unit.name === "api"),
        );
        assert.ok(
          units.some((unit) => unit.kind === "type" && unit.name === "Service"),
        );
        assert.ok(
          units.some(
            (unit) =>
              unit.kind === "type" &&
              unit.subtype === "impl_item" &&
              unit.extensions?.data.impl === true,
          ),
        );
        assert.ok(
          units.some(
            (unit) =>
              unit.kind === "method" &&
              unit.qualified_name === "api::Service::run",
          ),
        );
      },
    ],
    [
      "typescript",
      "class Service { private value = 0; run() { const nested = () => save(); const localValue = 1; return nested(); } }\nconst limit = 4;\nfunction add(): number;\nfunction add(value: number): number;\nfunction add(value = 0) { return value; }\n",
      (units) => {
        assert.ok(
          units.some((unit) => unit.kind === "type" && unit.name === "Service"),
        );
        assert.ok(
          units.some((unit) => unit.kind === "method" && unit.name === "run"),
        );
        assert.ok(
          units.some((unit) => unit.kind === "value" && unit.name === "value"),
        );
        assert.ok(
          units.some((unit) => unit.kind === "value" && unit.name === "limit"),
        );
        assert.ok(!units.some((unit) => unit.name === "localValue"));
        assert.equal(units.filter((unit) => unit.name === "add").length, 3);
        assert.equal(
          units.filter(
            (unit) =>
              unit.name === "add" &&
              unit.extensions?.data.overload_signature === true,
          ).length,
          2,
        );
      },
    ],
  ];
  for (const [language, text, assertUnits] of cases) {
    const { snapshot } = await extractSnapshot("demo", [
      {
        root_id: "root",
        relative_path: `fixture.${language}`,
        language,
        bytes: Buffer.from(text),
      },
    ]);
    assert.equal(snapshot.files[0].extraction.status, "complete", language);
    assertUnits(snapshot.units, snapshot.facts);
    validateSnapshot(
      snapshot,
      new Map([[snapshot.files[0].file_id, Buffer.from(text)]]),
    );
  }
});

test("Go declarations retain type/doc bytes and source-backed struct fields with distinct token spans", async () => {
  const text = [
    "package demo",
    "// A type's attached documentation.",
    "type Widget struct {",
    '  ID, Name string `json:"name"`',
    "  *Other",
    "  pkg.External",
    "  nested struct { child int }",
    "}",
    "type Alias = Widget",
    "var First, Second = 1, 2",
    "",
  ].join("\r\n");
  const bytes = Buffer.from(text);
  const { snapshot, sources } = await extractSnapshot("demo", [
    {
      root_id: "root",
      relative_path: "widget.go",
      language: "go",
      bytes,
    },
  ]);
  validateSnapshot(snapshot, sources);
  const named = (name) => snapshot.units.find((unit) => unit.name === name);
  assert.equal(
    bytes
      .subarray(
        named("Widget").source.start_byte,
        named("Widget").source.end_byte,
      )
      .toString(),
    text.slice(text.indexOf("// A type's"), text.indexOf("\r\ntype Alias")),
  );
  assert.equal(named("Widget").source.start.line, 2);
  assert.equal(named("Alias").source.start.column_byte, 0);
  assert.equal(
    bytes
      .subarray(
        named("Alias").source.start_byte,
        named("Alias").source.end_byte,
      )
      .toString(),
    "type Alias = Widget",
  );
  const widgetSyntax = named("Widget").extensions.data.syntax_source;
  assert.equal(
    bytes.subarray(widgetSyntax.start_byte, widgetSyntax.end_byte).toString(),
    text.slice(text.indexOf("Widget struct"), text.indexOf("\r\ntype Alias")),
  );
  const fields = ["ID", "Name", "Other", "External", "nested", "child"];
  for (const field of fields) {
    const unit = named(field);
    assert.equal(unit.kind, "value", field);
    assert.equal(unit.extensions.language, "go");
    const token = unit.extensions.data.identifier_source;
    assert.equal(
      bytes.subarray(token.start_byte, token.end_byte).toString(),
      field,
    );
    assert.equal(
      snapshot.units.find((candidate) => candidate.id === unit.parent_id)?.kind,
      field === "child" ? "value" : "type",
    );
  }
  assert.equal(named("ID").source.start_byte, named("Name").source.start_byte);
  assert.equal(named("ID").source.end_byte, named("Name").source.end_byte);
  assert.notEqual(named("ID").id, named("Name").id);
  assert.equal(named("child").qualified_name, "Widget::nested::child");
  assert.equal(
    named("First").source.start_byte,
    named("Second").source.start_byte,
  );
  assert.equal(
    named("First").parent_id,
    snapshot.units.find((unit) => unit.kind === "file").id,
  );
  const tampered = structuredClone(snapshot);
  const target = tampered.units.find((unit) => unit.name === "ID");
  target.extensions.data.identifier_source.start_byte += 1;
  assert.throws(
    () => validateSnapshot(tampered, sources),
    /source map|identifier/,
  );
  const fakeSyntax = structuredClone(snapshot);
  fakeSyntax.units.find(
    (unit) => unit.name === "Widget",
  ).extensions.data.syntax_source.start_byte += 1;
  assert.throws(
    () => validateSnapshot(fakeSyntax, sources),
    /source map|syntax/,
  );
});

test("class attributes and attached export/attribute spans remain source-backed without guessing local variables", async () => {
  const samples = [
    [
      "python",
      "@registered\nclass Service:\n    count: int = 1\n    ready = True\n    def run(self):\n        local = 2\n        self.dynamic = 3\n",
      ["count", "ready"],
      ["local", "dynamic"],
      "@registered\nclass Service:",
    ],
    [
      "typescript",
      "/** Docs */\nexport class Service { run() {} }\n",
      [],
      [],
      "export class Service",
    ],
    [
      "rust",
      "/// Docs\n#[derive(Clone)]\npub struct Service { value: i32 }\n",
      [],
      [],
      "/// Docs\n#[derive(Clone)]\npub struct Service",
    ],
  ];
  for (const [language, text, present, absent, header] of samples) {
    const { snapshot, sources } = await extractSnapshot("demo", [
      {
        root_id: "root",
        relative_path: `fixture.${language}`,
        language,
        bytes: Buffer.from(text),
      },
    ]);
    validateSnapshot(snapshot, sources);
    const service = snapshot.units.find((unit) => unit.name === "Service");
    assert.ok(service);
    const full = Buffer.from(text)
      .subarray(service.source.start_byte, service.source.end_byte)
      .toString();
    assert.ok(full.startsWith(header), `${language}: ${full}`);
    for (const name of present) {
      const unit = snapshot.units.find((candidate) => candidate.name === name);
      assert.equal(unit.kind, "value");
      assert.equal(unit.parent_id, service.id);
      assert.equal(
        Buffer.from(text)
          .subarray(
            unit.extensions.data.identifier_source.start_byte,
            unit.extensions.data.identifier_source.end_byte,
          )
          .toString(),
        name,
      );
    }
    for (const name of absent)
      assert.ok(!snapshot.units.some((unit) => unit.name === name));
  }
});

test("one parse emits exact signature and adjacent documentation refs in all four languages", async () => {
  const cases = [
    {
      language: "go",
      text: "package demo\r\n// é 🚀 guide\r\n// second line\r\nfunc run(x int) int {\r\n  return x\r\n}\r\n// detached\r\n\r\nfunc skip() {}\r\n",
      signature: "func run(x int) int",
      documentation: "// é 🚀 guide\r\n// second line",
    },
    {
      language: "python",
      text: "# é 🚀 guide\r\n@cached\r\ndef run(x: int) -> int:\r\n    return x\r\n# detached\r\n\r\ndef skip():\r\n    pass\r\n",
      signature: "def run(x: int) -> int:",
      documentation: "# é 🚀 guide",
    },
    {
      language: "rust",
      text: "/// é 🚀 guide\r\n#[inline]\r\npub fn run(x: i32) -> i32 {\r\n    x\r\n}\r\n/// detached\r\n\r\nfn skip() {}\r\n",
      signature: "pub fn run(x: i32) -> i32",
      documentation: "/// é 🚀 guide",
    },
    {
      language: "typescript",
      text: "/** é 🚀 guide */\r\nexport function run(x: number): number {\r\n  return x;\r\n}\r\n/** detached */\r\n\r\nfunction skip() {}\r\n",
      signature: "export function run(x: number): number",
      documentation: "/** é 🚀 guide */",
    },
  ];
  for (const { language, text, signature, documentation } of cases) {
    const bytes = Buffer.from(text);
    const { snapshot, sources } = await extractSnapshot("demo", [
      {
        root_id: "root",
        relative_path: `fixture.${language}`,
        language,
        bytes,
      },
    ]);
    assert.equal(snapshot.files[0].extraction.status, "complete", language);
    validateSnapshot(snapshot, sources);
    const run = snapshot.units.find((unit) => unit.name === "run");
    const skip = snapshot.units.find((unit) => unit.name === "skip");
    assert.ok(run, `${language}: run unit`);
    assert.ok(skip, `${language}: skip unit`);
    for (const [anchored, expected] of [
      [run.signature, signature],
      [run.documentation, documentation],
    ]) {
      assert.equal(anchored?.text, expected, `${language}: anchored text`);
      assert.equal(
        bytes
          .subarray(anchored.source.start_byte, anchored.source.end_byte)
          .toString(),
        expected,
        `${language}: byte-for-byte source slice`,
      );
      assert.equal(anchored.source.sha256, snapshot.files[0].sha256);
    }
    assert.equal(
      skip.documentation,
      undefined,
      `${language}: blank line detaches comment`,
    );
    const fake = structuredClone(snapshot);
    fake.units.find((unit) => unit.name === "run").signature.text +=
      " invented";
    assert.throws(
      () => validateSnapshot(fake, sources),
      /synthetic anchored text/,
    );
  }
});

test("type declarations retain source-backed headers and comments across wrappers", async () => {
  const cases = [
    [
      "go",
      "package demo\n// type docs\ntype Gadget struct { Value int }\n",
      "type Gadget struct",
      "// type docs",
    ],
    [
      "python",
      "# class docs\n@register\nclass Gadget:\n    pass\n",
      "class Gadget:",
      "# class docs",
    ],
    [
      "rust",
      "/// struct docs\n#[derive(Clone)]\npub struct Gadget { field: i32 }\n",
      "pub struct Gadget",
      "/// struct docs",
    ],
    [
      "typescript",
      "/** class docs */\nexport class Gadget { value = 1; }\n",
      "export class Gadget",
      "/** class docs */",
    ],
  ];
  for (const [language, text, signature, documentation] of cases) {
    const bytes = Buffer.from(text);
    const { snapshot, sources } = await extractSnapshot("demo", [
      {
        root_id: "root",
        relative_path: `fixture.${language}`,
        language,
        bytes,
      },
    ]);
    validateSnapshot(snapshot, sources);
    const gadget = snapshot.units.find((unit) => unit.name === "Gadget");
    assert.ok(gadget, language);
    assert.equal(gadget.signature?.text, signature, `${language}: type header`);
    assert.equal(
      gadget.documentation?.text,
      documentation,
      `${language}: attached docs`,
    );
  }
});

test("multiline exported signatures preserve exact whitespace instead of truncating or synthesizing text", async () => {
  const signature = "export function run(\r\n  x: number,\r\n): number";
  const text = `${signature} {\r\n  return x;\r\n}\r\n`;
  const bytes = Buffer.from(text);
  const { snapshot, sources } = await extractSnapshot("demo", [
    {
      root_id: "root",
      relative_path: "multi.ts",
      language: "typescript",
      bytes,
    },
  ]);
  validateSnapshot(snapshot, sources);
  const run = snapshot.units.find((unit) => unit.name === "run");
  assert.equal(run.signature?.text, signature);
  assert.equal(run.signature.source.start_byte, 0);
  assert.equal(run.signature.source.end_byte, Buffer.byteLength(signature));
});

test("values remain semantic units without mislabeling initializers as signatures", async () => {
  for (const [language, text, name, kind] of [
    ["go", "package demo\nvar Result = compute()\n", "Result", "value"],
    [
      "typescript",
      "class Gadget { field = () => compute(); }\n",
      "field",
      "function",
    ],
  ]) {
    const { snapshot } = await extractSnapshot("demo", [
      {
        root_id: "root",
        relative_path: `fixture.${language}`,
        language,
        bytes: Buffer.from(text),
      },
    ]);
    const value = snapshot.units.find((unit) => unit.name === name);
    assert.equal(value?.kind, kind, language);
    assert.equal(
      value.signature,
      undefined,
      `${language}: initializer is not a signature`,
    );
  }
});

test("unbounded multiline declaration headers abstain instead of publishing a partial signature", async () => {
  const text = "function run(\n  x: number,\n): number;\n";
  const { snapshot, sources } = await extractSnapshot("demo", [
    {
      root_id: "root",
      relative_path: "overload.ts",
      language: "typescript",
      bytes: Buffer.from(text),
    },
  ]);
  validateSnapshot(snapshot, sources);
  const run = snapshot.units.find((unit) => unit.name === "run");
  assert.ok(run);
  assert.equal(run.signature, undefined);
});
