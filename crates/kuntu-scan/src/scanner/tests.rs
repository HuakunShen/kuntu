use super::*;
use std::fs::{create_dir_all, hard_link, remove_dir_all, write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct TestDir {
  path: PathBuf,
}

impl TestDir {
  fn new(name: &str) -> Self {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("kuntu-scan-{name}-{}-{id}", std::process::id()));
    let _ = remove_dir_all(&path);
    create_dir_all(&path).unwrap();
    Self { path }
  }
}

impl Drop for TestDir {
  fn drop(&mut self) {
    let _ = remove_dir_all(&self.path);
  }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum HookPoint {
  AfterEnumeration,
  BeforeAdmission,
  BeforeCollapsedEntry,
  BeforeMetadata,
  BeforeInodeMutation,
}

struct BlockingHook {
  point: HookPoint,
  entered: SyncSender<()>,
  release: Mutex<Receiver<()>>,
  blocked: AtomicBool,
  metadata_reads: AtomicU64,
  inode_insertions: AtomicU64,
  deadline_reached: AtomicBool,
}

impl BlockingHook {
  fn new(point: HookPoint) -> (Arc<Self>, Receiver<()>, SyncSender<()>) {
    let (entered_tx, entered_rx) = sync_channel(0);
    let (release_tx, release_rx) = sync_channel(0);
    (
      Arc::new(Self {
        point,
        entered: entered_tx,
        release: Mutex::new(release_rx),
        blocked: AtomicBool::new(false),
        metadata_reads: AtomicU64::new(0),
        inode_insertions: AtomicU64::new(0),
        deadline_reached: AtomicBool::new(false),
      }),
      entered_rx,
      release_tx,
    )
  }

  fn block_once(&self, point: HookPoint) {
    if self.point == point && !self.blocked.swap(true, Ordering::SeqCst) {
      self.entered.send(()).unwrap();
      self.release.lock().unwrap().recv().unwrap();
    }
  }
}

impl TraversalHook for BlockingHook {
  fn after_enumeration(&self, _path: &Path) {
    self.block_once(HookPoint::AfterEnumeration);
  }

  fn before_admission(&self, _path: &Path) {
    self.block_once(HookPoint::BeforeAdmission);
  }

  fn before_collapsed_entry(&self, _path: &Path) {
    self.block_once(HookPoint::BeforeCollapsedEntry);
  }

  fn before_metadata(&self, _path: &Path) {
    self.block_once(HookPoint::BeforeMetadata);
  }

  fn after_metadata(&self, _path: &Path) {
    self.metadata_reads.fetch_add(1, Ordering::SeqCst);
  }

  fn before_inode_mutation(&self, _path: &Path) {
    self.block_once(HookPoint::BeforeInodeMutation);
  }

  fn after_inode_mutation(&self, _path: &Path) {
    self.inode_insertions.fetch_add(1, Ordering::SeqCst);
  }

  fn deadline_reached(&self, _deadline: Instant) -> bool {
    self.deadline_reached.load(Ordering::SeqCst)
  }
}

fn options(root: &TestDir) -> ScanOptions {
  ScanOptions {
    directories: vec![root.path.clone()],
    ignore_hidden: false,
    full_path: false,
    respect_gitignore: false,
    ignored_mode: IgnoredMode::Exclude,
  }
}

fn assert_monotonic(before: ScanProgress, after: ScanProgress) {
  assert!(before.visited <= after.visited);
  assert!(before.admitted <= after.admitted);
  assert!(before.completed <= after.completed);
  assert!(before.bytes <= after.bytes);
}

fn assert_causal(progress: ScanProgress) {
  assert!(
    progress.completed <= progress.visited,
    "completed exceeded visited: {progress:?}"
  );
  assert!(
    progress.visited <= progress.admitted,
    "visited exceeded admitted: {progress:?}"
  );
}

struct InterleavedProgressHook {
  advance: SyncSender<()>,
  advanced: Mutex<Receiver<()>>,
  paused: AtomicBool,
}

impl ProgressSnapshotHook for InterleavedProgressHook {
  fn after_completed_load(&self) {
    if !self.paused.swap(true, Ordering::SeqCst) {
      self.advance.send(()).unwrap();
      self.advanced.lock().unwrap().recv().unwrap();
    }
  }
}

fn normalized(mut roots: Vec<ScanNode>) -> serde_json::Value {
  fn normalize_node(node: &mut ScanNode) {
    for child in &mut node.children {
      normalize_node(child);
    }
    node
      .children
      .sort_by(|left, right| left.path.cmp(&right.path));
  }

  for root in &mut roots {
    normalize_node(root);
  }
  roots.sort_by(|left, right| left.path.cmp(&right.path));
  serde_json::to_value(roots).unwrap()
}

fn assert_matches_base(options: ScanOptions) {
  let expected = legacy_reference::scan_directory(options.clone());
  let ScanOutcome::Completed { roots, .. } =
    scan_directory_cooperative(options, ScanControl::default(), None)
  else {
    panic!("an uncancelled scan did not complete")
  };
  assert_eq!(normalized(expected), normalized(roots));
}

#[test]
fn legacy_scan_matches_completed_cooperative_scan() {
  let ordinary = TestDir::new("legacy-ordinary");
  create_dir_all(ordinary.path.join("nested/deeper")).unwrap();
  write(ordinary.path.join("alpha.txt"), b"alpha").unwrap();
  write(ordinary.path.join("nested/beta.txt"), b"beta").unwrap();
  write(ordinary.path.join("nested/deeper/gamma.txt"), b"gamma").unwrap();
  write(ordinary.path.join(".hidden"), b"hidden").unwrap();
  assert_matches_base(options(&ordinary));

  let mut hidden = options(&ordinary);
  hidden.ignore_hidden = true;
  assert_matches_base(hidden);

  let ignored = TestDir::new("legacy-gitignore");
  create_dir_all(ignored.path.join("ignored/nested")).unwrap();
  write(ignored.path.join(".gitignore"), "ignored/\n*.log\n").unwrap();
  write(ignored.path.join("kept.txt"), b"kept").unwrap();
  write(ignored.path.join("ignored/nested/payload"), b"payload").unwrap();
  write(ignored.path.join("debug.log"), b"log").unwrap();
  let mut exclude = options(&ignored);
  exclude.respect_gitignore = true;
  assert_matches_base(exclude.clone());

  let mut summarize = exclude;
  summarize.ignored_mode = IgnoredMode::Summarize;
  assert_matches_base(summarize);

  let links = TestDir::new("legacy-hardlinks");
  let original = links.path.join("original");
  let alias = links.path.join("alias");
  write(&original, b"one inode").unwrap();
  hard_link(&original, &alias).unwrap();
  let link_options = ScanOptions {
    directories: vec![original, alias],
    ignore_hidden: false,
    full_path: false,
    respect_gitignore: false,
    ignored_mode: IgnoredMode::Exclude,
  };
  let expected = legacy_reference::scan_directory(link_options.clone());
  assert_eq!(expected.len(), 1);
  let ScanOutcome::Completed { roots, .. } =
    scan_directory_cooperative(link_options, ScanControl::default(), None)
  else {
    panic!("an uncancelled hard-link scan did not complete")
  };
  assert_eq!(roots.len(), 1);
  assert_eq!(normalized(expected), normalized(roots));

  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;

    let unreadable = TestDir::new("legacy-unreadable");
    let denied = unreadable.path.join("denied");
    create_dir_all(&denied).unwrap();
    write(denied.join("payload"), b"payload").unwrap();
    let original_permissions = std::fs::metadata(&denied).unwrap().permissions();
    std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&denied).is_err() {
      assert_matches_base(options(&unreadable));
    }
    std::fs::set_permissions(&denied, original_permissions).unwrap();
  }
}

#[test]
fn legacy_wrapper_delegates_to_completed_cooperative_scan() {
  let root = TestDir::new("legacy-wrapper");
  create_dir_all(root.path.join("nested")).unwrap();
  write(root.path.join("nested/payload"), b"payload").unwrap();
  let scan_options = options(&root);

  let wrapped = scan_directory(scan_options.clone());
  let outcome = scan_directory_cooperative(scan_options, ScanControl::default(), None);

  let ScanOutcome::Completed { roots, .. } = outcome else {
    panic!("an uncancelled scan did not complete")
  };
  assert_eq!(normalized(wrapped), normalized(roots));
}

#[test]
fn progress_counters_are_monotonic_and_polled_off_workers() {
  let root = TestDir::new("progress");
  create_dir_all(root.path.join("nested")).unwrap();
  write(root.path.join("first"), b"first").unwrap();
  write(root.path.join("nested/second"), b"second").unwrap();
  let control = ScanControl::default();
  let initial = control.progress();
  assert_causal(initial);
  let (hook, entered, release) = BlockingHook::new(HookPoint::BeforeAdmission);
  let worker_control = control.clone();
  let handle = thread::spawn(move || {
    scan_directory_cooperative_with_hook(options(&root), worker_control, None, hook)
  });

  entered.recv().unwrap();
  let while_worker_is_blocked = control.progress();
  assert_causal(while_worker_is_blocked);
  assert_monotonic(initial, while_worker_is_blocked);
  release.send(()).unwrap();
  let outcome = handle.join().unwrap();
  let ScanOutcome::Completed { progress, .. } = outcome else {
    panic!("an uncancelled scan did not complete")
  };
  assert_causal(progress);
  assert_monotonic(while_worker_is_blocked, progress);
  assert!(progress.visited >= 4);
  assert_eq!(progress.visited, progress.admitted);
  assert_eq!(progress.completed, progress.admitted);
  assert!(progress.bytes > 0);
}

#[test]
fn progress_snapshot_preserves_causality_across_interleaved_loads() {
  let control = ScanControl::default();
  let initial = control.progress();
  let (advance_tx, advance_rx) = sync_channel(0);
  let (advanced_tx, advanced_rx) = sync_channel(0);
  let hook = InterleavedProgressHook {
    advance: advance_tx,
    advanced: Mutex::new(advanced_rx),
    paused: AtomicBool::new(false),
  };
  let worker_control = control.clone();
  let worker = thread::spawn(move || {
    advance_rx.recv().unwrap();
    ScanControl::increment(&worker_control.admitted);
    ScanControl::increment(&worker_control.visited);
    worker_control.add_bytes(7);
    ScanControl::increment(&worker_control.completed);
    advanced_tx.send(()).unwrap();
  });

  let interleaved = control.progress_with_hook(&hook);
  worker.join().unwrap();
  let final_progress = control.progress();

  assert_causal(initial);
  assert_causal(interleaved);
  assert_causal(final_progress);
  assert_monotonic(initial, interleaved);
  assert_monotonic(interleaved, final_progress);
  assert_eq!(
    interleaved,
    ScanProgress {
      visited: 1,
      admitted: 1,
      completed: 0,
      bytes: 7,
    }
  );
  assert_eq!(final_progress.completed, 1);
}

#[test]
fn cancel_propagates_to_root_without_partial_success() {
  let root = TestDir::new("cancel-root");
  write(root.path.join("child"), b"child").unwrap();
  let control = ScanControl::default();
  let (hook, entered, release) = BlockingHook::new(HookPoint::BeforeAdmission);
  let worker_control = control.clone();
  let handle = thread::spawn(move || {
    scan_directory_cooperative_with_hook(options(&root), worker_control, None, hook)
  });

  entered.recv().unwrap();
  control.cancel();
  release.send(()).unwrap();

  let ScanOutcome::Cancelled { progress } = handle.join().unwrap() else {
    panic!("cancellation was reported as partial success")
  };
  assert!(control.is_cancelled());
  assert_eq!(progress, control.progress());
}

#[test]
fn cancel_bounds_new_recursive_admissions_after_barrier() {
  let root = TestDir::new("cancel-admission");
  for index in 0..32 {
    write(root.path.join(format!("entry-{index}")), b"entry").unwrap();
  }
  let control = ScanControl::default();
  let (hook, entered, release) = BlockingHook::new(HookPoint::AfterEnumeration);
  let worker_control = control.clone();
  let handle = thread::spawn(move || {
    scan_directory_cooperative_with_hook(options(&root), worker_control, None, hook)
  });

  entered.recv().unwrap();
  let admitted_at_barrier = control.progress().admitted;
  control.cancel();
  release.send(()).unwrap();

  let ScanOutcome::Cancelled { progress } = handle.join().unwrap() else {
    panic!("cancellation was reported as success")
  };
  assert_eq!(progress.admitted, admitted_at_barrier);
}

#[test]
fn deadline_is_not_reported_as_explicit_cancel() {
  let root = TestDir::new("deadline");
  write(root.path.join("child"), b"child").unwrap();
  let control = ScanControl::default();

  let outcome = scan_directory_cooperative(options(&root), control.clone(), Some(Instant::now()));

  let ScanOutcome::Deadline { progress } = outcome else {
    panic!("an expired deadline was not distinguished from explicit cancellation")
  };
  assert!(!control.is_cancelled());
  assert_eq!(progress, control.progress());
}

#[test]
fn cancel_at_pre_metadata_barrier_prevents_metadata_and_inode_mutation() {
  let root = TestDir::new("cancel-before-metadata");
  let target = root.path.join("target");
  write(&target, b"target").unwrap();
  let scan_options = ScanOptions {
    directories: vec![target],
    ignore_hidden: false,
    full_path: false,
    respect_gitignore: false,
    ignored_mode: IgnoredMode::Exclude,
  };
  let control = ScanControl::default();
  let (hook, entered, release) = BlockingHook::new(HookPoint::BeforeMetadata);
  let worker_hook = hook.clone();
  let worker_control = control.clone();
  let handle = thread::spawn(move || {
    scan_directory_cooperative_with_hook(scan_options, worker_control, None, worker_hook)
  });

  entered.recv().unwrap();
  assert_eq!(hook.metadata_reads.load(Ordering::SeqCst), 0);
  assert_eq!(hook.inode_insertions.load(Ordering::SeqCst), 0);
  control.cancel();
  release.send(()).unwrap();

  assert!(matches!(
    handle.join().unwrap(),
    ScanOutcome::Cancelled { .. }
  ));
  assert_eq!(hook.metadata_reads.load(Ordering::SeqCst), 0);
  assert_eq!(hook.inode_insertions.load(Ordering::SeqCst), 0);
}

#[cfg(any(unix, windows))]
#[test]
fn deadline_at_locked_inode_barrier_prevents_inode_insertion() {
  let root = TestDir::new("deadline-before-inode");
  let target = root.path.join("target");
  write(&target, b"target").unwrap();
  let scan_options = ScanOptions {
    directories: vec![target],
    ignore_hidden: false,
    full_path: false,
    respect_gitignore: false,
    ignored_mode: IgnoredMode::Exclude,
  };
  let control = ScanControl::default();
  let (hook, entered, release) = BlockingHook::new(HookPoint::BeforeInodeMutation);
  let worker_hook = hook.clone();
  let worker_control = control.clone();
  // The test hook owns the clock decision, so this instant is not a timing
  // oracle: false reaches the barrier, then true expires it synchronously.
  let deadline = Instant::now();
  let handle = thread::spawn(move || {
    scan_directory_cooperative_with_hook(scan_options, worker_control, Some(deadline), worker_hook)
  });

  entered.recv().unwrap();
  assert_eq!(hook.metadata_reads.load(Ordering::SeqCst), 1);
  assert_eq!(hook.inode_insertions.load(Ordering::SeqCst), 0);
  hook.deadline_reached.store(true, Ordering::SeqCst);
  release.send(()).unwrap();

  assert!(matches!(
    handle.join().unwrap(),
    ScanOutcome::Deadline { .. }
  ));
  assert!(!control.is_cancelled());
  assert_eq!(hook.metadata_reads.load(Ordering::SeqCst), 1);
  assert_eq!(hook.inode_insertions.load(Ordering::SeqCst), 0);
}

#[test]
fn collapsed_summary_propagates_cancel() {
  let root = TestDir::new("collapsed-cancel");
  create_dir_all(root.path.join("ignored/nested")).unwrap();
  write(root.path.join(".gitignore"), "ignored/\n").unwrap();
  write(root.path.join("ignored/nested/payload"), b"payload").unwrap();
  let mut scan_options = options(&root);
  scan_options.respect_gitignore = true;
  scan_options.ignored_mode = IgnoredMode::Summarize;
  let control = ScanControl::default();
  let (hook, entered, release) = BlockingHook::new(HookPoint::BeforeCollapsedEntry);
  let worker_control = control.clone();
  let handle = thread::spawn(move || {
    scan_directory_cooperative_with_hook(scan_options, worker_control, None, hook)
  });

  entered.recv().unwrap();
  control.cancel();
  release.send(()).unwrap();

  assert!(matches!(
    handle.join().unwrap(),
    ScanOutcome::Cancelled { .. }
  ));
}

// Test-only reference copied from crates/kuntu-scan/src/scanner.rs at
// a5df3559671b09f2af75fdd28ed5a4a06ab7dc56. Keep this independent of the
// cooperative implementation so legacy-tree comparisons have a real oracle.
mod legacy_reference {
  use super::{IgnoredMode, ScanNode, ScanOptions};
  use ignore::gitignore::{Gitignore, GitignoreBuilder};
  use rayon::iter::ParallelBridge;
  use rayon::prelude::*;
  use std::collections::HashSet;
  use std::path::Path;
  use std::sync::{Arc, Mutex};

  type IgnoreStack = Vec<Arc<Gitignore>>;
  type SeenInodes = Arc<Mutex<HashSet<(u64, u64)>>>;

  pub(super) fn scan_directory(options: ScanOptions) -> Vec<ScanNode> {
    let seen_inodes = Arc::new(Mutex::new(HashSet::new()));

    options
      .directories
      .iter()
      .filter_map(|directory| scan_path(directory, 0, &[], false, false, &options, &seen_inodes))
      .collect()
  }

  fn scan_path(
    path: &Path,
    depth: u32,
    ignore_stack: &[Arc<Gitignore>],
    ignored: bool,
    collapsed: bool,
    options: &ScanOptions,
    seen_inodes: &SeenInodes,
  ) -> Option<ScanNode> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let own_size = unique_allocated_size(path, &metadata, seen_inodes)?;
    let is_dir = metadata.is_dir();

    if !is_dir {
      return Some(ScanNode {
        name: display_name(path, options.full_path),
        path: path.to_path_buf(),
        size: own_size,
        children: vec![],
        depth,
        ignored,
        collapsed,
      });
    }

    let current_stack = if options.respect_gitignore && !collapsed {
      append_gitignore(path, ignore_stack)
    } else {
      ignore_stack.to_vec()
    };

    if collapsed {
      return Some(ScanNode {
        name: display_name(path, options.full_path),
        path: path.to_path_buf(),
        size: own_size + summarize_dir_children(path, seen_inodes),
        children: vec![],
        depth,
        ignored,
        collapsed,
      });
    }

    let children = match std::fs::read_dir(path) {
      Ok(entries) => entries
        .par_bridge()
        .filter_map(|entry| {
          let entry = entry.ok()?;
          let entry_path = entry.path();
          let file_type = entry.file_type().ok()?;
          let is_entry_dir = file_type.is_dir();

          if options.ignore_hidden && is_hidden(&entry_path) {
            return None;
          }

          let is_ignored =
            options.respect_gitignore && is_gitignored(&entry_path, is_entry_dir, &current_stack);
          if is_ignored && options.ignored_mode == IgnoredMode::Exclude {
            return None;
          }

          let collapse_child =
            is_ignored && options.ignored_mode == IgnoredMode::Summarize && is_entry_dir;

          scan_path(
            &entry_path,
            if is_entry_dir { depth + 1 } else { depth },
            &current_stack,
            is_ignored,
            collapse_child,
            options,
            seen_inodes,
          )
        })
        .collect::<Vec<_>>(),
      Err(_) => vec![],
    };

    let children_size = children.iter().map(|child| child.size).sum::<u64>();

    Some(ScanNode {
      name: display_name(path, options.full_path),
      path: path.to_path_buf(),
      size: own_size + children_size,
      children,
      depth,
      ignored,
      collapsed,
    })
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
    let ok =
      unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, info.as_mut_ptr()) };
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
}
