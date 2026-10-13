//! Cancellable snapshot walk for M2 hosts; the protected crawler stays unchanged.
use crate::{IndexError, Result};
use kuntu_core::{EntryKind, PathPolicy};
use kuntu_crawler::{CrawlOptions, CrawlStats, CrawledEntry};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

pub(crate) fn crawl_cancellable(
  options: &CrawlOptions,
  stop: &AtomicBool,
) -> Result<(Vec<CrawledEntry>, CrawlStats)> {
  let policy = PathPolicy::new(options.config.clone());
  let mut entries = Vec::new();
  let mut stats = CrawlStats {
    visited: 0,
    yielded: 0,
    skipped: 0,
    errors: Vec::new(),
  };
  for root in options.config.roots.iter().filter(|r| r.enabled) {
    visit(
      &root.path,
      0,
      options,
      &policy,
      stop,
      &mut entries,
      &mut stats,
    )?;
  }
  Ok((entries, stats))
}
fn check(stop: &AtomicBool) -> Result<()> {
  if stop.load(Ordering::Acquire) {
    Err(IndexError::Cancelled)
  } else {
    Ok(())
  }
}
fn visit(
  path: &Path,
  depth: usize,
  options: &CrawlOptions,
  policy: &PathPolicy,
  stop: &AtomicBool,
  entries: &mut Vec<CrawledEntry>,
  stats: &mut CrawlStats,
) -> Result<()> {
  check(stop)?;
  stats.visited += 1;
  let decision = policy.evaluate(path, Some(&options.query));
  if !decision.allowed {
    stats.skipped += 1;
    return Ok(());
  }
  let metadata = match std::fs::symlink_metadata(path) {
    Ok(m) => m,
    Err(e) => {
      stats.errors.push(format!("{}: {e}", path.display()));
      return Ok(());
    }
  };
  check(stop)?;
  let kind = if metadata.is_dir() {
    EntryKind::Directory
  } else if metadata.is_file() {
    EntryKind::File
  } else {
    EntryKind::Other
  };
  let mtime = metadata
    .modified()
    .ok()
    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
    .and_then(|d| i64::try_from(d.as_secs()).ok());
  entries.push(CrawledEntry {
    path: decision.path.clone(),
    kind,
    size: metadata.is_file().then_some(metadata.len()),
    mtime,
    decision,
  });
  stats.yielded += 1;
  if metadata.is_dir() && options.max_depth.is_none_or(|max| depth < max) {
    let children = match std::fs::read_dir(path) {
      Ok(r) => r,
      Err(e) => {
        stats.errors.push(format!("{}: {e}", path.display()));
        return Ok(());
      }
    };
    check(stop)?;
    for child in children {
      check(stop)?;
      match child {
        Ok(child) => visit(
          &child.path(),
          depth + 1,
          options,
          policy,
          stop,
          entries,
          stats,
        )?,
        Err(e) => stats.errors.push(format!("{}: {e}", path.display())),
      }
    }
  }
  Ok(())
}
