use super::engine::{
  EvictionCandidate, EvictionPlan, EvictionProgressState, EvictionStatus, PlanBudget, ScanOptions,
  build_eviction_plan, build_eviction_plan_bounded, execute_eviction_plan,
  execute_eviction_plan_with_state, execute_eviction_plan_with_state_and_observer,
};
use super::guard::{absolute_path, ensure_within_scope, fingerprint_for_path};
use super::model::{Bytes, DownloadState, Fingerprint, ItemInfo, ItemKind};
use super::model::{CloudError, ErrorKind};
use super::platform::CloudBackend;
use super::policy::{SkipReason, eviction_skip_reason};
use std::collections::HashMap;
use std::fs::{create_dir_all, write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn observer_fixture() -> (FakeBackend, EvictionPlan) {
  let root = PathBuf::from("/tmp/kuntu-observer-fixture");
  let mut items = HashMap::new();
  let mut candidates = Vec::new();
  for index in 0..2 {
    let path = root.join(format!("item-{index}"));
    let mut info = item(
      ItemKind::RegularFile,
      DownloadState::LocalCurrent,
      Some(4096),
      true,
      Some(false),
      Some(true),
      Some(false),
      Some(false),
      Some(false),
      1,
    );
    info.fingerprint.inode += index;
    candidates.push(EvictionCandidate {
      path: path.clone(),
      logical_bytes: Bytes(4096),
      allocated_bytes: Bytes(4096),
      fingerprint: info.fingerprint.clone(),
    });
    items.insert(path, info);
  }
  (
    FakeBackend::new(items),
    EvictionPlan {
      root,
      candidates,
      cloud_only: Vec::new(),
      skipped: Vec::new(),
      coverage_complete: true,
      visited_entries: 2,
      notes_total: 0,
    },
  )
}

#[test]
fn observer_start_failure_admits_no_backend_effect() {
  let (backend, plan) = observer_fixture();
  let state = EvictionProgressState::new();
  let result = execute_eviction_plan_with_state_and_observer(
    &backend,
    &plan,
    1,
    &state,
    &|_| {
      Err(CloudError::new(
        ErrorKind::Io,
        "durable pre-action write failed",
      ))
    },
    &|_, _| Ok(()),
  );
  assert!(result.is_err());
  assert!(backend.evicted.lock().unwrap().is_empty());
  assert_eq!(state.snapshot().processed, 0);
}

#[test]
fn cancellation_after_prepare_is_not_cleared_when_worker_starts() {
  let (backend, plan) = observer_fixture();
  let state = EvictionProgressState::new();
  state.prepare(plan.candidates.len());
  state.cancel();

  let outcome = execute_eviction_plan_with_state(&backend, &plan, 1, &state).unwrap();

  assert!(outcome.cancelled);
  assert!(backend.evicted.lock().unwrap().is_empty());
  assert!(state.snapshot().cancelled);
}

#[test]
fn observer_result_failure_stops_before_next_candidate() {
  let (backend, plan) = observer_fixture();
  let state = EvictionProgressState::new();
  let started = AtomicUsize::new(0);
  let result = execute_eviction_plan_with_state_and_observer(
    &backend,
    &plan,
    1,
    &state,
    &|_| {
      started.fetch_add(1, Ordering::SeqCst);
      Ok(())
    },
    &|_, _| {
      Err(CloudError::new(
        ErrorKind::Io,
        "durable post-action write failed",
      ))
    },
  );
  assert!(result.is_err());
  assert_eq!(started.load(Ordering::SeqCst), 1);
  assert_eq!(backend.evicted.lock().unwrap().len(), 1);
  assert_eq!(state.snapshot().processed, 0);
}

#[cfg(unix)]
use std::os::unix::fs::symlink;

#[allow(clippy::too_many_arguments)]
fn item(
  kind: ItemKind,
  download_state: DownloadState,
  allocated_bytes: Option<u64>,
  is_icloud: bool,
  is_package: Option<bool>,
  uploaded: Option<bool>,
  uploading: Option<bool>,
  downloading: Option<bool>,
  conflicts: Option<bool>,
  links: u64,
) -> ItemInfo {
  ItemInfo {
    fingerprint: Fingerprint {
      device: 1,
      inode: 2,
      kind,
      logical_bytes: 4096,
      links,
      modified_sec: 1,
      modified_nsec: 0,
      changed_sec: 1,
      changed_nsec: 0,
    },
    allocated_bytes: allocated_bytes.map(Bytes),
    foundation_allocated_bytes: None,
    foundation_total_allocated_bytes: None,
    is_icloud,
    is_package,
    download_state,
    is_uploaded: uploaded,
    is_uploading: uploading,
    is_downloading: downloading,
    has_conflicts: conflicts,
    upload_error: None,
    download_error: None,
  }
}

#[test]
fn unknown_download_state_is_not_eligible() {
  let info = item(
    ItemKind::RegularFile,
    DownloadState::Unknown,
    Some(4096),
    true,
    Some(false),
    Some(true),
    Some(false),
    Some(false),
    Some(false),
    1,
  );

  assert_eq!(
    eviction_skip_reason(&info),
    Some(SkipReason::UnknownDownloadState)
  );
}

#[test]
fn uploaded_current_regular_file_with_local_bytes_is_eligible() {
  let info = item(
    ItemKind::RegularFile,
    DownloadState::LocalCurrent,
    Some(4096),
    true,
    Some(false),
    Some(true),
    Some(false),
    Some(false),
    Some(false),
    1,
  );

  assert_eq!(eviction_skip_reason(&info), None);
}

#[test]
fn unsafe_transfer_states_and_zero_allocation_are_skipped() {
  let cases = [
    (
      item(
        ItemKind::RegularFile,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(false),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
      SkipReason::NotUploaded,
    ),
    (
      item(
        ItemKind::RegularFile,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(true),
        Some(false),
        Some(false),
        1,
      ),
      SkipReason::Uploading,
    ),
    (
      item(
        ItemKind::RegularFile,
        DownloadState::LocalCurrent,
        Some(0),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
      SkipReason::ZeroLocalAllocation,
    ),
  ];

  for (info, expected) in cases {
    assert_eq!(eviction_skip_reason(&info), Some(expected));
  }
}

#[test]
fn structural_items_and_hard_links_are_not_candidates() {
  let cases = [
    (
      ItemKind::SymbolicLink,
      Some(false),
      SkipReason::SymbolicLink,
    ),
    (ItemKind::Directory, Some(false), SkipReason::NotRegularFile),
    (ItemKind::RegularFile, Some(true), SkipReason::Package),
  ];

  for (kind, is_package, expected) in cases {
    let info = item(
      kind,
      DownloadState::LocalCurrent,
      Some(4096),
      true,
      is_package,
      Some(true),
      Some(false),
      Some(false),
      Some(false),
      1,
    );
    assert_eq!(eviction_skip_reason(&info), Some(expected));
  }

  let hard_linked = item(
    ItemKind::RegularFile,
    DownloadState::LocalCurrent,
    Some(4096),
    true,
    Some(false),
    Some(true),
    Some(false),
    Some(false),
    Some(false),
    2,
  );
  assert_eq!(
    eviction_skip_reason(&hard_linked),
    Some(SkipReason::HardLinked)
  );
}

#[test]
fn path_guards_require_absolute_paths_without_parent_components() {
  assert!(absolute_path(PathBuf::from("relative/file").as_path()).is_err());
  assert!(absolute_path(PathBuf::from("/tmp/../private").as_path()).is_err());

  let root = std::env::temp_dir().join("space-lens-cloud-scope");
  create_dir_all(root.join("nested")).unwrap();
  let child = root.join("nested/file");
  write(&child, b"test").unwrap();

  assert!(ensure_within_scope(&root, &child).is_ok());
  assert!(ensure_within_scope(&root, &root.join("../outside")).is_err());

  std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn fingerprint_uses_lstat_and_rejects_symbolic_links() {
  let root = std::env::temp_dir().join("space-lens-cloud-symlink");
  create_dir_all(&root).unwrap();
  let target = root.join("target");
  let link = root.join("link");
  write(&target, b"target").unwrap();
  symlink(&target, &link).unwrap();

  assert_eq!(
    fingerprint_for_path(&link).unwrap().kind,
    ItemKind::SymbolicLink
  );

  std::fs::remove_dir_all(root).unwrap();
}

struct FakeBackend {
  items: Mutex<HashMap<PathBuf, ItemInfo>>,
  evicted: Mutex<Vec<PathBuf>>,
  replace_on_evict: bool,
}

impl FakeBackend {
  fn new(items: HashMap<PathBuf, ItemInfo>) -> Self {
    Self::new_with_replacement(items, false)
  }

  fn new_with_replacement(items: HashMap<PathBuf, ItemInfo>, replace_on_evict: bool) -> Self {
    Self {
      items: Mutex::new(items),
      evicted: Mutex::new(Vec::new()),
      replace_on_evict,
    }
  }
}

impl CloudBackend for FakeBackend {
  fn inspect(&self, path: &std::path::Path) -> super::model::Result<ItemInfo> {
    self
      .items
      .lock()
      .unwrap()
      .get(path)
      .cloned()
      .ok_or_else(|| {
        CloudError::new(
          ErrorKind::Io,
          format!("missing fixture: {}", path.display()),
        )
      })
  }

  fn evict_local_copy(
    &self,
    path: &std::path::Path,
    expected: &Fingerprint,
  ) -> super::model::Result<()> {
    let mut items = self.items.lock().unwrap();
    let item = items
      .get_mut(path)
      .ok_or_else(|| CloudError::new(ErrorKind::Io, "missing fixture"))?;
    if item.fingerprint != *expected {
      return Err(CloudError::new(
        ErrorKind::ChangedSincePlan,
        "fixture changed",
      ));
    }
    item.download_state = DownloadState::CloudOnly;
    item.allocated_bytes = Some(Bytes(0));
    if self.replace_on_evict {
      item.fingerprint.inode += 1;
    }
    self.evicted.lock().unwrap().push(path.to_path_buf());
    Ok(())
  }

  fn request_download(
    &self,
    _path: &std::path::Path,
    _expected: &Fingerprint,
  ) -> super::model::Result<()> {
    Ok(())
  }
}

struct SlowBackend {
  inner: FakeBackend,
  active: AtomicUsize,
  maximum_active: Mutex<usize>,
  delay: Duration,
}

impl SlowBackend {
  fn new(items: HashMap<PathBuf, ItemInfo>, delay: Duration) -> Self {
    Self {
      inner: FakeBackend::new(items),
      active: AtomicUsize::new(0),
      maximum_active: Mutex::new(0),
      delay,
    }
  }

  fn maximum_active(&self) -> usize {
    *self.maximum_active.lock().unwrap()
  }
}

impl CloudBackend for SlowBackend {
  fn inspect(&self, path: &std::path::Path) -> super::model::Result<ItemInfo> {
    self.inner.inspect(path)
  }

  fn evict_local_copy(
    &self,
    path: &std::path::Path,
    expected: &Fingerprint,
  ) -> super::model::Result<()> {
    let active = self.active.fetch_add(1, Ordering::Relaxed) + 1;
    {
      let mut maximum = self.maximum_active.lock().unwrap();
      *maximum = (*maximum).max(active);
    }
    std::thread::sleep(self.delay);
    let result = self.inner.evict_local_copy(path, expected);
    self.active.fetch_sub(1, Ordering::Relaxed);
    result
  }

  fn request_download(
    &self,
    path: &std::path::Path,
    expected: &Fingerprint,
  ) -> super::model::Result<()> {
    self.inner.request_download(path, expected)
  }
}

fn fixture_info(path: &std::path::Path, mut info: ItemInfo) -> ItemInfo {
  info.fingerprint = fingerprint_for_path(path).unwrap();
  info
}

fn bounded_fixture(label: &str, count: usize) -> (PathBuf, FakeBackend) {
  let nonce = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  let root = std::env::temp_dir().join(format!("kuntu-{label}-{}-{nonce}", std::process::id()));
  create_dir_all(&root).unwrap();
  let mut items = HashMap::new();
  items.insert(
    root.clone(),
    fixture_info(
      &root,
      item(
        ItemKind::Directory,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );
  for index in 0..count {
    let path = root.join(format!("entry-{index:05}.txt"));
    write(&path, b"eligible local iCloud item").unwrap();
    items.insert(
      path.clone(),
      fixture_info(
        &path,
        item(
          ItemKind::RegularFile,
          DownloadState::LocalCurrent,
          Some(4096),
          true,
          Some(false),
          Some(true),
          Some(false),
          Some(false),
          Some(false),
          1,
        ),
      ),
    );
  }
  (root, FakeBackend::new(items))
}

#[test]
fn bounded_plan_refuses_to_claim_complete_after_candidate_limit() {
  let (root, backend) = bounded_fixture("candidate-cap", 8);
  let plan = build_eviction_plan_bounded(
    &backend,
    &root,
    ScanOptions::default(),
    PlanBudget {
      max_candidates: 2,
      max_staged_bytes: 1024 * 1024,
    },
  )
  .unwrap();

  assert_eq!(plan.candidates.len(), 2);
  assert!(!plan.coverage_complete);
  std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_plan_stops_staging_a_wide_directory_at_its_memory_budget() {
  let (root, backend) = bounded_fixture("wide-cap", 128);
  let first_child = root.join("entry-00000.txt");
  let max_staged_bytes = (root.as_os_str().len() + 256) * 2 + first_child.as_os_str().len() + 256;
  let plan = build_eviction_plan_bounded(
    &backend,
    &root,
    ScanOptions::default(),
    PlanBudget {
      max_candidates: 10_000,
      max_staged_bytes,
    },
  )
  .unwrap();

  assert_eq!(
    plan.visited_entries, 1,
    "children were not all materialized"
  );
  assert!(plan.candidates.is_empty());
  assert!(!plan.coverage_complete);
  std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn plan_only_contains_eligible_files_and_reports_cloud_only_items() {
  let root = std::env::temp_dir().join("space-lens-cloud-plan");
  create_dir_all(&root).unwrap();
  let local = root.join("local.bin");
  let cloud_only = root.join("cloud-only.bin");
  write(&local, b"local").unwrap();
  write(&cloud_only, b"cloud").unwrap();

  let mut items = HashMap::new();
  items.insert(
    root.clone(),
    fixture_info(
      &root,
      item(
        ItemKind::Directory,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );
  items.insert(
    local.clone(),
    fixture_info(
      &local,
      item(
        ItemKind::RegularFile,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );
  items.insert(
    cloud_only.clone(),
    fixture_info(
      &cloud_only,
      item(
        ItemKind::RegularFile,
        DownloadState::CloudOnly,
        Some(0),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );

  let backend = FakeBackend::new(items);
  let plan = build_eviction_plan(&backend, &root, ScanOptions::default()).unwrap();

  assert!(plan.coverage_complete);
  assert_eq!(plan.candidates.len(), 1);
  assert_eq!(plan.candidates[0].path, local);
  assert_eq!(plan.cloud_only.len(), 1);

  std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn eviction_uses_bounded_concurrency() {
  let root = std::env::temp_dir().join("space-lens-cloud-concurrency");
  create_dir_all(&root).unwrap();
  let mut items = HashMap::new();
  items.insert(
    root.clone(),
    fixture_info(
      &root,
      item(
        ItemKind::Directory,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );
  for index in 0..24 {
    let path = root.join(format!("local-{index}.bin"));
    write(&path, b"local").unwrap();
    items.insert(
      path.clone(),
      fixture_info(
        &path,
        item(
          ItemKind::RegularFile,
          DownloadState::LocalCurrent,
          Some(4096),
          true,
          Some(false),
          Some(true),
          Some(false),
          Some(false),
          Some(false),
          1,
        ),
      ),
    );
  }

  let backend = SlowBackend::new(items, Duration::from_millis(15));
  let plan = build_eviction_plan(&backend, &root, ScanOptions::default()).unwrap();
  let state = EvictionProgressState::new();
  let outcome = execute_eviction_plan_with_state(&backend, &plan, 4, &state).unwrap();

  assert_eq!(outcome.results.len(), 24);
  assert!(backend.maximum_active() > 1);
  assert!(backend.maximum_active() <= 4);
  assert_eq!(state.snapshot().processed, 24);
  assert_eq!(state.snapshot().freed_bytes, 24 * 4096);

  std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn incomplete_plan_executes_discovered_candidates_only() {
  let root = std::env::temp_dir().join("space-lens-cloud-incomplete");
  create_dir_all(&root).unwrap();
  let first = root.join("first.bin");
  let second = root.join("second.bin");
  write(&first, b"first").unwrap();
  write(&second, b"second").unwrap();
  let mut items = HashMap::new();
  items.insert(
    root.clone(),
    fixture_info(
      &root,
      item(
        ItemKind::Directory,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );
  for path in [&first, &second] {
    items.insert(
      path.clone(),
      fixture_info(
        path,
        item(
          ItemKind::RegularFile,
          DownloadState::LocalCurrent,
          Some(4096),
          true,
          Some(false),
          Some(true),
          Some(false),
          Some(false),
          Some(false),
          1,
        ),
      ),
    );
  }
  let backend = FakeBackend::new(items);
  let options = ScanOptions {
    max_entries: 2,
    ..ScanOptions::default()
  };

  let plan = build_eviction_plan(&backend, &root, options).unwrap();
  assert!(!plan.coverage_complete);
  assert_eq!(plan.candidates.len(), 1);
  let outcome = execute_eviction_plan(&backend, &plan).unwrap();
  assert_eq!(outcome.results.len(), 1);
  assert_eq!(backend.evicted.lock().unwrap().as_slice(), &[first]);
  assert_eq!(
    backend
      .items
      .lock()
      .unwrap()
      .get(&second)
      .unwrap()
      .download_state,
    DownloadState::LocalCurrent
  );

  std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_fingerprint_is_rejected_before_eviction() {
  let root = std::env::temp_dir().join("space-lens-cloud-changed");
  create_dir_all(&root).unwrap();
  let local = root.join("local.bin");
  write(&local, b"local").unwrap();
  let mut info = item(
    ItemKind::RegularFile,
    DownloadState::LocalCurrent,
    Some(4096),
    true,
    Some(false),
    Some(true),
    Some(false),
    Some(false),
    Some(false),
    1,
  );
  info = fixture_info(&local, info);
  let original = info.fingerprint.clone();
  let mut items = HashMap::new();
  items.insert(
    root.clone(),
    fixture_info(
      &root,
      item(
        ItemKind::Directory,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );
  items.insert(local.clone(), info);
  let backend = FakeBackend::new(items);
  let mut plan = build_eviction_plan(&backend, &root, ScanOptions::default()).unwrap();
  plan.candidates[0].fingerprint = Fingerprint {
    logical_bytes: original.logical_bytes + 1,
    ..original
  };

  let outcome = execute_eviction_plan(&backend, &plan).unwrap();
  assert_eq!(outcome.results[0].status, EvictionStatus::Skipped);
  assert!(backend.evicted.lock().unwrap().is_empty());

  std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn eviction_failure_does_not_delete_files() {
  let root = std::env::temp_dir().join("space-lens-cloud-failure");
  create_dir_all(&root).unwrap();
  let local = root.join("local.bin");
  write(&local, b"local").unwrap();
  let mut items = HashMap::new();
  items.insert(
    root.clone(),
    fixture_info(
      &root,
      item(
        ItemKind::Directory,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );
  items.insert(
    local.clone(),
    fixture_info(
      &local,
      item(
        ItemKind::RegularFile,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );
  let backend = FakeBackend::new(items);
  let plan = build_eviction_plan(&backend, &root, ScanOptions::default()).unwrap();
  let outcome = execute_eviction_plan(&backend, &plan).unwrap();

  assert_eq!(outcome.results[0].status, EvictionStatus::Evicted);
  assert!(local.exists());

  std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replaced_identity_after_eviction_is_not_marked_confirmed() {
  let root = std::env::temp_dir().join("space-lens-cloud-replaced");
  create_dir_all(&root).unwrap();
  let local = root.join("local.bin");
  write(&local, b"local").unwrap();
  let mut items = HashMap::new();
  items.insert(
    root.clone(),
    fixture_info(
      &root,
      item(
        ItemKind::Directory,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );
  items.insert(
    local.clone(),
    fixture_info(
      &local,
      item(
        ItemKind::RegularFile,
        DownloadState::LocalCurrent,
        Some(4096),
        true,
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        Some(false),
        1,
      ),
    ),
  );
  let backend = FakeBackend::new_with_replacement(items, true);
  let plan = build_eviction_plan(&backend, &root, ScanOptions::default()).unwrap();
  let outcome = execute_eviction_plan(&backend, &plan).unwrap();

  assert_eq!(
    outcome.results[0].status,
    EvictionStatus::RequestedUnverified
  );

  std::fs::remove_dir_all(root).unwrap();
}
