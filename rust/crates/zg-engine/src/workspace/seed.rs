//! Seeding a new worktree's index from a sibling checkout of the same repo.
//!
//! A fresh worktree otherwise pays a full index build before its first search.
//! When the user's registry already holds an index for the same repository
//! (normally the main checkout's) built by the same embedding model and the
//! same backend identity, that generation seeds the new worktree: it is copied
//! copy-on-write into a staging generation and the ordinary indexing pass then
//! re-embeds only the delta. Branch topology is irrelevant — cost is content
//! divergence, so a stacked change pays its stack, not the corpus.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use zg_host_native::WorkspaceScannerPort;

use crate::{
    EngineError, EngineResult,
    domain::{FileRecord, ScanRules},
    storage::{IndexStore, types::WorkspaceIndexStorageOptions},
};

use super::{
    git::{self, CommitRelation, GitProvenance},
    layout::ZVEC_GREP_DIRECTORY,
    manifest::ManifestState,
    registry::WorkspaceRegistry,
};

/// A selected seed: the donor manifest plus a read-only handle on its active
/// generation. The handle holds the storage's shared cross-process lock, so a
/// concurrent drop or rebuild of the donor cannot delete the generation while
/// the copy runs; drop it (or `close()` it) as soon as the copy finishes.
pub(crate) struct SeedCandidate {
    pub manifest: super::manifest::WorkspaceManifest,
    pub storage: IndexStore,
}

impl SeedCandidate {
    pub(crate) fn storage_home(&self) -> PathBuf {
        self.manifest.storage_home()
    }

    pub(crate) fn close(self) -> EngineResult<()> {
        self.storage.close()
    }
}

/// Identity of the repository a workspace root belongs to: the canonical git
/// common directory. A main checkout contributes its `.git` directory; a linked
/// worktree contributes the common directory behind its `.git` file. Roots
/// without a usable `.git` have no repo identity and never seed or are seeded.
pub(crate) fn repo_identity(root: &Path) -> Option<PathBuf> {
    let gitdir = super::git::resolve_git_dir(root)?;
    // A linked worktree's gitdir is `<common>/worktrees/<name>`; the seed must
    // agree with the main checkout, so compare the common directory.
    let is_worktree = gitdir
        .parent()
        .is_some_and(|parent| parent.file_name() == Some(std::ffi::OsStr::new("worktrees")));
    if is_worktree && let Some(common) = gitdir.parent().and_then(Path::parent) {
        return Some(common.to_path_buf());
    }
    Some(gitdir)
}

/// Candidate donors for a workspace: registered indexes of the same repository
/// that are indexed and record a backend identity (unknown provenance is never
/// adoptable). The embedding model is deliberately **not** filtered here
/// (issue #105): the model may itself be resolved from the selected donor, so
/// capsule compatibility is checked once the model is known, at adoption.
pub(crate) fn donor_candidates(
    registry: &WorkspaceRegistry,
    own_root: &Path,
) -> EngineResult<Vec<super::manifest::WorkspaceManifest>> {
    let Some(repo) = repo_identity(own_root) else {
        return Ok(Vec::new());
    };
    let mut candidates = Vec::new();
    for (_name, root) in registry.entries()? {
        if root == own_root {
            continue;
        }
        if repo_identity(&root).as_deref() != Some(repo.as_path()) {
            continue;
        }
        let home = root.join(ZVEC_GREP_DIRECTORY);
        let Ok(ManifestState::Current(manifest)) =
            super::manifest::inspect_workspace_manifest(&home)
        else {
            continue;
        };
        if !manifest.workspace.index_enabled() || manifest.index_version.is_none() {
            continue;
        }
        if manifest.embedding().is_none() {
            continue;
        }
        if manifest
            .embedding()
            .expect("checked")
            .backend_identity()
            .is_none()
        {
            continue;
        }
        candidates.push(*manifest);
    }
    Ok(candidates)
}

/// A target file discovered by the snapshot scan. The content hash is filled
/// lazily: a file whose size already differs from the donor's record counts as
/// modified without ever being read (issue #105 — hashing reads only the
/// size-equal files, usually the close-donor majority).
#[derive(Clone, Debug)]
pub(crate) struct TargetFile {
    pub size_bytes: u64,
    pub hash: Option<String>,
}

/// The target workspace's discovered file set plus its git provenance,
/// measured once per index request and shared across donor comparisons.
pub(crate) struct TargetSnapshot {
    pub provenance: GitProvenance,
    pub files: HashMap<PathBuf, TargetFile>,
}

/// Measured divergence between a donor generation and the target snapshot
/// (issue #105). `ratio` is the estimated share of embedding work a seed from
/// this donor would still pay: `(added_or_modified + deleted) / union`, where
/// the union counts target files plus donor files that fall inside the
/// target's scan selection (donor files outside it are scan exclusion, not
/// deletion, and cost nothing).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Divergence {
    pub target_files: usize,
    pub matched: usize,
    pub added_or_modified: usize,
    pub deleted: usize,
    pub ratio: f64,
}

impl Divergence {
    /// Divergence in per-mille of the union, clamped to [0, 1000] for
    /// reporting.
    pub(crate) fn permille(&self) -> u64 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the ratio is clamped to [0, 1] before scaling"
        )]
        let permille = (self.ratio.clamp(0.0, 1.0) * 1000.0).round() as u64;
        permille
    }
}

/// Compare a donor's stored file records against the target snapshot. Donor
/// records without a content hash (pending, failed, deleting) never match and
/// are excluded from the union — they are not donor content the target can
/// reuse. Returns `None` when the donor's storage cannot be read (busy,
/// missing): such a donor is skipped, never fatal.
pub(crate) fn measure_divergence(
    donor_records: &[FileRecord],
    snapshot: &mut TargetSnapshot,
    root: &Path,
    selection: &crate::file_selection::GlobMatcher,
) -> Divergence {
    let mut matched = 0_usize;
    let mut deleted = 0_usize;
    for record in donor_records {
        // Only indexed donor content is reusable; pending, failed and deleting
        // records never match (issue #105 review).
        if !record.index_status.is_indexed() {
            continue;
        }
        let Some(donor_hash) = record.snapshot.content_hash.as_deref() else {
            continue;
        };
        let path = record.relative_path.as_path();
        if !selection.matches_path(path) {
            continue; // The target's scan rules exclude it: not a deletion.
        }
        match snapshot.files.get_mut(path) {
            Some(target) => {
                if target.size_bytes != record.snapshot.size_bytes {
                    continue; // Different size: certainly modified, no read needed.
                }
                // Fill the hash lazily (issue #105): only size-equal files are
                // ever read. A read failure counts as modified, never matched.
                let content_matches =
                    target
                        .hash
                        .get_or_insert_with(|| match fs::read(root.join(path)) {
                            Ok(bytes) => crate::utils::sha256_hex(&bytes),
                            Err(_) => "\0unreadable".to_owned(),
                        })
                        == donor_hash;
                if content_matches {
                    matched += 1;
                }
            }
            None => deleted += 1,
        }
    }
    let target_files = snapshot.files.len();
    let added_or_modified = target_files.saturating_sub(matched);
    // union = |target files ∪ in-scope donor files| = target + (in-scope donor absent from target).
    let union = target_files + deleted;
    let ratio = if union == 0 {
        1.0
    } else {
        #[expect(
            clippy::cast_precision_loss,
            reason = "file counts are far below 2^52; the ratio is advisory"
        )]
        let changed = (added_or_modified + deleted) as f64;
        #[expect(
            clippy::cast_precision_loss,
            reason = "file counts are far below 2^52; the ratio is advisory"
        )]
        let total = union as f64;
        changed / total
    };
    Divergence {
        target_files,
        matched,
        added_or_modified,
        deleted,
        ratio,
    }
}

/// A donor with its measured divergence and commit relation, ready for
/// ranking (issue #105).
#[derive(Clone)]
pub(crate) struct RankedSeed {
    pub manifest: super::manifest::WorkspaceManifest,
    pub divergence: Divergence,
    pub relation: CommitRelation,
    /// How far the donor lags the target's commit; `None` when unknown or
    /// the histories are unrelated.
    pub commits_behind: Option<u64>,
}

/// Seed selection outcome: the ranked, guard-passing donors for a target, or
/// the best donor refused by the divergence guard.
pub(crate) struct SeedSelection {
    pub snapshot: TargetSnapshot,
    pub donors: Vec<RankedSeed>,
    /// The best-ranked donor refused by the delta-ratio guard, kept for the
    /// reported refusal reason.
    pub refused: Option<RankedSeed>,
}

impl SeedSelection {
    /// The model reference of the best donor, for donor-model adoption when
    /// the workspace has no model signal of its own.
    pub(crate) fn donor_model(&self) -> Option<String> {
        self.donors.first().map(|donor| {
            donor
                .manifest
                .embedding()
                .expect("donors record an embedding")
                .model
                .reference()
        })
    }
}

/// Maximum estimated re-embed share for which adoption still beats a full
/// build (issue #105). Measured, not guessed: the break-even timing
/// measurement (`seed_guard_break_even_measurement`, run with
/// `cargo test -p zg-engine --lib --release -- --ignored --nocapture
/// seed_guard_break_even_measurement`) compares an adopted build's wall time
/// against a fresh build at increasing divergence on a 120-file corpus
/// (local/potion-code-16m-v2, APFS clonefile). Two runs on arm64 macOS:
/// divergence 0% seed 1.26s vs fresh 2.14s; 50% 1.49s vs 1.60s; 85% 1.50s vs
/// 1.46s — parity. Embedding one corpus costs ~0.9s while the copy-on-write
/// copy plus the lazy-hash measurement cost ~0.10-0.15s, so the break-even
/// sits at roughly 1 - 0.13/0.9 ≈ 0.85-0.95; the guard keeps the conservative
/// end of that band. Filesystems without copy-on-write (the plain `fs::copy`
/// fallback) pay
/// more for the copy, making the guard stricter there, which is the safe
/// direction.
pub(crate) const SEED_MAX_DIVERGENCE_RATIO: f64 = 0.85;

/// Scan the target working tree once for seed selection: the discovered file
/// set (sizes only — hashes fill lazily during comparison) plus git
/// provenance. Runs under the caller's cancellation token like any scan.
pub(crate) async fn snapshot_target(
    scanner: &dyn WorkspaceScannerPort,
    root: &Path,
    scan: &ScanRules,
    signal: Option<&tokio_util::sync::CancellationToken>,
) -> EngineResult<TargetSnapshot> {
    let root_spec = crate::file_selection::ScanPolicy::root_spec(root, scan)?;
    let snapshot = scanner
        .discover(
            &zg_host_native::ScanRequest {
                roots: vec![root_spec],
                scope_paths: Vec::new(),
            },
            &zg_host_native::TaskControl::new(signal.cloned().unwrap_or_default()),
        )
        .await
        .map_err(crate::pipelines::indexing::pipeline::map_host_error)?;
    let files = snapshot
        .files
        .into_iter()
        .map(|file| {
            (
                file.relative_path,
                TargetFile {
                    size_bytes: file.size_bytes,
                    hash: None,
                },
            )
        })
        .collect();
    Ok(TargetSnapshot {
        provenance: git::read_provenance(root).await,
        files,
    })
}

/// Select seed donors for a brand-new workspace by measured divergence
/// instead of wall-clock freshness (issue #105): same commit first, then
/// ancestor/descendant, then content overlap, with freshness only breaking
/// ties. Donors are opened strictly one at a time and closed immediately; a
/// busy or unreadable donor is skipped, never fatal. Returns `None` when the
/// root has no repo identity or no candidate donors exist.
pub(crate) async fn select_seed(
    registry: &WorkspaceRegistry,
    own_root: &Path,
    scanner: &dyn WorkspaceScannerPort,
    scan: &ScanRules,
    selection: &crate::file_selection::GlobMatcher,
    signal: Option<&tokio_util::sync::CancellationToken>,
) -> EngineResult<Option<SeedSelection>> {
    let candidates = donor_candidates(registry, own_root)?;
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut snapshot = snapshot_target(scanner, own_root, scan, signal).await?;
    let Some(repo) = repo_identity(own_root) else {
        return Ok(None);
    };
    let repo_root = repo.parent().map(Path::to_path_buf);
    let target_commit = snapshot.provenance.commit.clone();
    let mut measured: Vec<RankedSeed> = Vec::new();
    for manifest in candidates {
        // One donor at a time, closed immediately (issue #105 review): the
        // shared storage lock must never overlap another candidate or
        // outlive the measurement.
        let Ok(storage) = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
            storage_path: manifest.storage_home(),
        }) else {
            continue; // Busy, missing or unreadable: skip, never fatal.
        };
        let records = storage.list_files();
        let close = storage.close();
        let (Ok(records), Ok(())) = (records, close) else {
            continue;
        };
        let divergence = measure_divergence(&records, &mut snapshot, own_root, selection);
        let relation = git::commit_relation(
            own_root,
            manifest
                .provenance
                .as_ref()
                .and_then(|p| p.commit.as_deref()),
            target_commit.as_deref(),
        )
        .await;
        let commits_behind = match (
            manifest
                .provenance
                .as_ref()
                .and_then(|p| p.commit.as_deref()),
            target_commit.as_deref(),
        ) {
            (Some(donor), Some(target)) if relation == CommitRelation::Ancestor => {
                git::commits_behind(own_root, donor, target).await
            }
            _ => None,
        };
        measured.push(RankedSeed {
            manifest,
            divergence,
            relation,
            commits_behind,
        });
    }
    if measured.is_empty() {
        return Ok(None);
    }
    // Closest first: relation, then measured divergence, then freshness,
    // with the repository root breaking final ties (issue #105).
    measured.sort_by(|left, right| {
        left.relation
            .rank()
            .cmp(&right.relation.rank())
            .then(
                left.divergence
                    .ratio
                    .partial_cmp(&right.divergence.ratio)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then_with(|| {
                right
                    .manifest
                    .workspace
                    .updated_epoch_ms
                    .cmp(&left.manifest.workspace.updated_epoch_ms)
            })
            .then_with(|| {
                let rooted = |manifest: &super::manifest::WorkspaceManifest| {
                    repo_root
                        .as_ref()
                        .is_some_and(|repo_root| *repo_root == manifest.workspace.root)
                };
                rooted(&right.manifest).cmp(&rooted(&left.manifest))
            })
    });
    // Delta-ratio guard (issue #105): a donor whose estimated re-embed exceeds
    // the measured threshold is worse than a clean build — the copy is paid
    // and nearly everything re-embeds anyway.
    let refused = measured
        .first()
        .filter(|donor| donor.divergence.ratio > SEED_MAX_DIVERGENCE_RATIO)
        .cloned();
    let donors: Vec<RankedSeed> = measured
        .into_iter()
        .filter(|donor| donor.divergence.ratio <= SEED_MAX_DIVERGENCE_RATIO)
        .collect();
    if donors.is_empty() && refused.is_none() {
        return Ok(None);
    }
    Ok(Some(SeedSelection {
        snapshot,
        donors,
        refused,
    }))
}

/// The reported reason a donor was declined by the divergence guard.
pub(crate) fn refusal_reason(donor: &RankedSeed) -> String {
    if donor.divergence.target_files == 0 {
        return format!(
            "seed donor {}: the target scan selects no files to reuse; building fresh",
            donor.manifest.workspace.root.display(),
        );
    }
    format!(
        "seed donor {} estimated {:.0}% re-embed ({} of {} files) exceeds the {:.0}% adoption threshold; building fresh",
        donor.manifest.workspace.root.display(),
        donor.divergence.ratio * 100.0,
        donor.divergence.added_or_modified + donor.divergence.deleted,
        donor.divergence.target_files + donor.divergence.deleted,
        SEED_MAX_DIVERGENCE_RATIO * 100.0,
    )
}

/// Re-read a donor's manifest from its registered root, for the copy-time
/// freshness check. `None` when the donor is gone or unreadable.
pub(crate) fn reinspect_donor(
    manifest: &super::manifest::WorkspaceManifest,
) -> Option<super::manifest::WorkspaceManifest> {
    let home = manifest
        .workspace
        .root
        .join(super::layout::ZVEC_GREP_DIRECTORY);
    match super::manifest::inspect_workspace_manifest(&home) {
        Ok(ManifestState::Current(current)) => Some(*current),
        _ => None,
    }
}

/// Re-measure a donor that was rebuilt since selection: refresh its
/// divergence and relation, and re-apply the guard. `None` when the donor is
/// busy, unreadable or now over the threshold.
pub(crate) async fn remeasure_donor(
    current: &super::manifest::WorkspaceManifest,
    snapshot: &mut TargetSnapshot,
    root: &Path,
    selection: &crate::file_selection::GlobMatcher,
) -> Option<RankedSeed> {
    let Ok(storage) = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
        storage_path: current.storage_home(),
    }) else {
        return None;
    };
    let records = storage.list_files();
    let close = storage.close();
    let (Ok(records), Ok(())) = (records, close) else {
        return None;
    };
    let divergence = measure_divergence(&records, snapshot, root, selection);
    if divergence.ratio > SEED_MAX_DIVERGENCE_RATIO {
        return None;
    }
    let relation = git::commit_relation(
        root,
        current
            .provenance
            .as_ref()
            .and_then(|p| p.commit.as_deref()),
        snapshot.provenance.commit.as_deref(),
    )
    .await;
    let commits_behind = match (
        current
            .provenance
            .as_ref()
            .and_then(|p| p.commit.as_deref()),
        snapshot.provenance.commit.as_deref(),
    ) {
        (Some(donor), Some(target)) if relation == CommitRelation::Ancestor => {
            git::commits_behind(root, donor, target).await
        }
        _ => None,
    };
    Some(RankedSeed {
        manifest: current.clone(),
        divergence,
        relation,
        commits_behind,
    })
}

/// Open the first candidate whose recorded backend identity matches the live
/// runtime's. The read-only handle holds the storage's shared cross-process
/// lock, so a concurrent drop or rebuild of the donor cannot delete the
/// generation while the copy runs; close it as soon as the copy finishes.
/// Candidates that are being rewritten or whose storage cannot be opened are
/// skipped, never fatal: adoption is an optimization.
pub(crate) fn open_seed(
    candidates: Vec<super::manifest::WorkspaceManifest>,
    live: &crate::domain::model::BackendIdentity,
) -> Option<SeedCandidate> {
    for manifest in candidates {
        if manifest
            .embedding()
            .expect("candidates record an embedding")
            .ensure_backend_compatible(Some(live))
            .is_err()
        {
            continue;
        }
        let Ok(storage) = IndexStore::open(WorkspaceIndexStorageOptions::ReadOnly {
            storage_path: manifest.storage_home(),
        }) else {
            continue;
        };
        return Some(SeedCandidate { manifest, storage });
    }
    None
}

/// Copy a directory tree copy-on-write where the filesystem supports it
/// (APFS `clonefile`, Linux `FICLONE`), falling back to a full copy. Never
/// hardlinks: the destination is writable and must not alias the seed's
/// blocks. Symlinks are recreated as symlinks, never dereferenced.
pub(crate) fn copy_tree_cow(source: &Path, destination: &Path) -> EngineResult<()> {
    copy_entry(source, destination)
}

fn copy_entry(source: &Path, destination: &Path) -> EngineResult<()> {
    let metadata =
        fs::symlink_metadata(source).map_err(|error| seed_io("inspect", source, &error))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(source).map_err(|error| seed_io("read link", source, &error))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, destination)
            .map_err(|error| seed_io("recreate link", destination, &error))?;
        #[cfg(not(unix))]
        {
            let _ = target;
            return Err(EngineError::unsupported(
                "symlinks inside workspace storage cannot be copied on this platform",
            ));
        }
        return Ok(());
    }
    if file_type.is_dir() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(destination)
            .map_err(|error| seed_io("create directory", destination, &error))?;
        let entries =
            fs::read_dir(source).map_err(|error| seed_io("read directory", source, &error))?;
        for entry in entries {
            let entry = entry.map_err(|error| seed_io("read directory", source, &error))?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if file_type.is_file() {
        cow_copy_file(source, destination)
            .map_err(|error| seed_io("copy file", destination, &error))?;
        return Ok(());
    }
    Err(EngineError::unsupported(format!(
        "unexpected file type inside workspace storage: {}",
        source.display()
    )))
}

fn cow_copy_file(source: &Path, destination: &Path) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
    }
    #[cfg(target_os = "macos")]
    {
        // APFS clonefile: the destination must not exist and both paths must
        // be on the same volume; any failure falls back to a plain copy.
        if let Ok(input) = fs::File::open(source)
            && let Some(parent) = destination.parent()
            && let Ok(directory) = fs::File::open(parent)
            && let Some(name) = destination.file_name()
            && rustix::fs::fclonefileat(&input, &directory, name, rustix::fs::CloneFlags::empty())
                .is_ok()
        {
            return Ok(());
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Reflink on XFS/btrfs/ZFS; falls back to a full copy on ext4 and on
        // architectures whose ioctl encoding the quarantined call does not
        // cover. The ioctl does not carry the source's mode bits, so apply
        // them after.
        let input = fs::File::open(source)?;
        let output = fs::File::create(destination)?;
        if zg_ficlone::ficlone(&output, &input).is_ok() {
            drop(output);
            fs::set_permissions(destination, fs::metadata(source)?.permissions())?;
            return Ok(());
        }
    }
    fs::copy(source, destination)?;
    Ok(())
}

fn seed_io(operation: &str, path: &Path, error: &io::Error) -> EngineError {
    EngineError::from_io(
        format!("seed copy failed to {operation} {}", path.display()),
        error,
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;

    fn write_git_dir(root: &Path) {
        fs::create_dir_all(root.join(".git")).expect("git dir");
    }

    /// A linked worktree: a `.git` file pointing at `<main>/.git/worktrees/<name>`.
    fn write_git_file(root: &Path, main: &Path, name: &str) {
        let gitdir = main.join(".git/worktrees").join(name);
        fs::create_dir_all(&gitdir).expect("worktree gitdir");
        fs::write(root.join(".git"), format!("gitdir: {}\n", gitdir.display())).expect("git file");
    }

    #[test]
    fn repo_identity_unifies_main_checkouts_and_linked_worktrees() {
        let directory = tempdir().expect("temporary directory");
        let main = directory.path().join("main");
        let worktree = directory.path().join("wt");
        let sibling = directory.path().join("sibling");
        fs::create_dir(&main).expect("main root");
        fs::create_dir(&worktree).expect("worktree root");
        fs::create_dir(&sibling).expect("sibling root");
        write_git_dir(&main);
        write_git_dir(&sibling);
        write_git_file(&worktree, &main, "wt");
        // A relative gitdir resolves against the worktree root.
        let relative = directory.path().join("relative");
        fs::create_dir(&relative).expect("relative root");
        let gitdir = main.join(".git/worktrees/relative");
        fs::create_dir_all(&gitdir).expect("relative gitdir");
        fs::write(
            relative.join(".git"),
            "gitdir: ../main/.git/worktrees/relative\n",
        )
        .expect("git file");

        let identity = repo_identity(&main).expect("main identity");
        assert_eq!(
            repo_identity(&worktree).as_deref(),
            Some(identity.as_path())
        );
        assert_eq!(
            repo_identity(&relative).as_deref(),
            Some(identity.as_path())
        );
        assert_ne!(repo_identity(&sibling).as_deref(), Some(identity.as_path()));
    }

    #[test]
    fn repo_identity_is_absent_without_a_usable_git_directory() {
        let directory = tempdir().expect("temporary directory");
        let plain = directory.path().join("plain");
        let broken = directory.path().join("broken");
        let linked = directory.path().join("linked");
        fs::create_dir(&plain).expect("plain root");
        fs::create_dir(&broken).expect("broken root");
        fs::create_dir(&linked).expect("linked root");
        fs::write(broken.join(".git"), "not a gitdir pointer").expect("malformed");
        fs::write(linked.join(".git"), "gitdir: missing/worktrees/x").expect("dangling");
        assert_eq!(repo_identity(&plain), None);
        assert_eq!(repo_identity(&broken), None);
        assert_eq!(repo_identity(&linked), None);
    }

    fn sample_tree(parent: &Path) {
        fs::create_dir_all(parent.join("nested/deeper")).expect("nested directories");
        let mut file = fs::File::create(parent.join("top.bin")).expect("top file");
        file.write_all(b"top level bytes").expect("top contents");
        let mut file = fs::File::create(parent.join("nested/deeper/leaf.bin")).expect("leaf");
        file.write_all(b"leaf bytes").expect("leaf contents");
        fs::write(parent.join("nested/empty.bin"), b"").expect("empty file");
    }

    fn assert_tree_equal(left: &Path, right: &Path) {
        assert_eq!(
            fs::read(left.join("top.bin")).expect("left top"),
            fs::read(right.join("top.bin")).expect("right top")
        );
        assert_eq!(
            fs::read(left.join("nested/deeper/leaf.bin")).expect("left leaf"),
            fs::read(right.join("nested/deeper/leaf.bin")).expect("right leaf")
        );
        assert!(right.join("nested/empty.bin").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn copied_files_are_never_hardlinks() {
        use std::os::unix::fs::MetadataExt;

        let directory = tempdir().expect("temporary directory");
        let source = directory.path().join("source");
        fs::create_dir(&source).expect("source directory");
        let mut file = fs::File::create(source.join("data.bin")).expect("source file");
        file.write_all(b"clone me").expect("contents");
        let destination = directory.path().join("copy/data.bin");
        copy_tree_cow(&source.join("data.bin"), &destination).expect("copy file");
        assert_eq!(
            fs::read(&destination).expect("copied contents"),
            b"clone me"
        );
        assert_eq!(
            fs::metadata(&destination).expect("copy metadata").nlink(),
            1,
            "a seed copy must never hardlink the writable index"
        );
    }

    #[test]
    fn copy_tree_replicates_nested_trees_and_empty_files() {
        let directory = tempdir().expect("temporary directory");
        let source = directory.path().join("generation");
        sample_tree(&source);
        let destination = directory.path().join("adopted/generation");
        copy_tree_cow(&source, &destination).expect("copy generation");
        assert_tree_equal(&source, &destination);
        // The source stays intact for its own workspace.
        assert!(source.join("top.bin").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_recreates_symlinks_without_dereferencing_them() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().expect("temporary directory");
        let source = directory.path().join("generation");
        fs::create_dir_all(source.join("storage")).expect("storage directory");
        let secret = directory.path().join("secret.txt");
        fs::write(&secret, b"outside the tree").expect("outside file");
        symlink("../../secret.txt", source.join("storage/link")).expect("outward symlink");
        let destination = directory.path().join("adopted/generation");
        copy_tree_cow(&source, &destination).expect("copy with symlink");
        let link = destination.join("storage/link");
        assert!(
            fs::symlink_metadata(&link)
                .expect("link metadata")
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(&link).expect("link target"),
            Path::new("../../secret.txt")
        );
        // The copy never read the target through the link.
        assert!(!link.is_file());
    }

    #[test]
    fn copy_tree_reports_missing_sources_without_creating_the_destination() {
        let directory = tempdir().expect("temporary directory");
        let destination = directory.path().join("adopted/generation");
        let error = copy_tree_cow(&directory.path().join("missing"), &destination)
            .expect_err("missing source");
        assert!(error.message().contains("seed copy"));
        assert!(!destination.exists());
    }

    /// Divergence measurement units (issue #105): the ranking and guard both
    /// build on `measure_divergence`, so its matching, lazy hashing and
    /// scan-rule scoping are pinned here without a workspace.
    mod divergence {
        use crate::domain::{FileId, FileIndexStatus, FileRecord, FileSnapshot, SourcePath};

        use super::super::{GitProvenance, TargetFile, TargetSnapshot, measure_divergence};
        use crate::domain::ScanRules;
        use crate::utils::sha256_hex;

        fn record(path: &str, bytes: &[u8], indexed: bool) -> FileRecord {
            FileRecord {
                id: FileId::new(1),
                relative_path: SourcePath::new(path).expect("path"),
                snapshot: FileSnapshot {
                    size_bytes: bytes.len() as u64,
                    modified_epoch_ms: None,
                    // Even failed records can carry a hash from an earlier
                    // success; the filter must look at the status, not the hash.
                    content_hash: Some(sha256_hex(bytes)),
                },
                index_status: if indexed {
                    FileIndexStatus::Indexed {
                        indexed_epoch_ms: 1,
                        entity_count: 1,
                    }
                } else {
                    FileIndexStatus::Failed {
                        error: "fixture".into(),
                    }
                },
            }
        }

        fn snapshot(entries: &[(&str, &str)]) -> (tempfile::TempDir, TargetSnapshot) {
            let directory = tempfile::tempdir().expect("temporary directory");
            let mut files = std::collections::HashMap::new();
            for (path, contents) in entries {
                std::fs::write(directory.path().join(path), contents).expect("target file");
                files.insert(
                    std::path::PathBuf::from(path),
                    TargetFile {
                        size_bytes: contents.len() as u64,
                        hash: None,
                    },
                );
            }
            let snapshot = TargetSnapshot {
                provenance: GitProvenance::default(),
                files,
            };
            (directory, snapshot)
        }

        fn matcher(root: &std::path::Path) -> crate::file_selection::GlobMatcher {
            crate::file_selection::GlobMatcher::new(root, &ScanRules::default().globs)
                .expect("matcher")
        }

        #[test]
        fn identical_content_matches_without_divergence() {
            let (directory, mut target) = snapshot(&[("one.txt", "shared bytes")]);
            let donor = [record("one.txt", b"shared bytes", true)];
            let divergence = measure_divergence(
                &donor,
                &mut target,
                directory.path(),
                &matcher(directory.path()),
            );
            assert_eq!(divergence.target_files, 1);
            assert_eq!(divergence.matched, 1);
            assert_eq!(divergence.added_or_modified, 0);
            assert_eq!(divergence.deleted, 0);
            assert!((divergence.ratio - 0.0).abs() < 1e-9);
            // The size-equal file was hashed lazily during comparison.
            assert!(target.files[std::path::Path::new("one.txt")].hash.is_some());
        }

        #[test]
        fn a_size_difference_counts_as_modified_without_reading_the_file() {
            let (directory, mut target) = snapshot(&[("one.txt", "longer target bytes")]);
            let donor = [record("one.txt", b"short", true)];
            let divergence = measure_divergence(
                &donor,
                &mut target,
                directory.path(),
                &matcher(directory.path()),
            );
            assert_eq!(divergence.matched, 0);
            assert_eq!(divergence.added_or_modified, 1);
            assert!((divergence.ratio - 1.0).abs() < 1e-9);
            // Never read: the lazy hash stays unset.
            assert!(target.files[std::path::Path::new("one.txt")].hash.is_none());
        }

        #[test]
        fn same_size_different_bytes_count_as_modified() {
            let (directory, mut target) = snapshot(&[("one.txt", "target bytes")]);
            let donor = [record("one.txt", b"donor bytes", true)];
            let divergence = measure_divergence(
                &donor,
                &mut target,
                directory.path(),
                &matcher(directory.path()),
            );
            assert_eq!(divergence.matched, 0);
            assert_eq!(divergence.added_or_modified, 1);
        }

        #[test]
        fn donor_files_missing_from_the_target_count_as_deletions_in_the_union() {
            let (directory, mut target) = snapshot(&[("one.txt", "target bytes")]);
            let donor = [
                record("one.txt", b"target bytes", true),
                record("gone.txt", b"deleted on the branch", true),
            ];
            let divergence = measure_divergence(
                &donor,
                &mut target,
                directory.path(),
                &matcher(directory.path()),
            );
            assert_eq!(divergence.matched, 1);
            assert_eq!(divergence.deleted, 1);
            // union = 1 target + 1 deleted = 2; re-embed = 0 added + 1 deleted.
            assert!((divergence.ratio - 0.5).abs() < 1e-9);
        }

        #[test]
        fn donor_files_outside_the_targets_selection_are_scan_exclusion_not_deletion() {
            let (directory, mut target) = snapshot(&[("one.txt", "target bytes")]);
            // The target excludes two.txt with a glob.
            let rules = crate::domain::ScanRules {
                globs: vec![crate::domain::GlobRule::from("!two.txt")],
                ..ScanRules::default()
            };
            let selection = crate::file_selection::GlobMatcher::new(directory.path(), &rules.globs)
                .expect("matcher");
            let donor = [
                record("one.txt", b"target bytes", true),
                record("two.txt", b"excluded by the target scan", true),
            ];
            let divergence = measure_divergence(&donor, &mut target, directory.path(), &selection);
            assert_eq!(divergence.deleted, 0, "scan exclusion is not a deletion");
            assert!((divergence.ratio - 0.0).abs() < 1e-9);
        }

        #[test]
        fn unreadable_target_files_count_as_modified_never_matched() {
            let (directory, mut target) = snapshot(&[]);
            // The snapshot claims a file the root does not have: reading fails
            // and the file must count as modified, never matched.
            target.files.insert(
                "one.txt".into(),
                TargetFile {
                    size_bytes: 5,
                    hash: None,
                },
            );
            let donor = [record("one.txt", b"bytes", true)];
            let divergence = measure_divergence(
                &donor,
                &mut target,
                directory.path(),
                &matcher(directory.path()),
            );
            assert_eq!(divergence.matched, 0);
            assert_eq!(divergence.added_or_modified, 1);
        }

        #[test]
        fn non_indexed_donor_records_never_match_or_count() {
            let (directory, mut target) = snapshot(&[("one.txt", "target bytes")]);
            let donor = [
                record("one.txt", b"target bytes", true),
                record("failed.txt", b"failed to index", false),
            ];
            let divergence = measure_divergence(
                &donor,
                &mut target,
                directory.path(),
                &matcher(directory.path()),
            );
            assert_eq!(divergence.matched, 1);
            assert_eq!(
                divergence.deleted, 0,
                "a failed donor record is not donor content"
            );
        }

        #[test]
        fn an_empty_target_never_seeds() {
            let (directory, mut target) = snapshot(&[]);
            let donor = [record("one.txt", b"bytes", true)];
            let divergence = measure_divergence(
                &donor,
                &mut target,
                directory.path(),
                &matcher(directory.path()),
            );
            assert!(
                (divergence.ratio - 1.0).abs() < 1e-9,
                "an empty union means nothing to reuse"
            );
        }
    }
}
