#![feature(rustc_private)]
#![allow(internal_features)]

extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use rustc_driver::{Callbacks, Compilation};
use rustc_hir::def::{DefKind, Res};
use rustc_hir::intravisit::{self, FnKind, Visitor};
use rustc_hir::{self as hir, ExprKind};
use rustc_interface::interface;
use rustc_middle::hir::nested_filter::OnlyBodies;
use rustc_middle::ty::{AssocContainer, TyCtxt};
use rustc_span::def_id::LocalDefId;
use rustc_span::source_map::SourceMap;
use rustc_span::{Pos, Span};

const SCHEMA: &str = "zvec-grep.rust-callfacts";
const VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SourceRange {
    path: String,
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
    start_column: usize,
    end_column: usize,
}

impl SourceRange {
    fn symbol(&self) -> SymbolRef {
        SymbolRef {
            path: self.path.clone(),
            start_byte: self.start_byte,
            end_byte: self.end_byte,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SymbolRef {
    path: String,
    start_byte: usize,
    end_byte: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Fact {
    range: SourceRange,
    caller: SymbolRef,
    target_name: String,
    target: Option<SymbolRef>,
    possible_targets: Vec<SymbolRef>,
    resolution: String,
}

#[derive(Clone)]
struct DriverConfig {
    root: PathBuf,
    run_dir: PathBuf,
    edition: String,
}

struct SourceLocator {
    root: PathBuf,
    files: BTreeMap<String, Vec<u8>>,
}

impl SourceLocator {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            files: BTreeMap::new(),
        }
    }

    fn range(&mut self, source_map: &SourceMap, span: Span) -> Option<SourceRange> {
        if span.is_dummy() || span.from_expansion() {
            return None;
        }
        let begin = source_map.lookup_byte_offset(span.lo());
        let end = source_map.lookup_byte_offset(span.hi());
        if begin.sf.start_pos != end.sf.start_pos {
            return None;
        }
        let path = source_map
            .span_to_filename(span)
            .into_local_path()?
            .canonicalize()
            .ok()?;
        let relative = path.strip_prefix(&self.root).ok()?;
        let path = relative_path(relative);
        let start_byte = begin.pos.to_usize();
        let end_byte = end.pos.to_usize();
        let source = self
            .files
            .entry(path.clone())
            .or_insert_with(|| fs::read(&self.root.join(&path)).unwrap_or_default());
        if start_byte >= end_byte || end_byte > source.len() {
            return None;
        }
        let (start_line, start_column) = line_column(source, start_byte);
        let (end_line, end_column) = line_column(source, end_byte);
        Some(SourceRange {
            path,
            start_byte,
            end_byte,
            start_line,
            end_line,
            start_column,
            end_column,
        })
    }
}

struct Enclosing {
    symbol: SymbolRef,
}

struct CallVisitor<'tcx> {
    tcx: TyCtxt<'tcx>,
    locator: SourceLocator,
    enclosing: Vec<Enclosing>,
    calls: Vec<Fact>,
}

impl<'tcx> CallVisitor<'tcx> {
    fn source_symbol(&mut self, def_id: rustc_span::def_id::DefId) -> Option<SymbolRef> {
        if !def_id.is_local() {
            return None;
        }
        let span = self
            .tcx
            .hir_span_with_body(self.tcx.local_def_id_to_hir_id(def_id.expect_local()));
        self.locator
            .range(self.tcx.sess.source_map(), span)
            .map(|range| range.symbol())
    }

    fn target_name(&self, def_id: rustc_span::def_id::DefId) -> String {
        self.tcx.item_name(def_id).as_str().to_owned()
    }

    fn target_for_def(
        &mut self,
        def_kind: DefKind,
        def_id: rustc_span::def_id::DefId,
    ) -> (String, Option<SymbolRef>, String, Vec<SymbolRef>) {
        let target_name = self.target_name(def_id);
        if !matches!(def_kind, DefKind::Fn | DefKind::AssocFn) {
            return (target_name, None, "unresolved".to_owned(), Vec::new());
        }
        let Some(symbol) = self.source_symbol(def_id) else {
            return (target_name, None, "external".to_owned(), Vec::new());
        };
        if let Some(item) = self.tcx.opt_associated_item(def_id)
            && matches!(
                item.container,
                AssocContainer::Trait | AssocContainer::TraitImpl(_)
            )
        {
            let possible = match item.container {
                AssocContainer::Trait => Some(symbol),
                AssocContainer::TraitImpl(Ok(trait_item)) => self.source_symbol(trait_item),
                AssocContainer::TraitImpl(Err(_)) | AssocContainer::InherentImpl => None,
            };
            return (
                target_name,
                None,
                if possible.is_some() {
                    "trait-dispatch".to_owned()
                } else {
                    "unresolved".to_owned()
                },
                possible.into_iter().collect(),
            );
        }
        (target_name, Some(symbol), "static".to_owned(), Vec::new())
    }

    fn report_call(&mut self, expr: &'tcx hir::Expr<'tcx>, callee: &'tcx hir::Expr<'tcx>) {
        let Some(caller) = self
            .enclosing
            .last()
            .map(|enclosing| enclosing.symbol.clone())
        else {
            return;
        };
        let Some(range) = self.locator.range(self.tcx.sess.source_map(), expr.span) else {
            return;
        };
        let typeck = self.tcx.typeck(expr.hir_id.owner.def_id);
        let (target_name, target, resolution, possible_targets) = match &callee.kind {
            ExprKind::Path(qpath) => match typeck.qpath_res(qpath, callee.hir_id) {
                Res::Def(def_kind, def_id) => self.target_for_def(def_kind, def_id),
                Res::Local(_) => (
                    "function-value".to_owned(),
                    None,
                    "function-value".to_owned(),
                    Vec::new(),
                ),
                Res::Err => (
                    "<unresolved>".to_owned(),
                    None,
                    "unresolved".to_owned(),
                    Vec::new(),
                ),
                _ => (
                    "<unresolved>".to_owned(),
                    None,
                    "unresolved".to_owned(),
                    Vec::new(),
                ),
            },
            ExprKind::Closure { .. } => (
                "<closure>".to_owned(),
                None,
                "function-value".to_owned(),
                Vec::new(),
            ),
            _ => (
                "<unresolved>".to_owned(),
                None,
                "unresolved".to_owned(),
                Vec::new(),
            ),
        };
        self.calls.push(Fact {
            range,
            caller,
            target_name,
            target,
            possible_targets,
            resolution,
        });
    }

    fn report_method(
        &mut self,
        expr: &'tcx hir::Expr<'tcx>,
        segment: &'tcx hir::PathSegment<'tcx>,
    ) {
        let Some(caller) = self
            .enclosing
            .last()
            .map(|enclosing| enclosing.symbol.clone())
        else {
            return;
        };
        let Some(range) = self.locator.range(self.tcx.sess.source_map(), expr.span) else {
            return;
        };
        let typeck = self.tcx.typeck(expr.hir_id.owner.def_id);
        let (target, resolution, possible_targets) = match typeck.type_dependent_def_id(expr.hir_id)
        {
            Some(def_id) if matches!(self.tcx.def_kind(def_id), DefKind::Fn | DefKind::AssocFn) => {
                let target = self.source_symbol(def_id);
                match self
                    .tcx
                    .opt_associated_item(def_id)
                    .map(|item| item.container)
                {
                    Some(AssocContainer::Trait) => (
                        None,
                        "trait-dispatch".to_owned(),
                        target.into_iter().collect(),
                    ),
                    Some(AssocContainer::TraitImpl(Ok(trait_item))) => (
                        None,
                        "trait-dispatch".to_owned(),
                        self.source_symbol(trait_item).into_iter().collect(),
                    ),
                    Some(AssocContainer::TraitImpl(Err(_))) => {
                        (None, "unresolved".to_owned(), Vec::new())
                    }
                    Some(AssocContainer::InherentImpl) => match target {
                        Some(target) => (Some(target), "static".to_owned(), Vec::new()),
                        None => (None, "external".to_owned(), Vec::new()),
                    },
                    None => match target {
                        Some(target) => (Some(target), "static".to_owned(), Vec::new()),
                        None => (None, "external".to_owned(), Vec::new()),
                    },
                }
            }
            Some(_) => (None, "unresolved".to_owned(), Vec::new()),
            None => (None, "unresolved".to_owned(), Vec::new()),
        };
        self.calls.push(Fact {
            range,
            caller,
            target_name: segment.ident.name.as_str().to_owned(),
            target,
            possible_targets,
            resolution,
        });
    }
}

impl<'tcx> Visitor<'tcx> for CallVisitor<'tcx> {
    type NestedFilter = OnlyBodies;

    fn maybe_tcx(&mut self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_fn(
        &mut self,
        kind: FnKind<'tcx>,
        decl: &'tcx hir::FnDecl<'tcx>,
        body: hir::BodyId,
        _span: Span,
        id: LocalDefId,
    ) -> Self::Result {
        if matches!(kind, FnKind::Closure) {
            // Closures do not have a stable source definition node in the
            // graph. Leave their bodies to the syntax fallback rather than
            // attributing nested calls to the enclosing function.
            return;
        }
        let depth = self.enclosing.len();
        if let Some(symbol) = self.source_symbol(id.to_def_id()) {
            self.enclosing.push(Enclosing { symbol });
        }
        let result = intravisit::walk_fn(self, kind, decl, body, id);
        self.enclosing.truncate(depth);
        result
    }

    fn visit_expr(&mut self, expr: &'tcx hir::Expr<'tcx>) -> Self::Result {
        match &expr.kind {
            ExprKind::Call(callee, _) => self.report_call(expr, callee),
            ExprKind::MethodCall(segment, ..) => self.report_method(expr, segment),
            _ => {}
        }
        intravisit::walk_expr(self, expr)
    }
}

struct DriverCallbacks {
    config: DriverConfig,
}

impl Callbacks for DriverCallbacks {
    fn after_analysis<'tcx>(
        &mut self,
        _compiler: &interface::Compiler,
        tcx: TyCtxt<'tcx>,
    ) -> Compilation {
        let mut visitor = CallVisitor {
            tcx,
            locator: SourceLocator::new(self.config.root.clone()),
            enclosing: Vec::new(),
            calls: Vec::new(),
        };
        tcx.hir_visit_all_item_likes_in_crate(&mut visitor);
        if let Err(error) =
            write_fragment(&self.config.run_dir, &self.config.edition, &visitor.calls)
        {
            eprintln!("rust-callfacts: write fragment: {error}");
        }
        Compilation::Continue
    }
}

fn main() {
    let arguments = env::args().collect::<Vec<_>>();
    if arguments.get(1).map(String::as_str) == Some("--merge") {
        if let Err(error) = merge(&arguments[2..]) {
            eprintln!("rust-callfacts: merge: {error}");
            process::exit(1);
        }
        return;
    }
    let root = match env::var_os("ZG_RUST_CALLFACTS_ROOT") {
        Some(root) => PathBuf::from(root),
        None => {
            eprintln!(
                "rust-callfacts: ZG_RUST_CALLFACTS_ROOT is required in compiler-wrapper mode"
            );
            process::exit(2);
        }
    };
    let run_dir = match env::var_os("ZG_RUST_CALLFACTS_RUN_DIR") {
        Some(run_dir) => PathBuf::from(run_dir),
        None => {
            eprintln!(
                "rust-callfacts: ZG_RUST_CALLFACTS_RUN_DIR is required in compiler-wrapper mode"
            );
            process::exit(2);
        }
    };
    let mut rustc_arguments = Vec::with_capacity(arguments.len() + 1);
    rustc_arguments.push("rustc".to_owned());
    if !arguments
        .iter()
        .skip(1)
        .any(|argument| argument == "--sysroot")
        && let Some(sysroot) = env::var_os("ZG_RUST_CALLFACTS_SYSROOT")
    {
        rustc_arguments.push("--sysroot".to_owned());
        rustc_arguments.push(sysroot.to_string_lossy().into_owned());
    }
    rustc_arguments.extend(arguments.into_iter().skip(1));
    let edition = rustc_edition(&rustc_arguments);
    rustc_driver::run_compiler(
        &rustc_arguments,
        &mut DriverCallbacks {
            config: DriverConfig {
                root,
                run_dir,
                edition,
            },
        },
    );
}

fn rustc_edition(arguments: &[String]) -> String {
    for (index, argument) in arguments.iter().enumerate() {
        if let Some(edition) = argument.strip_prefix("--edition=") {
            return edition.to_owned();
        }
        if argument == "--edition"
            && let Some(edition) = arguments.get(index + 1)
        {
            return edition.clone();
        }
    }
    env::var("ZG_RUST_CALLFACTS_EDITION").unwrap_or_else(|_| "unknown".to_owned())
}

fn write_fragment(run_dir: &Path, edition: &str, calls: &[Fact]) -> io::Result<()> {
    fs::create_dir_all(run_dir)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = run_dir.join(format!("facts-{}-{timestamp}.tsv", process::id()));
    let mut output = OpenOptions::new().create_new(true).write(true).open(path)?;
    writeln!(output, "#rust-callfacts-edition\t{}", encode_text(edition))?;
    for call in calls {
        writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            encode_text(&call.range.path),
            call.range.start_byte,
            call.range.end_byte,
            call.range.start_line,
            call.range.end_line,
            call.range.start_column,
            call.range.end_column,
            encode_text(&call.caller.path),
            call.caller.start_byte,
            call.caller.end_byte,
            encode_text(&call.target_name),
            encode_text(&call.resolution),
            call.target.as_ref().map_or_else(String::new, encode_symbol),
            call.possible_targets
                .iter()
                .map(encode_symbol)
                .collect::<Vec<_>>()
                .join(";"),
        )?;
    }
    Ok(())
}

fn merge(arguments: &[String]) -> io::Result<()> {
    let root = required_argument(arguments, "--root")?;
    let run_dir = required_argument(arguments, "--run-dir")?;
    let output = required_argument(arguments, "--output")?;
    let root = canonicalize(Path::new(&root))?;
    let run_dir = PathBuf::from(run_dir);
    let output = PathBuf::from(output);
    let mut calls = Vec::new();
    let mut editions = BTreeSet::new();
    if run_dir.is_dir() {
        let mut fragments = fs::read_dir(&run_dir)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("tsv"))
            .collect::<Vec<_>>();
        fragments.sort();
        for fragment in fragments {
            read_fragment(&fragment, &mut calls, &mut editions)?;
        }
    }
    let calls = deduplicate_calls(calls);
    let source_files = collect_rust_sources(&root)?;
    let context_files = collect_context_files(&root)?;
    let context = context_from_environment(&root, context_files, &editions);
    let context_sha256 = context_fingerprint(&context);
    let encoded = artifact_json(&source_files, &context, &context_sha256, &calls);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = output.with_extension("json.tmp");
    fs::write(&temporary, encoded)?;
    fs::rename(temporary, output)?;
    Ok(())
}

fn read_fragment(
    path: &Path,
    calls: &mut Vec<Fact>,
    editions: &mut BTreeSet<String>,
) -> io::Result<()> {
    let file = File::open(path)?;
    for (line_number, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("#rust-callfacts-edition\t") {
            editions.insert(decode_text(value)?);
            continue;
        }
        let fields = line
            .split('\t')
            .map(decode_text)
            .collect::<io::Result<Vec<_>>>()?;
        if fields.len() < 14 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{}:{} has {} fields, expected at least 14",
                    path.display(),
                    line_number + 1,
                    fields.len()
                ),
            ));
        }
        let number = |index: usize| {
            fields[index].parse::<usize>().map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{}:{} field {index}: {error}",
                        path.display(),
                        line_number + 1
                    ),
                )
            })
        };
        let target = parse_symbol(&fields[12])?;
        let possible_targets = if fields[13].is_empty() {
            Vec::new()
        } else {
            fields[13]
                .split(';')
                .map(|value| {
                    parse_symbol(value)?.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "empty possible target")
                    })
                })
                .collect::<io::Result<Vec<_>>>()?
        };
        calls.push(Fact {
            range: SourceRange {
                path: fields[0].clone(),
                start_byte: number(1)?,
                end_byte: number(2)?,
                start_line: number(3)?,
                end_line: number(4)?,
                start_column: number(5)?,
                end_column: number(6)?,
            },
            caller: SymbolRef {
                path: fields[7].clone(),
                start_byte: number(8)?,
                end_byte: number(9)?,
            },
            target_name: fields[10].clone(),
            resolution: fields[11].clone(),
            target,
            possible_targets,
        });
    }
    Ok(())
}

fn parse_symbol(value: &str) -> io::Result<Option<SymbolRef>> {
    if value.is_empty() {
        return Ok(None);
    }
    let mut fields = value.splitn(3, ',');
    let path = fields.next().unwrap_or_default().to_owned();
    let start_byte = fields
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "symbol is missing start"))?
        .parse()
        .map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, format!("symbol start: {error}"))
        })?;
    let end_byte = fields
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "symbol is missing end"))?
        .parse()
        .map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, format!("symbol end: {error}"))
        })?;
    Ok(Some(SymbolRef {
        path,
        start_byte,
        end_byte,
    }))
}

fn deduplicate_calls(calls: Vec<Fact>) -> Vec<Fact> {
    let mut calls = calls;
    for call in &mut calls {
        call.possible_targets.sort();
        call.possible_targets.dedup();
    }
    calls.sort();
    let mut output = Vec::new();
    for call in calls {
        if let Some(previous) = output.iter_mut().find(|previous: &&mut Fact| {
            previous.range.path == call.range.path
                && previous.range.start_byte == call.range.start_byte
                && previous.range.end_byte == call.range.end_byte
        }) {
            if *previous != call {
                previous.target = None;
                previous.possible_targets.clear();
                previous.resolution = "unresolved".to_owned();
            }
        } else {
            output.push(call);
        }
    }
    output
}

#[derive(Clone)]
struct Context {
    rustc_version: String,
    rustc_commit: String,
    host: String,
    target: String,
    edition: String,
    manifest_path: String,
    lockfile_path: Option<String>,
    toolchain_path: Option<String>,
    settings: BTreeMap<String, String>,
    context_files: Vec<(String, String)>,
}

fn context_from_environment(
    root: &Path,
    context_files: Vec<(String, String)>,
    observed_editions: &BTreeSet<String>,
) -> Context {
    let manifest_path = env::var("ZG_RUST_CALLFACTS_MANIFEST")
        .ok()
        .and_then(|path| relative_existing_path(root, Path::new(&path)))
        .or_else(|| find_context_file(&context_files, "Cargo.toml"))
        .unwrap_or_else(|| "Cargo.toml".to_owned());
    let lockfile_path = env::var("ZG_RUST_CALLFACTS_LOCKFILE")
        .ok()
        .filter(|path| !path.is_empty())
        .and_then(|path| relative_existing_path(root, Path::new(&path)))
        .or_else(|| {
            let candidate = Path::new(&manifest_path)
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("Cargo.lock");
            context_files
                .iter()
                .any(|(path, _)| path == &candidate.to_string_lossy())
                .then(|| candidate.to_string_lossy().into_owned())
        });
    let toolchain_path = env::var("ZG_RUST_CALLFACTS_TOOLCHAIN")
        .ok()
        .filter(|path| !path.is_empty())
        .and_then(|path| relative_existing_path(root, Path::new(&path)))
        .or_else(|| {
            let parent = Path::new(&manifest_path)
                .parent()
                .unwrap_or_else(|| Path::new("."));
            ["rust-toolchain.toml", "rust-toolchain"]
                .iter()
                .map(|name| parent.join(name))
                .find(|candidate| {
                    context_files
                        .iter()
                        .any(|(path, _)| path == &candidate.to_string_lossy())
                })
                .map(|path| path.to_string_lossy().into_owned())
        });
    let setting_names = [
        "CARGO_BUILD_TARGET",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_FEATURES",
        "CARGO_PROFILE",
        "CARGO_TERM_COLOR",
        "RUSTFLAGS",
        "RUSTDOCFLAGS",
    ];
    let settings = setting_names
        .into_iter()
        .map(|name| (name.to_owned(), env::var(name).unwrap_or_default()))
        .collect();
    let edition = if observed_editions.is_empty() {
        env::var("ZG_RUST_CALLFACTS_EDITION").unwrap_or_else(|_| "unknown".to_owned())
    } else if observed_editions.len() == 1 {
        observed_editions
            .first()
            .expect("non-empty edition set")
            .clone()
    } else {
        format!(
            "mixed:{}",
            observed_editions
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    Context {
        rustc_version: env::var("ZG_RUST_CALLFACTS_RUSTC_VERSION").unwrap_or_default(),
        rustc_commit: env::var("ZG_RUST_CALLFACTS_RUSTC_COMMIT").unwrap_or_default(),
        host: env::var("ZG_RUST_CALLFACTS_HOST").unwrap_or_default(),
        target: env::var("ZG_RUST_CALLFACTS_TARGET").unwrap_or_default(),
        edition,
        manifest_path,
        lockfile_path,
        toolchain_path,
        settings,
        context_files,
    }
}

fn artifact_json(
    source_files: &[(String, String)],
    context: &Context,
    context_sha256: &str,
    calls: &[Fact],
) -> String {
    let mut output = String::from("{\"schema\":");
    output.push_str(&json_string(SCHEMA));
    output.push_str(&format!(",\"version\":{VERSION},\"context\":{{"));
    output.push_str("\"rustc_version\":");
    output.push_str(&json_string(&context.rustc_version));
    output.push_str(",\"rustc_commit\":");
    output.push_str(&json_string(&context.rustc_commit));
    output.push_str(",\"host\":");
    output.push_str(&json_string(&context.host));
    output.push_str(",\"target\":");
    output.push_str(&json_string(&context.target));
    output.push_str(",\"edition\":");
    output.push_str(&json_string(&context.edition));
    output.push_str(",\"manifest_path\":");
    output.push_str(&json_string(&context.manifest_path));
    output.push_str(",\"lockfile_path\":");
    push_optional_string(&mut output, context.lockfile_path.as_deref());
    output.push_str(",\"toolchain_path\":");
    push_optional_string(&mut output, context.toolchain_path.as_deref());
    output.push_str(",\"settings\":{");
    for (index, (name, value)) in context.settings.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(&json_string(name));
        output.push(':');
        output.push_str(&json_string(value));
    }
    output.push_str("},\"context_files\":");
    push_files(&mut output, &context.context_files);
    output.push_str("},\"context_sha256\":");
    output.push_str(&json_string(context_sha256));
    output.push_str(",\"files\":");
    push_files(&mut output, source_files);
    output.push_str(",\"calls\":[");
    for (index, call) in calls.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str("{\"path\":");
        output.push_str(&json_string(&call.range.path));
        output.push_str(&format!(
            ",\"start_byte\":{},\"end_byte\":{},\"start_line\":{},\"end_line\":{},\"start_column\":{},\"end_column\":{}",
            call.range.start_byte,
            call.range.end_byte,
            call.range.start_line,
            call.range.end_line,
            call.range.start_column,
            call.range.end_column,
        ));
        output.push_str(",\"caller\":");
        push_symbol(&mut output, &call.caller);
        output.push_str(",\"target_name\":");
        output.push_str(&json_string(&call.target_name));
        output.push_str(",\"target\":");
        if let Some(target) = &call.target {
            push_symbol(&mut output, target);
        } else {
            output.push_str("null");
        }
        output.push_str(",\"possible_targets\":[");
        for (target_index, target) in call.possible_targets.iter().enumerate() {
            if target_index > 0 {
                output.push(',');
            }
            push_symbol(&mut output, target);
        }
        output.push_str("],\"resolution\":");
        output.push_str(&json_string(&call.resolution));
        output.push('}');
    }
    output.push_str("]}");
    output
}

fn push_files(output: &mut String, files: &[(String, String)]) {
    output.push('[');
    for (index, (path, sha256)) in files.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str("{\"path\":");
        output.push_str(&json_string(path));
        output.push_str(",\"sha256\":");
        output.push_str(&json_string(sha256));
        output.push('}');
    }
    output.push(']');
}

fn push_symbol(output: &mut String, symbol: &SymbolRef) {
    output.push_str("{\"path\":");
    output.push_str(&json_string(&symbol.path));
    output.push_str(&format!(
        ",\"start_byte\":{},\"end_byte\":{}",
        symbol.start_byte, symbol.end_byte
    ));
    output.push('}');
}

fn push_optional_string(output: &mut String, value: Option<&str>) {
    if let Some(value) = value {
        output.push_str(&json_string(value));
    } else {
        output.push_str("null");
    }
}

fn collect_rust_sources(root: &Path) -> io::Result<Vec<(String, String)>> {
    let mut paths = Vec::new();
    collect_paths(root, &mut paths, |path| {
        path.extension().and_then(|extension| extension.to_str()) == Some("rs")
    })?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| digest_file(root, &path))
        .collect()
}

fn collect_context_files(root: &Path) -> io::Result<Vec<(String, String)>> {
    let mut paths = Vec::new();
    collect_paths(root, &mut paths, is_context_path)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| digest_file(root, &path))
        .collect()
}

fn collect_paths<F>(root: &Path, output: &mut Vec<PathBuf>, predicate: F) -> io::Result<()>
where
    F: Fn(&Path) -> bool + Copy,
{
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some(".git" | ".zvec-grep" | "node_modules" | "target")
            ) {
                continue;
            }
            collect_paths(&path, output, predicate)?;
        } else if file_type.is_file() && predicate(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn is_context_path(path: &Path) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("Cargo.toml" | "Cargo.lock" | "rust-toolchain" | "rust-toolchain.toml") => true,
        Some("config" | "config.toml") => path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == ".cargo"),
        _ => false,
    }
}

fn digest_file(root: &Path, path: &Path) -> io::Result<(String, String)> {
    let bytes = fs::read(path)?;
    Ok((
        relative_path(path.strip_prefix(root).unwrap_or(path)),
        sha256(&bytes),
    ))
}

fn find_context_file(files: &[(String, String)], name: &str) -> Option<String> {
    files
        .iter()
        .map(|(path, _)| path)
        .filter(|path| Path::new(path).file_name().and_then(|file| file.to_str()) == Some(name))
        .min()
        .cloned()
}

fn relative_existing_path(root: &Path, path: &Path) -> Option<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let path = path.canonicalize().ok()?;
    Some(relative_path(path.strip_prefix(root).ok()?))
}

fn context_fingerprint(context: &Context) -> String {
    let mut input = Vec::new();
    for value in [
        "zvec-grep.rust-callfacts-context-v1",
        &context.rustc_version,
        &context.rustc_commit,
        &context.host,
        &context.target,
        &context.edition,
        &context.manifest_path,
        context.lockfile_path.as_deref().unwrap_or_default(),
        context.toolchain_path.as_deref().unwrap_or_default(),
    ] {
        push_context_part(&mut input, value);
    }
    for (name, value) in &context.settings {
        push_context_part(&mut input, name);
        push_context_part(&mut input, value);
    }
    let mut files = context.context_files.clone();
    files.sort();
    for (path, digest) in files {
        push_context_part(&mut input, &path);
        push_context_part(&mut input, &digest);
    }
    sha256_bytes(&input)
}

fn push_context_part(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(value.as_bytes());
    output.push(0);
}

fn line_column(source: &[u8], offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut line_start = 0;
    for (index, byte) in source.iter().enumerate().take(offset) {
        if *byte == b'\n' {
            line += 1;
            line_start = index + 1;
        }
    }
    (line, offset - line_start)
}

fn relative_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    path.canonicalize()
}

fn required_argument(arguments: &[String], name: &str) -> io::Result<String> {
    arguments
        .windows(2)
        .find(|window| window[0] == name)
        .map(|window| window[1].clone())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("{name} is required")))
}

fn encode_symbol(symbol: &SymbolRef) -> String {
    format!(
        "{}, {}, {}",
        encode_text(&symbol.path),
        symbol.start_byte,
        symbol.end_byte
    )
    .replace(", ", ",")
}

fn encode_text(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn decode_text(value: &str) -> io::Result<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated percent escape",
                ));
            }
            let high = hex_digit(bytes[index + 1])?;
            let low = hex_digit(bytes[index + 2])?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid UTF-8 field: {error}"),
        )
    })
}

fn hex_digit(byte: u8) -> io::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid percent escape",
        )),
    }
}

fn json_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn sha256(bytes: &[u8]) -> String {
    sha256_bytes(bytes)
}

fn sha256_bytes(input: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (input.len() as u64) * 8;
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut schedule = [0u32; 64];
        for (index, word) in schedule[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }
        let mut working = state;
        for index in 0..64 {
            let choice = (working[4] & working[5]) ^ ((!working[4]) & working[6]);
            let majority =
                (working[0] & working[1]) ^ (working[0] & working[2]) ^ (working[1] & working[2]);
            let sum1 = working[4].rotate_right(6)
                ^ working[4].rotate_right(11)
                ^ working[4].rotate_right(25);
            let sum0 = working[0].rotate_right(2)
                ^ working[0].rotate_right(13)
                ^ working[0].rotate_right(22);
            let temporary1 = working[7]
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let temporary2 = sum0.wrapping_add(majority);
            working[7] = working[6];
            working[6] = working[5];
            working[5] = working[4];
            working[4] = working[3].wrapping_add(temporary1);
            working[3] = working[2];
            working[2] = working[1];
            working[1] = working[0];
            working[0] = temporary1.wrapping_add(temporary2);
        }
        for (state_word, working_word) in state.iter_mut().zip(working) {
            *state_word = state_word.wrapping_add(working_word);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}
