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
    fs, io,
    path::{Path, PathBuf},
};

use crate::{
    EngineError, EngineResult,
    domain::model::EmbeddingModelInfo,
    storage::{IndexStore, types::WorkspaceIndexStorageOptions},
};

use super::{layout::ZVEC_GREP_DIRECTORY, manifest::ManifestState, registry::WorkspaceRegistry};

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
    let dot_git = root.join(".git");
    let metadata = fs::symlink_metadata(&dot_git).ok()?;
    if metadata.file_type().is_symlink() {
        return None;
    }
    if metadata.is_dir() {
        return fs::canonicalize(&dot_git).ok();
    }
    if !metadata.is_file() {
        return None;
    }
    let contents = fs::read_to_string(&dot_git).ok()?;
    let gitdir = contents.strip_prefix("gitdir:")?.trim();
    let gitdir = if Path::new(gitdir).is_absolute() {
        PathBuf::from(gitdir)
    } else {
        root.join(gitdir)
    };
    let gitdir = fs::canonicalize(&gitdir).ok()?;
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

/// Candidate seeds for a workspace: registered indexes of the same repository
/// whose embedding model capsule matches the target exactly and that record a
/// backend identity (unknown provenance is never adoptable). Ordered by
/// preference: rooted at the repository root first, then freshest.
pub(crate) fn seed_candidates(
    registry: &WorkspaceRegistry,
    own_root: &Path,
    capsule: &EmbeddingModelInfo,
) -> EngineResult<Vec<super::manifest::WorkspaceManifest>> {
    let Some(repo) = repo_identity(own_root) else {
        return Ok(Vec::new());
    };
    let repo_root = repo.parent().map(Path::to_path_buf);
    let mut candidates: Vec<(u64, bool, super::manifest::WorkspaceManifest)> = Vec::new();
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
        let Some(embedding) = manifest.embedding() else {
            continue;
        };
        // Model lock: a mismatched model never silently re-embeds. The seed
        // must also record an identity; the caller compares it with the live
        // runtime's, mirroring the cross-runtime refusal of incremental runs.
        if embedding.ensure_model_compatible(capsule).is_err() {
            continue;
        }
        if embedding.backend_identity().is_none() {
            continue;
        }
        let rooted_at_repo = repo_root
            .as_ref()
            .is_some_and(|repo_root| *repo_root == root);
        candidates.push((
            manifest.workspace.updated_epoch_ms,
            rooted_at_repo,
            *manifest,
        ));
    }
    candidates.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.0.cmp(&left.0)));
    Ok(candidates
        .into_iter()
        .map(|(_, _, manifest)| manifest)
        .collect())
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
        use std::os::fd::AsRawFd;
        // Reflink on XFS/btrfs/ZFS; falls back to a full copy on ext4. The
        // ioctl does not carry the source's mode bits, so apply them after.
        const FICLONE: std::os::raw::c_ulong = 0x40049409;
        let input = fs::File::open(source)?;
        let output = fs::File::create(destination)?;
        let result = unsafe { libc::ioctl(output.as_raw_fd(), FICLONE, input.as_raw_fd()) };
        if result == 0 {
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
}
