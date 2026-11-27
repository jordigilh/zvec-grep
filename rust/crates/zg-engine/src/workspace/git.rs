//! Read-only git provenance for a workspace root (issue #105).
//!
//! Provenance is advisory metadata: the commit, branch label and dirty flag of
//! the working tree a generation was built from. It never participates in
//! backend identity, model capsules or seed authorization, and nothing here
//! mutates refs or the working tree. Every git subprocess is read-only, runs
//! under a timeout, and degrades to `None` on failure — a slow or missing git
//! never fails an index request.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

/// Timeout for cheap plumbing queries (`merge-base`, `rev-list`, `rev-parse`).
const GIT_QUERY_TIMEOUT: Duration = Duration::from_secs(3);
/// `git status` walks the index and the tree; large checkouts need longer.
const GIT_STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// Git state of a working tree, recorded per generation at publication.
/// Every field degrades to `None` independently: a detached HEAD records a
/// commit but no branch; a missing `git` binary records no dirty flag.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct GitProvenance {
    /// Full commit id of `HEAD`, when resolvable without the git binary.
    pub commit: Option<String>,
    /// Branch label of `HEAD` (`ref: refs/heads/<label>`); `None` when detached.
    pub branch: Option<String>,
    /// Whether the working tree differs from `HEAD`. `None` when unknown.
    pub dirty: Option<bool>,
}

impl GitProvenance {
    pub(crate) fn is_known(&self) -> bool {
        self.commit.is_some() || self.branch.is_some() || self.dirty.is_some()
    }
}

/// Resolve a root's `.git` entry to the git directory itself: a main
/// checkout's `.git` directory, or the gitdir a linked worktree's `.git` file
/// points at. Symlinked and malformed `.git` entries resolve to `None`.
pub(crate) fn resolve_git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let metadata = std::fs::symlink_metadata(&dot_git).ok()?;
    if metadata.file_type().is_symlink() {
        return None;
    }
    if metadata.is_dir() {
        return std::fs::canonicalize(&dot_git).ok();
    }
    if !metadata.is_file() {
        return None;
    }
    let contents = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir = contents.strip_prefix("gitdir:")?.trim();
    let gitdir = if Path::new(gitdir).is_absolute() {
        PathBuf::from(gitdir)
    } else {
        root.join(gitdir)
    };
    std::fs::canonicalize(&gitdir).ok()
}

/// The repository's common directory: a linked worktree's gitdir lives under
/// `<common>/worktrees/<name>`, and its refs resolve against the common dir.
fn common_dir(git_dir: &Path) -> PathBuf {
    if git_dir
        .parent()
        .is_some_and(|parent| parent.file_name() == Some(std::ffi::OsStr::new("worktrees")))
    {
        git_dir
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| git_dir.to_path_buf(), Path::to_path_buf)
    } else {
        git_dir.to_path_buf()
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|contents| contents.trim().to_owned())
        .filter(|contents| !contents.is_empty())
}

fn is_commit_id(value: &str) -> bool {
    value.len() >= 7 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) && !value.contains('/')
}

/// Branch label and commit of `HEAD`, read from the git directory without a
/// subprocess. A worktree's `HEAD` is local to the worktree; refs resolve
/// against the worktree gitdir first, then the common directory and its
/// packed-refs.
fn head_state(git_dir: &Path) -> (Option<String>, Option<String>) {
    let Some(head) = read_trimmed(&git_dir.join("HEAD")) else {
        return (None, None);
    };
    if let Some(reference) = head.strip_prefix("ref: ") {
        let reference = reference.trim();
        let branch = reference.strip_prefix("refs/heads/").map(str::to_owned);
        return (branch, resolve_ref(git_dir, reference));
    }
    if is_commit_id(&head) {
        return (None, Some(head));
    }
    (None, None)
}

fn resolve_ref(git_dir: &Path, reference: &str) -> Option<String> {
    for base in [git_dir.to_path_buf(), common_dir(git_dir)] {
        if let Some(value) = read_trimmed(&base.join(reference))
            && is_commit_id(&value)
        {
            return Some(value);
        }
    }
    // Packed refs: "<commit> <reference>" lines in the common directory.
    let packed = read_trimmed(&common_dir(git_dir).join("packed-refs"))?;
    packed.lines().find_map(|line| {
        let (commit, name) = line.split_once(' ')?;
        (name.trim() == reference && is_commit_id(commit)).then(|| commit.to_owned())
    })
}

/// Read the provenance of a working tree. The commit and branch come from the
/// git directory directly; the dirty flag shells out to
/// `git --no-optional-locks status --porcelain`, which never touches refs.
pub(crate) async fn read_provenance(root: &Path) -> GitProvenance {
    let (branch, commit, own_git_dir) = match resolve_git_dir(root) {
        Some(git_dir) => {
            let (branch, commit) = head_state(&git_dir);
            (branch, commit, true)
        }
        None => (None, None, false),
    };
    // Only a root with its own git directory has a meaningful dirty flag: a
    // bare `git status` in a subdirectory of a larger repository would report
    // the parent repo's whole tree, which says nothing about this workspace
    // (and would spawn a doomed subprocess for roots outside any repo).
    let dirty = if own_git_dir {
        read_dirty(root).await
    } else {
        None
    };
    // Reftable repositories (and other layouts without a loose or packed ref
    // for HEAD) lose the commit in the direct lookup: fall back to bounded
    // read-only plumbing so same-commit ranking still works there.
    let commit = match commit {
        Some(commit) => Some(commit),
        None if own_git_dir => rev_parse_head(root).await,
        None => None,
    };
    GitProvenance {
        commit,
        branch,
        dirty,
    }
}

/// Resolve the commit id of `HEAD` with read-only plumbing; `None` on
/// failure, timeout, or a non-commit answer.
async fn rev_parse_head(root: &Path) -> Option<String> {
    let output = git(root, &["rev-parse", "HEAD"], GIT_QUERY_TIMEOUT)
        .await?
        .success()?;
    let commit = output.trim().to_owned();
    is_commit_id(&commit).then_some(commit)
}

async fn read_dirty(root: &Path) -> Option<bool> {
    // The workspace's own index directory is untracked content that says
    // nothing about the source tree, so it is excluded from the dirty check;
    // without this every indexed workspace would record `dirty: true`.
    // The untracked policy is pinned (`status.showUntrackedFiles` could
    // otherwise hide new source files) and the optional machinery a
    // repository may configure (fsmonitor, untracked cache) is disabled for
    // this invocation, so no repository-configured command ever runs.
    let output = git(
        root,
        &[
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--",
            ":(exclude).zvec-grep",
        ],
        GIT_STATUS_TIMEOUT,
    )
    .await?
    .success()?;
    Some(!output.is_empty())
}

/// How a donor's commit relates to the target's (issue #105 ranking key).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommitRelation {
    /// Both record the same commit id.
    Same,
    /// One is an ancestor of the other.
    Ancestor,
    /// Histories are unrelated or git could not answer in time.
    Unrelated,
}

impl CommitRelation {
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::Same => 0,
            Self::Ancestor => 1,
            Self::Unrelated => 2,
        }
    }
}

/// Classify the relation between two commits with read-only plumbing.
/// Unknown commits (dirty-only provenance, legacy manifests, subprocess
/// failure or timeout) classify as unrelated — the ranking then falls back to
/// measured content overlap.
pub(crate) async fn commit_relation(
    root: &Path,
    left: Option<&str>,
    right: Option<&str>,
) -> CommitRelation {
    let (Some(left), Some(right)) = (left, right) else {
        return CommitRelation::Unrelated;
    };
    if left == right {
        return CommitRelation::Same;
    }
    let (Some(forward), Some(backward)) = (
        is_ancestor(root, left, right).await,
        is_ancestor(root, right, left).await,
    ) else {
        return CommitRelation::Unrelated;
    };
    if forward || backward {
        CommitRelation::Ancestor
    } else {
        CommitRelation::Unrelated
    }
}

/// How many commits `right` has that `left` lacks — how far `left` lags
/// `right`. `None` for unknown commits, unrelated histories or failure.
pub(crate) async fn commits_behind(root: &Path, left: &str, right: &str) -> Option<u64> {
    let range = format!("{left}..{right}");
    let output = git(root, &["rev-list", "--count", &range], GIT_QUERY_TIMEOUT)
        .await?
        .success()?;
    output.trim().parse().ok()
}

async fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Option<bool> {
    let status = tokio::time::timeout(
        GIT_QUERY_TIMEOUT,
        tokio::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // A timed-out query must not leak its child process.
            .kill_on_drop(true)
            .status(),
    )
    .await
    .ok()?
    .ok()?;
    // 0: ancestor; 1: not an ancestor; anything else is unusable.
    Some(status.code() == Some(0))
}

struct GitOutput {
    stdout: String,
    code: i32,
}

impl GitOutput {
    fn success(self) -> Option<String> {
        (self.code == 0).then_some(self.stdout)
    }
}

/// Run a read-only git query under a timeout. `None` on spawn failure,
/// timeout, or a killed process: provenance degrades, it never fails.
async fn git(root: &Path, args: &[&str], timeout: Duration) -> Option<GitOutput> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // A timed-out query must not leak its child process (review finding).
        .kill_on_drop(true);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .ok()?
        .ok()?;
    Some(GitOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        code: output.status.code()?,
    })
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn write_repo(root: &Path, head: &str) {
        let git_dir = root.join(".git");
        std::fs::create_dir_all(git_dir.join("refs/heads")).expect("git dir");
        std::fs::write(git_dir.join("HEAD"), head).expect("HEAD");
    }

    #[test]
    fn head_state_reads_branch_labels_and_commits() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path().join("repo");
        std::fs::create_dir(&root).expect("root");
        write_repo(&root, "ref: refs/heads/release/v1.6\n");
        std::fs::create_dir_all(root.join(".git/refs/heads/release")).expect("nested ref dir");
        std::fs::write(
            root.join(".git/refs/heads/release/v1.6"),
            "abc123def0000000000000000000000000000000\n",
        )
        .expect("ref");
        let git_dir = root.join(".git");
        assert_eq!(
            head_state(&git_dir),
            (
                Some("release/v1.6".into()),
                Some("abc123def0000000000000000000000000000000".into())
            )
        );
        // Packed refs resolve when the loose ref is absent.
        std::fs::remove_file(root.join(".git/refs/heads/release/v1.6")).expect("remove loose ref");
        std::fs::write(
            root.join(".git/packed-refs"),
            "abc123def0000000000000000000000000000000 refs/heads/release/v1.6\n",
        )
        .expect("packed refs");
        assert_eq!(
            head_state(&git_dir),
            (
                Some("release/v1.6".into()),
                Some("abc123def0000000000000000000000000000000".into())
            )
        );
        // Detached HEAD records a commit without a branch.
        std::fs::write(
            root.join(".git/HEAD"),
            "1234567890abcdef1234567890abcdef12345678\n",
        )
        .expect("detached");
        assert_eq!(
            head_state(&git_dir),
            (
                None,
                Some("1234567890abcdef1234567890abcdef12345678".into())
            )
        );
    }

    #[test]
    fn head_state_resolves_worktree_refs_against_the_common_dir() {
        let directory = tempdir().expect("temporary directory");
        let main = directory.path().join("main");
        let worktree = directory.path().join("wt");
        std::fs::create_dir(&main).expect("main");
        std::fs::create_dir(&worktree).expect("worktree");
        let gitdir = main.join(".git/worktrees/wt");
        std::fs::create_dir_all(&gitdir).expect("worktree gitdir");
        std::fs::create_dir_all(main.join(".git/refs/heads")).expect("common refs");
        std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/feature\n").expect("HEAD");
        std::fs::write(
            main.join(".git/refs/heads/feature"),
            "feed000000000000000000000000000000000000\n",
        )
        .expect("common ref");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .expect("git file");
        let resolved = resolve_git_dir(&worktree).expect("worktree gitdir");
        assert_eq!(
            head_state(&resolved),
            (
                Some("feature".into()),
                Some("feed000000000000000000000000000000000000".into())
            )
        );
    }

    #[test]
    fn head_state_is_empty_without_git_metadata() {
        let directory = tempdir().expect("temporary directory");
        assert_eq!(head_state(directory.path()), (None, None));
    }

    #[tokio::test]
    async fn provenance_degrades_without_git() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path().join("plain");
        std::fs::create_dir(&root).expect("root");
        let provenance = read_provenance(&root).await;
        assert_eq!(provenance, GitProvenance::default());
        assert!(!provenance.is_known());
    }

    #[tokio::test]
    async fn provenance_reads_commit_branch_and_dirty_from_a_real_repo() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path();
        for args in [
            vec!["init", "--quiet"],
            vec!["config", "user.email", "test@example.test"],
            vec!["config", "user.name", "Test"],
        ] {
            let status = std::process::Command::new("git")
                .args(&args)
                .current_dir(root)
                .status()
                .expect("git available on the test host");
            assert!(status.success(), "git {args:?}");
        }
        std::fs::write(root.join("file.txt"), "content").expect("source");
        let status = std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(root)
            .status()
            .expect("git add");
        assert!(status.success());
        let status = std::process::Command::new("git")
            .args(["commit", "--quiet", "--message", "initial"])
            .current_dir(root)
            .status()
            .expect("git commit");
        assert!(status.success());

        let clean = read_provenance(root).await;
        assert!(clean.commit.is_some(), "HEAD resolves to a commit");
        // The initial branch name follows the host git's default (main or
        // master); both are valid branch labels for this test.
        assert!(matches!(clean.branch.as_deref(), Some("main" | "master")));
        // The workspace's own index directory is untracked but excluded from
        // the dirty check (issue #105 review finding). A file inside it makes
        // the exclusion meaningful: git ignores empty directories.
        std::fs::create_dir(root.join(".zvec-grep")).expect("index directory");
        std::fs::write(root.join(".zvec-grep/manifest.json"), "{}").expect("index metadata");
        let still_clean = read_provenance(root).await;
        assert_eq!(
            still_clean.dirty,
            Some(false),
            "the index directory is not dirt"
        );
        assert_eq!(still_clean.commit, clean.commit);

        std::fs::write(root.join("file.txt"), "changed").expect("dirty the tree");
        let dirty = read_provenance(root).await;
        assert_eq!(dirty.dirty, Some(true));
        assert_eq!(dirty.commit, clean.commit);

        let relation =
            commit_relation(root, clean.commit.as_deref(), clean.commit.as_deref()).await;
        assert_eq!(relation, CommitRelation::Same);
        // A second commit: the first becomes its ancestor, and a donor at the
        // first commit lags the target by exactly one commit.
        let status = std::process::Command::new("git")
            .args(["commit", "--quiet", "--allow-empty", "-m", "second"])
            .current_dir(root)
            .status()
            .expect("git commit");
        assert!(status.success());
        let second = read_provenance(root).await;
        assert_eq!(
            commit_relation(root, clean.commit.as_deref(), second.commit.as_deref()).await,
            CommitRelation::Ancestor
        );
        assert_eq!(
            commit_relation(root, second.commit.as_deref(), clean.commit.as_deref()).await,
            CommitRelation::Ancestor
        );
        assert_eq!(
            commits_behind(
                root,
                clean.commit.as_deref().expect("first commit"),
                second.commit.as_deref().expect("second commit"),
            )
            .await,
            Some(1)
        );
    }

    #[tokio::test]
    async fn unknown_commits_classify_as_unrelated() {
        let directory = tempdir().expect("temporary directory");
        assert_eq!(
            commit_relation(directory.path(), None, Some("abc")).await,
            CommitRelation::Unrelated
        );
        assert_eq!(
            commit_relation(directory.path(), Some("abc"), None).await,
            CommitRelation::Unrelated
        );
        assert_eq!(commits_behind(directory.path(), "abc", "def").await, None);
    }
}
