use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rayon::iter::ParallelBridge;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IgnoredMode {
  Exclude,
  Summarize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanOptions {
  pub directories: Vec<PathBuf>,
  pub ignore_hidden: bool,
  pub full_path: bool,
  pub respect_gitignore: bool,
  pub ignored_mode: IgnoredMode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanNode {
  pub name: String,
  pub path: PathBuf,
  pub size: u64,
  pub children: Vec<ScanNode>,
  pub depth: u32,
  pub ignored: bool,
  pub collapsed: bool,
}

#[derive(Clone, Default)]
pub struct ScanControl {
  cancelled: Arc<AtomicBool>,
  visited: Arc<AtomicU64>,
  admitted: Arc<AtomicU64>,
  completed: Arc<AtomicU64>,
  bytes: Arc<AtomicU64>,
}

impl ScanControl {
  pub fn cancel(&self) {
    self.cancelled.store(true, Ordering::Release);
  }

  pub fn is_cancelled(&self) -> bool {
    self.cancelled.load(Ordering::Acquire)
  }

  pub fn progress(&self) -> ScanProgress {
    ScanProgress {
      visited: self.visited.load(Ordering::Relaxed),
      admitted: self.admitted.load(Ordering::Relaxed),
      completed: self.completed.load(Ordering::Relaxed),
      bytes: self.bytes.load(Ordering::Relaxed),
    }
  }

  fn increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
      Some(value.saturating_add(1))
    });
  }

  fn add_bytes(&self, bytes: u64) {
    let _ = self
      .bytes
      .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(bytes))
      });
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanProgress {
  pub visited: u64,
  pub admitted: u64,
  pub completed: u64,
  pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanError {
  pub message: String,
}

impl std::fmt::Display for ScanError {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter.write_str(&self.message)
  }
}

impl std::error::Error for ScanError {}

#[derive(Debug)]
pub enum ScanOutcome {
  Completed {
    roots: Vec<ScanNode>,
    progress: ScanProgress,
  },
  Cancelled {
    progress: ScanProgress,
  },
  Deadline {
    progress: ScanProgress,
  },
  Failed(ScanError),
}

type IgnoreStack = Vec<Arc<Gitignore>>;
type SeenInodes = Arc<Mutex<HashSet<(u64, u64)>>>;

enum WalkOutcome<T> {
  Completed(T),
  Skipped,
  Cancelled,
  Failed(ScanError),
}

trait TraversalHook: Send + Sync {
  fn after_enumeration(&self, _path: &Path) {}
  fn before_admission(&self, _path: &Path) {}
  fn before_collapsed_entry(&self, _path: &Path) {}
}

struct NoopTraversalHook;

impl TraversalHook for NoopTraversalHook {}

struct WalkContext<'a> {
  control: &'a ScanControl,
  deadline: Option<Instant>,
  hook: Arc<dyn TraversalHook>,
}

#[derive(Clone, Copy)]
struct NodeState {
  depth: u32,
  ignored: bool,
  collapsed: bool,
}

impl WalkContext<'_> {
  fn should_stop(&self) -> bool {
    self.control.is_cancelled()
      || self
        .deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
  }

  fn stopped_outcome(&self) -> ScanOutcome {
    let progress = self.control.progress();
    if self.control.is_cancelled() {
      ScanOutcome::Cancelled { progress }
    } else {
      ScanOutcome::Deadline { progress }
    }
  }

  fn admit(&self) {
    ScanControl::increment(&self.control.admitted);
  }

  fn visit(&self) {
    ScanControl::increment(&self.control.visited);
  }

  fn complete(&self) {
    ScanControl::increment(&self.control.completed);
  }
}

pub fn scan_directory(options: ScanOptions) -> Vec<ScanNode> {
  match scan_directory_cooperative(options, ScanControl::default(), None) {
    ScanOutcome::Completed { roots, .. } => roots,
    ScanOutcome::Cancelled { .. } | ScanOutcome::Deadline { .. } | ScanOutcome::Failed(_) => {
      Vec::new()
    }
  }
}

pub fn scan_directory_cooperative(
  options: ScanOptions,
  control: ScanControl,
  deadline: Option<Instant>,
) -> ScanOutcome {
  scan_directory_cooperative_inner(options, control, deadline, Arc::new(NoopTraversalHook))
}

#[cfg(test)]
fn scan_directory_cooperative_with_hook(
  options: ScanOptions,
  control: ScanControl,
  deadline: Option<Instant>,
  hook: Arc<dyn TraversalHook>,
) -> ScanOutcome {
  scan_directory_cooperative_inner(options, control, deadline, hook)
}

fn scan_directory_cooperative_inner(
  options: ScanOptions,
  control: ScanControl,
  deadline: Option<Instant>,
  hook: Arc<dyn TraversalHook>,
) -> ScanOutcome {
  let seen_inodes = Arc::new(Mutex::new(HashSet::new()));
  let context = WalkContext {
    control: &control,
    deadline,
    hook,
  };
  let mut roots = Vec::new();

  for directory in &options.directories {
    if context.should_stop() {
      return context.stopped_outcome();
    }
    context.admit();
    match scan_path(
      directory,
      &[],
      NodeState {
        depth: 0,
        ignored: false,
        collapsed: false,
      },
      &options,
      &seen_inodes,
      &context,
    ) {
      WalkOutcome::Completed(root) => roots.push(root),
      WalkOutcome::Skipped => {}
      WalkOutcome::Cancelled => return context.stopped_outcome(),
      WalkOutcome::Failed(error) => return ScanOutcome::Failed(error),
    }
  }

  ScanOutcome::Completed {
    roots,
    progress: control.progress(),
  }
}

pub fn measure_path(path: &Path) -> u64 {
  let seen_inodes = Arc::new(Mutex::new(HashSet::new()));
  summarize_path(path, &seen_inodes)
}

fn scan_path(
  path: &Path,
  ignore_stack: &[Arc<Gitignore>],
  state: NodeState,
  options: &ScanOptions,
  seen_inodes: &SeenInodes,
  context: &WalkContext<'_>,
) -> WalkOutcome<ScanNode> {
  if context.should_stop() {
    return WalkOutcome::Cancelled;
  }
  context.visit();

  let metadata = match std::fs::symlink_metadata(path) {
    Ok(metadata) => metadata,
    Err(_) => {
      context.complete();
      return WalkOutcome::Skipped;
    }
  };
  if context.should_stop() {
    return WalkOutcome::Cancelled;
  }
  let own_size = match unique_allocated_size_cooperative(path, &metadata, seen_inodes) {
    Ok(Some(size)) => size,
    Ok(None) => {
      context.complete();
      return WalkOutcome::Skipped;
    }
    Err(error) => return WalkOutcome::Failed(error),
  };
  context.control.add_bytes(own_size);
  let is_dir = metadata.is_dir();

  if !is_dir {
    context.complete();
    return WalkOutcome::Completed(ScanNode {
      name: display_name(path, options.full_path),
      path: path.to_path_buf(),
      size: own_size,
      children: vec![],
      depth: state.depth,
      ignored: state.ignored,
      collapsed: state.collapsed,
    });
  }

  let current_stack = if options.respect_gitignore && !state.collapsed {
    append_gitignore(path, ignore_stack)
  } else {
    ignore_stack.to_vec()
  };

  if state.collapsed {
    let children_size = match summarize_dir_children_cooperative(path, seen_inodes, context) {
      WalkOutcome::Completed(size) => size,
      WalkOutcome::Skipped => 0,
      WalkOutcome::Cancelled => return WalkOutcome::Cancelled,
      WalkOutcome::Failed(error) => return WalkOutcome::Failed(error),
    };
    context.complete();
    return WalkOutcome::Completed(ScanNode {
      name: display_name(path, options.full_path),
      path: path.to_path_buf(),
      size: own_size.saturating_add(children_size),
      children: vec![],
      depth: state.depth,
      ignored: state.ignored,
      collapsed: state.collapsed,
    });
  }

  let children = match std::fs::read_dir(path) {
    Ok(entries) => {
      let entries = entries.collect::<Vec<_>>();
      context.hook.after_enumeration(path);
      if context.should_stop() {
        return WalkOutcome::Cancelled;
      }
      let outcomes = entries
        .into_par_iter()
        .map(|entry| {
          if context.should_stop() {
            return WalkOutcome::Cancelled;
          }
          let entry = match entry {
            Ok(entry) => entry,
            Err(_) => return WalkOutcome::Skipped,
          };
          let entry_path = entry.path();
          let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => return WalkOutcome::Skipped,
          };
          let is_entry_dir = file_type.is_dir();

          if options.ignore_hidden && is_hidden(&entry_path) {
            return WalkOutcome::Skipped;
          }

          let is_ignored =
            options.respect_gitignore && is_gitignored(&entry_path, is_entry_dir, &current_stack);
          if is_ignored && options.ignored_mode == IgnoredMode::Exclude {
            return WalkOutcome::Skipped;
          }

          let collapse_child =
            is_ignored && options.ignored_mode == IgnoredMode::Summarize && is_entry_dir;

          context.hook.before_admission(&entry_path);
          if context.should_stop() {
            return WalkOutcome::Cancelled;
          }
          context.admit();
          scan_path(
            &entry_path,
            &current_stack,
            NodeState {
              depth: if is_entry_dir {
                state.depth + 1
              } else {
                state.depth
              },
              ignored: is_ignored,
              collapsed: collapse_child,
            },
            options,
            seen_inodes,
            context,
          )
        })
        .collect::<Vec<_>>();

      if outcomes
        .iter()
        .any(|outcome| matches!(outcome, WalkOutcome::Cancelled))
      {
        return WalkOutcome::Cancelled;
      }
      let mut children = Vec::new();
      for outcome in outcomes {
        match outcome {
          WalkOutcome::Completed(child) => children.push(child),
          WalkOutcome::Skipped => {}
          WalkOutcome::Cancelled => return WalkOutcome::Cancelled,
          WalkOutcome::Failed(error) => return WalkOutcome::Failed(error),
        }
      }
      children
    }
    Err(_) => vec![],
  };

  let children_size = children.iter().map(|child| child.size).sum::<u64>();

  context.complete();
  WalkOutcome::Completed(ScanNode {
    name: display_name(path, options.full_path),
    path: path.to_path_buf(),
    size: own_size.saturating_add(children_size),
    children,
    depth: state.depth,
    ignored: state.ignored,
    collapsed: state.collapsed,
  })
}

fn summarize_path(path: &Path, seen_inodes: &SeenInodes) -> u64 {
  let metadata = match std::fs::symlink_metadata(path) {
    Ok(metadata) => metadata,
    Err(_) => return 0,
  };
  let own_size = unique_allocated_size(path, &metadata, seen_inodes).unwrap_or(0);

  if metadata.is_dir() {
    own_size + summarize_dir_children(path, seen_inodes)
  } else {
    own_size
  }
}

fn summarize_path_cooperative(
  path: &Path,
  seen_inodes: &SeenInodes,
  context: &WalkContext<'_>,
) -> WalkOutcome<u64> {
  if context.should_stop() {
    return WalkOutcome::Cancelled;
  }
  context.visit();

  let metadata = match std::fs::symlink_metadata(path) {
    Ok(metadata) => metadata,
    Err(_) => {
      context.complete();
      return WalkOutcome::Skipped;
    }
  };
  if context.should_stop() {
    return WalkOutcome::Cancelled;
  }
  let own_size = match unique_allocated_size_cooperative(path, &metadata, seen_inodes) {
    Ok(Some(size)) => size,
    Ok(None) => {
      context.complete();
      return WalkOutcome::Skipped;
    }
    Err(error) => return WalkOutcome::Failed(error),
  };
  context.control.add_bytes(own_size);

  let size = if metadata.is_dir() {
    match summarize_dir_children_cooperative(path, seen_inodes, context) {
      WalkOutcome::Completed(children_size) => own_size.saturating_add(children_size),
      WalkOutcome::Skipped => own_size,
      WalkOutcome::Cancelled => return WalkOutcome::Cancelled,
      WalkOutcome::Failed(error) => return WalkOutcome::Failed(error),
    }
  } else {
    own_size
  };
  context.complete();
  WalkOutcome::Completed(size)
}

fn append_gitignore(path: &Path, ignore_stack: &[Arc<Gitignore>]) -> IgnoreStack {
  let gitignore_path = path.join(".gitignore");
  if !gitignore_path.exists() {
    return ignore_stack.to_vec();
  }

  let mut builder = GitignoreBuilder::new(path);
  let _ = builder.add(&gitignore_path);

  match builder.build() {
    Ok(gitignore) => {
      let mut next = ignore_stack.to_vec();
      next.push(Arc::new(gitignore));
      next
    }
    Err(_) => ignore_stack.to_vec(),
  }
}

fn is_gitignored(path: &Path, is_dir: bool, ignore_stack: &[Arc<Gitignore>]) -> bool {
  let mut ignored = false;

  for gitignore in ignore_stack {
    let matched = gitignore.matched(path, is_dir);
    if matched.is_ignore() {
      ignored = true;
    } else if matched.is_whitelist() {
      ignored = false;
    }
  }

  ignored
}

fn summarize_dir_children(path: &Path, seen_inodes: &SeenInodes) -> u64 {
  match std::fs::read_dir(path) {
    Ok(entries) => entries
      .par_bridge()
      .filter_map(|entry| {
        let entry = entry.ok()?;
        let entry_path = entry.path();
        let metadata = std::fs::symlink_metadata(&entry_path).ok()?;
        let own_size = unique_allocated_size(&entry_path, &metadata, seen_inodes)?;

        if metadata.is_dir() {
          Some(own_size + summarize_dir_children(&entry_path, seen_inodes))
        } else {
          Some(own_size)
        }
      })
      .sum(),
    Err(_) => 0,
  }
}

fn summarize_dir_children_cooperative(
  path: &Path,
  seen_inodes: &SeenInodes,
  context: &WalkContext<'_>,
) -> WalkOutcome<u64> {
  if context.should_stop() {
    return WalkOutcome::Cancelled;
  }
  let entries = match std::fs::read_dir(path) {
    Ok(entries) => entries.collect::<Vec<_>>(),
    Err(_) => return WalkOutcome::Completed(0),
  };
  context.hook.after_enumeration(path);
  if context.should_stop() {
    return WalkOutcome::Cancelled;
  }

  let outcomes = entries
    .into_par_iter()
    .map(|entry| {
      if context.should_stop() {
        return WalkOutcome::Cancelled;
      }
      let entry = match entry {
        Ok(entry) => entry,
        Err(_) => return WalkOutcome::Skipped,
      };
      let entry_path = entry.path();
      context.hook.before_collapsed_entry(&entry_path);
      context.hook.before_admission(&entry_path);
      if context.should_stop() {
        return WalkOutcome::Cancelled;
      }
      context.admit();
      summarize_path_cooperative(&entry_path, seen_inodes, context)
    })
    .collect::<Vec<_>>();

  if outcomes
    .iter()
    .any(|outcome| matches!(outcome, WalkOutcome::Cancelled))
  {
    return WalkOutcome::Cancelled;
  }

  let mut total = 0_u64;
  for outcome in outcomes {
    match outcome {
      WalkOutcome::Completed(size) => total = total.saturating_add(size),
      WalkOutcome::Skipped => {}
      WalkOutcome::Cancelled => return WalkOutcome::Cancelled,
      WalkOutcome::Failed(error) => return WalkOutcome::Failed(error),
    }
  }
  WalkOutcome::Completed(total)
}

fn display_name(path: &Path, full_path: bool) -> String {
  if full_path {
    return path.to_string_lossy().to_string();
  }

  path
    .file_name()
    .map(|name| name.to_string_lossy().to_string())
    .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn is_hidden(path: &Path) -> bool {
  path
    .file_name()
    .and_then(|name| name.to_str())
    .is_some_and(|name| name.starts_with('.'))
}

fn unique_allocated_size(
  path: &Path,
  metadata: &std::fs::Metadata,
  seen_inodes: &SeenInodes,
) -> Option<u64> {
  if let Some(key) = inode_key(path, metadata) {
    let mut seen = seen_inodes.lock().ok()?;
    if !seen.insert(key) {
      return None;
    }
  }

  Some(allocated_size(metadata))
}

fn unique_allocated_size_cooperative(
  path: &Path,
  metadata: &std::fs::Metadata,
  seen_inodes: &SeenInodes,
) -> Result<Option<u64>, ScanError> {
  if let Some(key) = inode_key(path, metadata) {
    let mut seen = seen_inodes.lock().map_err(|_| ScanError {
      message: "scan inode state was poisoned".to_string(),
    })?;
    if !seen.insert(key) {
      return Ok(None);
    }
  }

  Ok(Some(allocated_size(metadata)))
}

#[cfg(unix)]
fn inode_key(_path: &Path, metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
  use std::os::unix::fs::MetadataExt;

  Some((metadata.ino(), metadata.dev()))
}

#[cfg(windows)]
fn inode_key(path: &Path, metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
  use std::mem::MaybeUninit;
  use std::os::windows::io::AsRawHandle;
  use windows_sys::Win32::Foundation::HANDLE;
  use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
  };

  if metadata.is_dir() {
    return None;
  }

  let file = std::fs::File::open(path).ok()?;
  let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
  let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, info.as_mut_ptr()) };
  if ok == 0 {
    return None;
  }

  let info = unsafe { info.assume_init() };
  let file_index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
  Some((file_index, info.dwVolumeSerialNumber as u64))
}

#[cfg(not(any(unix, windows)))]
fn inode_key(_path: &Path, _metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
  None
}

#[cfg(unix)]
fn allocated_size(metadata: &std::fs::Metadata) -> u64 {
  use std::os::unix::fs::MetadataExt;

  metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn allocated_size(metadata: &std::fs::Metadata) -> u64 {
  metadata.len()
}

#[cfg(test)]
mod tests;
