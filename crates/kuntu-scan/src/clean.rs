use crate::local_scan::{scan_local_directory, LocalScanNode, ScanCoverage};
use crate::scanner::{IgnoredMode, ScanOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io;
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
  /// Ignored: candidate searches are local-only and never follow links.
  #[serde(default)]
  pub follow_symlinks: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CleanupCandidate {
  #[serde(serialize_with = "crate::wire_path::serialize")]
  pub path: PathBuf,
  pub size: u64,
  pub reason: String,
  pub preset: CleanupPreset,
  pub ignored: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemovalEntry {
  #[serde(serialize_with = "crate::wire_path::serialize")]
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

/// What a candidate search found, and what it had to leave alone.
#[derive(Clone, Debug)]
pub struct CandidateReport {
  pub candidates: Vec<CleanupCandidate>,
  /// Matches not offered because part of them was not scanned (a cloud or
  /// online-only item, another volume, an unreadable folder). Removing such a
  /// folder would remove what was never looked at.
  pub withheld: Vec<PathBuf>,
  pub coverage: ScanCoverage,
}

/// [`find_local_candidates`] without the coverage; a failed scan finds nothing.
pub fn find_candidates(options: CandidateOptions) -> Vec<CleanupCandidate> {
  find_local_candidates(options)
    .map(|report| report.candidates)
    .unwrap_or_default()
}

/// Searches one protected local scan of the roots: cloud storage, online-only
/// items, links and other volumes are never entered. Sizes are allocated bytes,
/// with a file hard-linked into several places counted once.
pub fn find_local_candidates(options: CandidateOptions) -> io::Result<CandidateReport> {
  let presets = options.effective_presets();
  let report = scan_local_directory(ScanOptions {
    directories: options.roots.clone(),
    // Hidden entries are filtered while searching, so a candidate is still
    // measured and judged complete as a whole.
    ignore_hidden: false,
    full_path: false,
    respect_gitignore: presets.contains(&CleanupPreset::Gitignored),
    ignored_mode: IgnoredMode::Summarize,
    follow_symlinks: false,
  })?;
  let mut search = Search {
    ignore_hidden: options.ignore_hidden,
    seen: HashSet::new(),
    candidates: Vec::new(),
    withheld: Vec::new(),
  };
  for preset in presets {
    for root in &report.nodes {
      search.visit(root, preset);
    }
  }
  let Search {
    mut candidates,
    mut withheld,
    ..
  } = search;
  candidates.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
  withheld.sort();
  withheld.dedup();
  Ok(CandidateReport {
    candidates,
    withheld,
    coverage: report.coverage,
  })
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

struct Search {
  ignore_hidden: bool,
  seen: HashSet<PathBuf>,
  candidates: Vec<CleanupCandidate>,
  withheld: Vec<PathBuf>,
}

impl Search {
  fn visit(&mut self, node: &LocalScanNode, preset: CleanupPreset) {
    if node.scan_state == "skipped" {
      return;
    }
    // `.gitignore` never hides the root the caller named.
    let hidden_applies = node.depth > 0 || preset != CleanupPreset::Gitignored;
    if self.ignore_hidden && hidden_applies && is_hidden(&node.path) {
      return;
    }
    let name = node.path.file_name().and_then(|name| name.to_str());
    let reason = match preset {
      CleanupPreset::Node => {
        (node.is_directory && name == Some("node_modules")).then_some("Node dependency directory")
      }
      CleanupPreset::Rust => {
        (node.is_directory && name == Some("target")).then_some("Cargo build output directory")
      }
      CleanupPreset::Gitignored => node.ignored.then_some("Path matched by .gitignore"),
    };
    let Some(reason) = reason else {
      for child in &node.children {
        self.visit(child, preset);
      }
      return;
    };
    if node.scan_state != "complete" {
      self.withheld.push(node.path.clone());
    } else if self.seen.insert(node.path.clone()) {
      self.candidates.push(CleanupCandidate {
        path: node.path.clone(),
        size: node.size,
        reason: reason.to_string(),
        preset,
        ignored: preset == CleanupPreset::Gitignored,
      });
    }
  }
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
  fn symlinked_projects_are_never_followed() {
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

    for follow_symlinks in [false, true] {
      assert!(find_candidates(CandidateOptions {
        roots: vec![root.clone()],
        presets: vec![CleanupPreset::Node],
        ignore_hidden: false,
        follow_symlinks,
      })
      .is_empty());
    }

    remove_dir_all(base).unwrap();
  }

  #[test]
  fn cloud_folders_are_never_candidates_and_withhold_what_contains_them() {
    let root = fixture("cloud-candidates");
    create_dir_all(root.join("OneDrive/app/node_modules")).unwrap();
    create_dir_all(root.join("mixed/node_modules/Dropbox")).unwrap();
    create_dir_all(root.join("plain/node_modules")).unwrap();
    for file in [
      "OneDrive/app/node_modules/a",
      "mixed/node_modules/Dropbox/b",
      "plain/node_modules/c",
    ] {
      write(root.join(file), "x\n").unwrap();
    }

    let report = find_local_candidates(CandidateOptions {
      roots: vec![root.clone()],
      presets: vec![CleanupPreset::Node],
      ignore_hidden: false,
      follow_symlinks: false,
    })
    .unwrap();

    assert_eq!(report.candidates.len(), 1, "{:?}", report.candidates);
    assert!(report.candidates[0].path.ends_with("plain/node_modules"));
    assert_eq!(report.withheld.len(), 1);
    assert!(report.withheld[0].ends_with("mixed/node_modules"));
    assert_eq!(
      report.coverage.skipped_by_reason.get("cloud-root"),
      Some(&2)
    );
    assert!(find_candidates(CandidateOptions {
      roots: vec![root.join("OneDrive")],
      presets: vec![],
      ignore_hidden: false,
      follow_symlinks: false,
    })
    .is_empty());

    remove_dir_all(root).unwrap();
  }

  #[test]
  fn hidden_entries_are_searched_only_when_asked_but_measured_either_way() {
    let root = fixture("hidden-candidates");
    create_dir_all(root.join(".tool/node_modules")).unwrap();
    create_dir_all(root.join("app/node_modules/.bin")).unwrap();
    write(root.join(".tool/node_modules/a"), "x\n").unwrap();
    write(root.join("app/node_modules/.bin/b"), "x\n").unwrap();
    let search = |ignore_hidden| {
      find_candidates(CandidateOptions {
        roots: vec![root.clone()],
        presets: vec![CleanupPreset::Node],
        ignore_hidden,
        follow_symlinks: false,
      })
    };

    assert_eq!(search(false).len(), 2);
    let visible = search(true);
    assert_eq!(visible.len(), 1);
    assert!(visible[0].path.ends_with("app/node_modules"));

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
