//! Git repository discovery with uncommitted-change detection.
//!
//! Walks the given roots looking for git repositories whose worktree has
//! uncommitted changes (`git status --porcelain` output is non-empty). The
//! `git` binary must be available on PATH; the crate deliberately carries no
//! git library dependency. Dirty repositories are reported and their subtree
//! is not descended into further; clean repositories are descended into so
//! nested repositories are still discovered.

use crate::scanner::{effective_metadata, mark_dir_visited, new_seen_inodes, SeenInodes};
use rayon::iter::ParallelBridge;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DirtyGitRepoOptions {
  pub roots: Vec<PathBuf>,
  /// Skip hidden entries while walking (a repository's own `.git` directory
  /// is never descended into regardless of this flag).
  #[serde(default)]
  pub ignore_hidden: bool,
  /// Descend into symlinked directories. Off by default.
  #[serde(default)]
  pub follow_symlinks: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DirtyGitRepo {
  pub path: PathBuf,
  /// Number of status entries reported by `git status --porcelain`.
  pub dirty_entries: u32,
}

pub fn find_dirty_git_repos(options: DirtyGitRepoOptions) -> Vec<DirtyGitRepo> {
  let repos = Mutex::new(Vec::new());
  let seen_dirs = new_seen_inodes();

  for root in &options.roots {
    visit(root, &options, &seen_dirs, &repos);
  }

  let mut found = repos.into_inner().unwrap_or_default();
  found.sort_by(|a, b| a.path.cmp(&b.path));
  found
}

fn visit(
  path: &Path,
  options: &DirtyGitRepoOptions,
  seen_dirs: &SeenInodes,
  repos: &Mutex<Vec<DirtyGitRepo>>,
) {
  if options.ignore_hidden && is_hidden(path) {
    return;
  }

  let metadata = match std::fs::symlink_metadata(path) {
    Ok(metadata) => metadata,
    Err(_) => return,
  };
  let metadata = match effective_metadata(path, metadata, options.follow_symlinks) {
    Some(metadata) => metadata,
    None => return,
  };
  if !metadata.is_dir() {
    return;
  }
  if options.follow_symlinks && !mark_dir_visited(path, &metadata, seen_dirs) {
    return;
  }

  if is_git_repo(path) {
    if let Some(dirty_entries) = uncommitted_entries(path) {
      if dirty_entries > 0 {
        if let Ok(mut repos) = repos.lock() {
          repos.push(DirtyGitRepo {
            path: path.to_path_buf(),
            dirty_entries,
          });
        }
        return;
      }
    }
    // Clean (or `git status` failed): keep walking so nested repositories
    // are still discovered.
  }

  let entries = match std::fs::read_dir(path) {
    Ok(entries) => entries,
    Err(_) => return,
  };

  entries.par_bridge().for_each(|entry| {
    let Ok(entry) = entry else { return };
    let entry_path = entry.path();
    if entry_path.file_name().is_some_and(|name| name == ".git") {
      return;
    }
    visit(&entry_path, options, seen_dirs, repos);
  });
}

/// A path is a git repository when it contains `.git` — a directory for
/// regular clones, a file for worktrees and submodules.
fn is_git_repo(path: &Path) -> bool {
  path.join(".git").exists()
}

fn uncommitted_entries(repo: &Path) -> Option<u32> {
  let output = std::process::Command::new("git")
    .current_dir(repo)
    .args(["status", "--porcelain"])
    .output()
    .ok()?;
  if !output.status.success() {
    return None;
  }
  let stdout = std::str::from_utf8(&output.stdout).ok()?;
  Some(
    stdout
      .lines()
      .filter(|line| !line.trim().is_empty())
      .count() as u32,
  )
}

fn is_hidden(path: &Path) -> bool {
  path
    .file_name()
    .and_then(|name| name.to_str())
    .is_some_and(|name| name.starts_with('.'))
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::fs;
  use std::process::Command;
  use std::time::{SystemTime, UNIX_EPOCH};

  fn git_available() -> bool {
    Command::new("git")
      .arg("--version")
      .output()
      .map(|output| output.status.success())
      .unwrap_or(false)
  }

  fn run_git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
      .args([
        "-c",
        "user.name=kuntu-test",
        "-c",
        "user.email=test@kuntu.local",
      ])
      .args(args)
      .current_dir(repo)
      .status()
      .expect("git should be invokable");
    assert!(status.success(), "git {args:?} failed");
  }

  /// root/clean (committed) and root/dirty (uncommitted file)
  fn setup_fixture(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    let root = std::env::temp_dir().join(format!("kuntu-dirty-git-{name}-{nanos}"));
    fs::create_dir_all(root.join("clean")).unwrap();
    fs::create_dir_all(root.join("dirty")).unwrap();

    for repo in ["clean", "dirty"] {
      run_git(&root.join(repo), &["init", "-q"]);
      fs::write(root.join(repo).join("tracked.txt"), "init\n").unwrap();
      run_git(&root.join(repo), &["add", "."]);
      run_git(&root.join(repo), &["commit", "-q", "-m", "init"]);
    }
    fs::write(root.join("dirty/uncommitted.txt"), "wip\n").unwrap();
    root
  }

  #[test]
  fn finds_dirty_repos_and_skips_clean_ones() {
    if !git_available() {
      return;
    }
    let root = setup_fixture("basic");

    let repos = find_dirty_git_repos(DirtyGitRepoOptions {
      roots: vec![root.clone()],
      ignore_hidden: false,
      follow_symlinks: false,
    });

    assert_eq!(repos.len(), 1);
    assert!(repos[0].path.ends_with("dirty"));
    assert!(repos[0].dirty_entries >= 1);

    fs::remove_dir_all(root).unwrap();
  }

  #[cfg(unix)]
  #[test]
  fn symlinked_repos_are_skipped_unless_follow_symlinks_is_set() {
    if !git_available() {
      return;
    }
    use std::os::unix::fs::symlink;

    // The linked repository lives OUTSIDE the scan root, like a vendored
    // reference checkout.
    let nanos = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    let base = std::env::temp_dir().join(format!("kuntu-dirty-git-symlink-ext-{nanos}"));
    let store = base.join("store");
    let root = base.join("root");
    fs::create_dir_all(store.join("repo")).unwrap();
    fs::create_dir_all(root.join("links")).unwrap();
    run_git(&store.join("repo"), &["init", "-q"]);
    fs::write(store.join("repo/uncommitted.txt"), "wip\n").unwrap();
    symlink(store.join("repo"), root.join("links/repo-link")).unwrap();

    let skipped = find_dirty_git_repos(DirtyGitRepoOptions {
      roots: vec![root.clone()],
      ignore_hidden: false,
      follow_symlinks: false,
    });
    assert!(skipped.is_empty());

    let followed = find_dirty_git_repos(DirtyGitRepoOptions {
      roots: vec![root.clone()],
      ignore_hidden: false,
      follow_symlinks: true,
    });
    assert_eq!(followed.len(), 1);
    assert!(followed[0].path.starts_with(root.join("links")));

    fs::remove_dir_all(base).unwrap();
  }
}
