use super::*;
use std::fs::{create_dir_all, remove_dir_all, write};
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
}

struct BlockingHook {
  point: HookPoint,
  entered: SyncSender<()>,
  release: Mutex<Receiver<()>>,
  blocked: AtomicBool,
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

#[test]
fn legacy_scan_matches_completed_cooperative_scan() {
  let root = TestDir::new("legacy");
  create_dir_all(root.path.join("nested")).unwrap();
  write(root.path.join("alpha.txt"), b"alpha").unwrap();
  write(root.path.join("nested/beta.txt"), b"beta").unwrap();
  let options = options(&root);

  let legacy = scan_directory(options.clone());
  let outcome = scan_directory_cooperative(options, ScanControl::default(), None);

  let ScanOutcome::Completed { roots, progress } = outcome else {
    panic!("an uncancelled scan did not complete")
  };
  assert_eq!(
    serde_json::to_value(legacy).unwrap(),
    serde_json::to_value(roots).unwrap()
  );
  assert_eq!(progress.visited, progress.admitted);
  assert_eq!(progress.completed, progress.admitted);
}

#[test]
fn progress_counters_are_monotonic_and_polled_off_workers() {
  let root = TestDir::new("progress");
  create_dir_all(root.path.join("nested")).unwrap();
  write(root.path.join("first"), b"first").unwrap();
  write(root.path.join("nested/second"), b"second").unwrap();
  let control = ScanControl::default();
  let initial = control.progress();
  let (hook, entered, release) = BlockingHook::new(HookPoint::BeforeAdmission);
  let worker_control = control.clone();
  let handle = thread::spawn(move || {
    scan_directory_cooperative_with_hook(options(&root), worker_control, None, hook)
  });

  entered.recv().unwrap();
  let while_worker_is_blocked = control.progress();
  assert_monotonic(initial, while_worker_is_blocked);
  release.send(()).unwrap();
  let outcome = handle.join().unwrap();
  let ScanOutcome::Completed { progress, .. } = outcome else {
    panic!("an uncancelled scan did not complete")
  };
  assert_monotonic(while_worker_is_blocked, progress);
  assert!(progress.visited >= 4);
  assert_eq!(progress.visited, progress.admitted);
  assert_eq!(progress.completed, progress.admitted);
  assert!(progress.bytes > 0);
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
