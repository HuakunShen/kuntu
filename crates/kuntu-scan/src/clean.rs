use crate::scanner::{
  effective_metadata, mark_dir_visited, measure_path_inner, new_seen_inodes, scan_directory,
  IgnoredMode, ScanNode, ScanOptions, SeenInodes,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanupPreset {
  Node,
  Rust,
  Gitignored,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateOptions {
  pub roots: Vec<PathBuf>,
  pub presets: Vec<CleanupPreset>,
  pub ignore_hidden: bool,
  /// Descend into symlinked directories when searching for candidates.
  /// Off by default so scans do not wander into symlinked external projects.
  #[serde(default)]
  pub follow_symlinks: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CleanupCandidate {
  pub path: PathBuf,
  pub size: u64,
  pub reason: String,
  pub preset: CleanupPreset,
  pub ignored: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemovalEntry {
  pub path: PathBuf,
  pub size: u64,
  pub reason: String,
  pub preset: CleanupPreset,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RemovalPlan {
  pub entries: Vec<RemovalEntry>,
  pub total_size: u64,
  pub errors: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RemovalOutcome {
  pub removed: Vec<RemovalEntry>,
  pub bytes_removed: u64,
  pub errors: Vec<String>,
}

impl CandidateOptions {
  fn effective_presets(&self) -> Vec<CleanupPreset> {
    if self.presets.is_empty() {
      return vec![
        CleanupPreset::Node,
        CleanupPreset::Rust,
        CleanupPreset::Gitignored,
      ];
    }

    self.presets.clone()
  }
}

struct CandidateSpec {
  dir_name: &'static str,
  preset: CleanupPreset,
  reason: &'static str,
}

pub fn find_candidates(options: CandidateOptions) -> Vec<CleanupCandidate> {
  let mut candidates = Vec::new();
  let mut seen = HashSet::new();

  for preset in options.effective_presets() {
    match preset {
      CleanupPreset::Node => find_named_dir_candidates(
        &options.roots,
        &CandidateSpec {
          dir_name: "node_modules",
          preset: CleanupPreset::Node,
          reason: "Node dependency directory",
        },
        &options,
        &mut seen,
        &mut candidates,
      ),
      CleanupPreset::Rust => find_named_dir_candidates(
        &options.roots,
        &CandidateSpec {
          dir_name: "target",
          preset: CleanupPreset::Rust,
          reason: "Cargo build output directory",
        },
        &options,
        &mut seen,
        &mut candidates,
      ),
      CleanupPreset::Gitignored => {
        find_gitignored_candidates(&options, &mut seen, &mut candidates);
      }
    }
  }

  candidates.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
  candidates
}

pub fn build_removal_plan(candidates: Vec<CleanupCandidate>) -> RemovalPlan {
  let entries = candidates
    .into_iter()
    .map(|candidate| RemovalEntry {
      path: candidate.path,
      size: candidate.size,
      reason: candidate.reason,
      preset: candidate.preset,
    })
    .collect::<Vec<_>>();
  let total_size = entries.iter().map(|entry| entry.size).sum();

  RemovalPlan {
    entries,
    total_size,
    errors: Vec::new(),
  }
}

pub fn execute_removal_plan(plan: &RemovalPlan) -> RemovalOutcome {
  let mut outcome = RemovalOutcome::default();

  for entry in &plan.entries {
    match remove_path(&entry.path) {
      Ok(()) => {
        outcome.bytes_removed = outcome.bytes_removed.saturating_add(entry.size);
        outcome.removed.push(entry.clone());
      }
      Err(error) => outcome
        .errors
        .push(format!("{}: {}", entry.path.display(), error)),
    }
  }

  outcome
}

fn find_named_dir_candidates(
  roots: &[PathBuf],
  spec: &CandidateSpec,
  options: &CandidateOptions,
  seen: &mut HashSet<PathBuf>,
  candidates: &mut Vec<CleanupCandidate>,
) {
  let seen_dirs = new_seen_inodes();
  for root in roots {
    visit_named_dir_candidate(root, spec, options, seen, &seen_dirs, candidates);
  }
}

fn visit_named_dir_candidate(
  path: &Path,
  spec: &CandidateSpec,
  options: &CandidateOptions,
  seen: &mut HashSet<PathBuf>,
  seen_dirs: &SeenInodes,
  candidates: &mut Vec<CleanupCandidate>,
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

  if path.file_name().and_then(|name| name.to_str()) == Some(spec.dir_name) {
    push_candidate(
      path,
      spec.preset,
      spec.reason,
      false,
      options,
      seen,
      candidates,
    );
    return;
  }

  let entries = match std::fs::read_dir(path) {
    Ok(entries) => entries,
    Err(_) => return,
  };

  for entry in entries.flatten() {
    visit_named_dir_candidate(&entry.path(), spec, options, seen, seen_dirs, candidates);
  }
}

fn find_gitignored_candidates(
  options: &CandidateOptions,
  seen: &mut HashSet<PathBuf>,
  candidates: &mut Vec<CleanupCandidate>,
) {
  let trees = scan_directory(ScanOptions {
    directories: options.roots.clone(),
    ignore_hidden: options.ignore_hidden,
    full_path: true,
    respect_gitignore: true,
    ignored_mode: IgnoredMode::Summarize,
    follow_symlinks: options.follow_symlinks,
  });

  for tree in trees {
    collect_ignored_nodes(&tree, options, seen, candidates);
  }
}

fn collect_ignored_nodes(
  node: &ScanNode,
  options: &CandidateOptions,
  seen: &mut HashSet<PathBuf>,
  candidates: &mut Vec<CleanupCandidate>,
) {
  if node.ignored {
    push_candidate(
      &node.path,
      CleanupPreset::Gitignored,
      "Path matched by .gitignore",
      true,
      options,
      seen,
      candidates,
    );
    return;
  }

  for child in &node.children {
    collect_ignored_nodes(child, options, seen, candidates);
  }
}

fn push_candidate(
  path: &Path,
  preset: CleanupPreset,
  reason: &str,
  ignored: bool,
  options: &CandidateOptions,
  seen: &mut HashSet<PathBuf>,
  candidates: &mut Vec<CleanupCandidate>,
) {
  let path = path.to_path_buf();
  if !seen.insert(path.clone()) {
    return;
  }

  let size = measure_path_inner(path.as_path(), options.follow_symlinks);
  candidates.push(CleanupCandidate {
    size,
    path,
    reason: reason.to_string(),
    preset,
    ignored,
  });
}

fn remove_path(path: &Path) -> std::io::Result<()> {
  let metadata = std::fs::symlink_metadata(path)?;
  if metadata.is_dir() {
    std::fs::remove_dir_all(path)
  } else {
    std::fs::remove_file(path)
  }
}

pub fn delete_path(path: &Path) -> std::io::Result<()> {
  remove_path(path)
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
  use std::fs::{create_dir_all, remove_dir_all, write};
  use std::time::{SystemTime, UNIX_EPOCH};

  #[test]
  fn finds_cleanup_candidates_from_presets() {
    let root = fixture("candidates");
    create_dir_all(root.join("app/node_modules/pkg")).unwrap();
    create_dir_all(root.join("app/target/debug")).unwrap();
    create_dir_all(root.join("app/ignored")).unwrap();
    write(root.join("app/.gitignore"), "ignored/\n*.log\n").unwrap();
    write(
      root.join("app/node_modules/pkg/index.js"),
      "module.exports = 1\n",
    )
    .unwrap();
    write(root.join("app/target/debug/app"), "binary\n").unwrap();
    write(root.join("app/ignored/cache.bin"), "cache\n").unwrap();
    write(root.join("app/debug.log"), "log\n").unwrap();

    let candidates = find_candidates(CandidateOptions {
      roots: vec![root.join("app")],
      presets: vec![],
      ignore_hidden: false,
      follow_symlinks: false,
    });

    assert!(candidates
      .iter()
      .any(|candidate| candidate.path.ends_with("node_modules")));
    assert!(candidates
      .iter()
      .any(|candidate| candidate.path.ends_with("target")));
    assert!(candidates
      .iter()
      .any(|candidate| candidate.path.ends_with("ignored")));
    assert!(candidates
      .iter()
      .any(|candidate| candidate.path.ends_with("debug.log")));

    remove_dir_all(root).unwrap();
  }

  #[test]
  fn removal_plan_is_dry_run_until_executed() {
    let root = fixture("removal");
    create_dir_all(root.join("node_modules/pkg")).unwrap();
    write(
      root.join("node_modules/pkg/index.js"),
      "module.exports = 1\n",
    )
    .unwrap();

    let candidates = find_candidates(CandidateOptions {
      roots: vec![root.clone()],
      presets: vec![CleanupPreset::Node],
      ignore_hidden: false,
      follow_symlinks: false,
    });
    let plan = build_removal_plan(candidates);

    assert_eq!(plan.entries.len(), 1);
    assert!(root.join("node_modules").exists());

    let outcome = execute_removal_plan(&plan);

    assert_eq!(outcome.errors, Vec::<String>::new());
    assert_eq!(outcome.removed.len(), 1);
    assert!(!root.join("node_modules").exists());

    remove_dir_all(root).unwrap();
  }

  #[test]
  fn deletes_an_arbitrary_file_or_directory_path() {
    let root = fixture("arbitrary-delete");
    create_dir_all(root.join("folder/nested")).unwrap();
    write(root.join("folder/nested/file.txt"), "delete me\n").unwrap();

    delete_path(&root.join("folder")).unwrap();

    assert!(!root.join("folder").exists());
    remove_dir_all(root).unwrap();
  }

  #[cfg(unix)]
  #[test]
  fn symlinked_projects_are_skipped_unless_follow_symlinks_is_set() {
    use std::os::unix::fs::symlink;

    // The linked project lives OUTSIDE the scan root, like a vendored
    // reference checkout.
    let base = fixture("symlinks");
    let store = base.join("store");
    let root = base.join("root");
    create_dir_all(store.join("real/node_modules/leftover")).unwrap();
    create_dir_all(root.join("links")).unwrap();
    write(store.join("real/package.json"), "{}").unwrap();
    write(store.join("real/node_modules/leftover/index.js"), "x\n").unwrap();
    symlink(store.join("real"), root.join("links/project")).unwrap();

    let skipped = find_candidates(CandidateOptions {
      roots: vec![root.clone()],
      presets: vec![CleanupPreset::Node],
      ignore_hidden: false,
      follow_symlinks: false,
    });
    assert!(skipped.is_empty());

    let followed = find_candidates(CandidateOptions {
      roots: vec![root.clone()],
      presets: vec![CleanupPreset::Node],
      ignore_hidden: false,
      follow_symlinks: true,
    });
    assert_eq!(followed.len(), 1);
    assert!(followed[0].path.starts_with(root.join("links")));
    assert!(followed[0].path.ends_with("node_modules"));

    remove_dir_all(base).unwrap();
  }

  #[cfg(unix)]
  #[test]
  fn symlink_cycles_do_not_hang_the_candidate_walk() {
    use std::os::unix::fs::symlink;

    let root = fixture("cycle");
    create_dir_all(root.join("app/node_modules/pkg")).unwrap();
    symlink(root.join("app"), root.join("app/loop")).unwrap();

    let candidates = find_candidates(CandidateOptions {
      roots: vec![root.join("app")],
      presets: vec![CleanupPreset::Node],
      ignore_hidden: false,
      follow_symlinks: true,
    });

    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].path.ends_with("node_modules"));

    remove_dir_all(root).unwrap();
  }

  fn fixture(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    let root = std::env::temp_dir().join(format!("space-lens-{name}-{nanos}"));
    create_dir_all(&root).unwrap();
    root
  }
}
