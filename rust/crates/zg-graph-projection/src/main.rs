//! `zg-graph-project`: projects an upstream zvec-grep in-engine graph store
//! into a codegraph-v2 sidecar our `zg --graph-query` / MCP tools can query.
//!
//! Usage: `zg-graph-project <workspace-root-or-store-dir> [--output PATH]`

use std::path::PathBuf;

use clap::Parser;

use zg_graph_projection::{locate_upstream_store, project_upstream_graph};

#[derive(Debug, Parser)]
#[command(about = "Project an upstream zvec-grep graph store into a codegraph-v2 sidecar")]
struct Args {
    /// Workspace root (containing `.zvec-grep/`) or the index store directory
    /// itself (the `.zvec-grep/index` directory).
    root: PathBuf,
    /// Output artifact path. Defaults to `<workspace>/.zvec-grep/codegraph-v2.json.zst`.
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    let store = if locate_upstream_store(&args.root).is_some() {
        locate_upstream_store(&args.root).expect("checked above")
    } else if args.root.join("graph").join("graph.sqlite").is_file() {
        args.root.clone()
    } else {
        eprintln!(
            "error: no upstream graph store found under {} (expected .zvec-grep/index/graph/graph.sqlite)",
            args.root.display()
        );
        std::process::exit(1);
    };
    match project_upstream_graph(&store, args.output.as_deref()) {
        Ok((path, artifact, report)) => {
            println!(
                "projected {} files, {} symbol nodes, {} file nodes, {} edges \
                 ({} resolved / {} pending / {} failed, {} declares dropped, \
                 {} dangling endpoints) -> {}",
                report.files,
                report.symbol_nodes,
                report.file_nodes,
                report.edges,
                report.resolved_edges,
                report.pending_edges,
                report.failed_edges,
                report.dropped_declares,
                report.dangling_endpoints,
                path.display()
            );
            println!(
                "query with: zg --graph-query {} blast-radius <symbol>",
                path.display()
            );
            let _ = artifact;
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}
