use super::guard::{absolute_path, ensure_within_scope};
use super::model::{
  Bytes, CloudError, DownloadState, ErrorKind, Fingerprint, ItemInfo, ItemKind, Result,
};
use super::platform::CloudBackend;
use super::policy::{SkipReason, eviction_skip_reason};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const DEFAULT_MAX_ENTRIES: usize = 100_000;
const DEFAULT_MAX_DEPTH: usize = 128;
const DEFAULT_MAX_TIME: Duration = Duration::from_secs(120);
pub const DEFAULT_EVICTION_CONCURRENCY: usize = 8;
pub const MAX_EVICTION_CONCURRENCY: usize = 32;

#[derive(Clone, Debug)]
pub struct ScanOptions {
  pub max_entries: usize,
  pub max_depth: usize,
  pub max_time: Duration,
}

/// Additional memory/candidate bounds for callers that retain eviction plans
/// in a long-lived service. The legacy builder remains unbounded by this type
/// so standalone Kuntu callers keep their existing policy.
#[derive(Clone, Copy, Debug)]
pub struct PlanBudget {
  pub max_candidates: usize,
  pub max_staged_bytes: usize,
}

impl Default for ScanOptions {
  fn default() -> Self {
    Self {
      max_entries: DEFAULT_MAX_ENTRIES,
      max_depth: DEFAULT_MAX_DEPTH,
      max_time: DEFAULT_MAX_TIME,
    }
  }
}

#[derive(Clone, Debug, Serialize)]
pub struct ItemSummary {
  pub path: PathBuf,
  pub logical_bytes: Bytes,
  pub allocated_bytes: Option<Bytes>,
  pub state: DownloadState,
  pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkippedItem {
  pub path: PathBuf,
  pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvictionCandidate {
  pub path: PathBuf,
  pub logical_bytes: Bytes,
  pub allocated_bytes: Bytes,
  #[serde(skip)]
  pub(crate) fingerprint: Fingerprint,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvictionPlan {
  pub root: PathBuf,
  pub candidates: Vec<EvictionCandidate>,
  pub cloud_only: Vec<ItemSummary>,
  pub skipped: Vec<SkippedItem>,
  pub coverage_complete: bool,
  pub visited_entries: usize,
  pub notes_total: usize,
}

impl EvictionPlan {
  pub fn total_allocated_bytes(&self) -> u64 {
    self
      .candidates
      .iter()
      .map(|candidate| candidate.allocated_bytes.0)
      .sum()
  }

  pub fn total_logical_bytes(&self) -> u64 {
    self
      .candidates
      .iter()
      .map(|candidate| candidate.logical_bytes.0)
      .sum()
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvictionStatus {
  Evicted,
  RequestedUnverified,
  Skipped,
  Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvictionResult {
  pub path: PathBuf,
  pub status: EvictionStatus,
  pub allocated_bytes_before: Bytes,
  pub error: Option<CloudError>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct EvictionOutcome {
  pub results: Vec<EvictionResult>,
  pub cancelled: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct EvictionProgress {
  pub processed: usize,
  pub total: usize,
  pub evicted: usize,
  pub failed: usize,
  pub freed_bytes: u64,
  pub active: usize,
  pub running: bool,
  pub paused: bool,
  pub cancelled: bool,
}

pub struct EvictionProgressState {
  total: AtomicUsize,
  processed: AtomicUsize,
  evicted: AtomicUsize,
  failed: AtomicUsize,
  freed_bytes: AtomicU64,
  active: AtomicUsize,
  running: AtomicBool,
  paused: AtomicBool,
  cancelled: AtomicBool,
}

impl EvictionProgressState {
  pub fn new() -> Self {
    Self {
      total: AtomicUsize::new(0),
      processed: AtomicUsize::new(0),
      evicted: AtomicUsize::new(0),
      failed: AtomicUsize::new(0),
      freed_bytes: AtomicU64::new(0),
      active: AtomicUsize::new(0),
      running: AtomicBool::new(false),
      paused: AtomicBool::new(false),
      cancelled: AtomicBool::new(false),
    }
  }

  pub fn pause(&self) {
    self.paused.store(true, Ordering::Release);
  }

  pub fn resume(&self) {
    self.paused.store(false, Ordering::Release);
  }

  pub fn cancel(&self) {
    self.cancelled.store(true, Ordering::Release);
    self.paused.store(false, Ordering::Release);
  }

  pub fn snapshot(&self) -> EvictionProgress {
    EvictionProgress {
      processed: self.processed.load(Ordering::Acquire),
      total: self.total.load(Ordering::Acquire),
      evicted: self.evicted.load(Ordering::Acquire),
      failed: self.failed.load(Ordering::Acquire),
      freed_bytes: self.freed_bytes.load(Ordering::Acquire),
      active: self.active.load(Ordering::Acquire),
      running: self.running.load(Ordering::Acquire),
      paused: self.paused.load(Ordering::Acquire),
      cancelled: self.cancelled.load(Ordering::Acquire),
    }
  }

  fn begin(&self, total: usize) {
    // Xross publishes this state to the control map before the blocking worker
    // starts. In that path `prepare` has already opened the run, and a control
    // request may have arrived while the worker was queued. Do not clear such
    // a request when the executor reaches its own begin call.
    if self.running.load(Ordering::Acquire) {
      return;
    }
    self.total.store(total, Ordering::Release);
    self.processed.store(0, Ordering::Release);
    self.evicted.store(0, Ordering::Release);
    self.failed.store(0, Ordering::Release);
    self.freed_bytes.store(0, Ordering::Release);
    self.active.store(0, Ordering::Release);
    self.paused.store(false, Ordering::Release);
    self.cancelled.store(false, Ordering::Release);
    self.running.store(true, Ordering::Release);
  }

  /// Starts progress tracking before publishing the state to controls.
  /// The executor's later begin call is idempotent while this run is active.
  pub fn prepare(&self, total: usize) {
    self.begin(total);
  }

  fn finish(&self) {
    self.running.store(false, Ordering::Release);
    self.active.store(0, Ordering::Release);
  }

  fn wait_if_paused(&self) -> bool {
    while self.paused.load(Ordering::Acquire) && !self.cancelled.load(Ordering::Acquire) {
      std::thread::sleep(Duration::from_millis(10));
    }
    !self.cancelled.load(Ordering::Acquire)
  }

  fn record(&self, result: &EvictionResult) {
    self.processed.fetch_add(1, Ordering::AcqRel);
    match result.status {
      EvictionStatus::Evicted => {
        self.evicted.fetch_add(1, Ordering::AcqRel);
        self
          .freed_bytes
          .fetch_add(result.allocated_bytes_before.0, Ordering::AcqRel);
      }
      EvictionStatus::Failed => {
        self.failed.fetch_add(1, Ordering::AcqRel);
      }
      EvictionStatus::RequestedUnverified | EvictionStatus::Skipped => {}
    }
  }
}

impl Default for EvictionProgressState {
  fn default() -> Self {
    Self::new()
  }
}

pub fn inspect_item<B: CloudBackend + ?Sized>(backend: &B, path: &Path) -> Result<ItemInfo> {
  backend.inspect(path)
}

pub fn build_eviction_plan<B: CloudBackend + ?Sized>(
  backend: &B,
  selected_root: &Path,
  options: ScanOptions,
) -> Result<EvictionPlan> {
  build_eviction_plan_inner(backend, selected_root, options, None)
}

/// Builds an eviction plan while bounding candidate count and the estimated
/// bytes retained by pending paths and plan-owned path data. A hit marks the
/// result incomplete; callers must not present it as an executable plan.
pub fn build_eviction_plan_bounded<B: CloudBackend + ?Sized>(
  backend: &B,
  selected_root: &Path,
  options: ScanOptions,
  budget: PlanBudget,
) -> Result<EvictionPlan> {
  if budget.max_candidates == 0 || budget.max_staged_bytes == 0 {
    return Err(CloudError::new(
      ErrorKind::InvalidState,
      "plan budget limits must be positive",
    ));
  }
  build_eviction_plan_inner(backend, selected_root, options, Some(budget))
}

fn build_eviction_plan_inner<B: CloudBackend + ?Sized>(
  backend: &B,
  selected_root: &Path,
  options: ScanOptions,
  budget: Option<PlanBudget>,
) -> Result<EvictionPlan> {
  if options.max_entries == 0 || options.max_depth == 0 || options.max_time.is_zero() {
    return Err(CloudError::new(
      ErrorKind::InvalidState,
      "scan limits must be positive",
    ));
  }

  let root = absolute_path(selected_root)?;
  let root_info = backend.inspect(&root)?;
  if !root_info.is_icloud {
    return Err(CloudError::new(
      ErrorKind::NotICloud,
      format!(
        "selected root is not recognized as an iCloud item: {}",
        root.display()
      ),
    ));
  }

  let started = Instant::now();
  let mut plan = EvictionPlan {
    root: root.clone(),
    candidates: Vec::new(),
    cloud_only: Vec::new(),
    skipped: Vec::new(),
    coverage_complete: true,
    visited_entries: 0,
    notes_total: 0,
  };
  let mut staged_bytes = path_storage_bytes(&root).saturating_mul(2);
  if budget.is_some_and(|budget| staged_bytes > budget.max_staged_bytes) {
    return Err(CloudError::new(
      ErrorKind::InvalidState,
      "selected root exceeds the plan memory budget",
    ));
  }
  let mut pending = vec![(root, 0_usize)];
  let mut seen_identities = HashSet::new();

  'walk: while let Some((path, depth)) = pending.pop() {
    if budget.is_some() {
      staged_bytes = staged_bytes.saturating_sub(path_storage_bytes(&path));
    }
    if started.elapsed() >= options.max_time {
      plan.coverage_complete = false;
      add_note(
        &mut plan,
        &path,
        "scan time budget exceeded",
        &mut staged_bytes,
        budget,
      );
      break;
    }
    if plan.visited_entries >= options.max_entries || depth > options.max_depth {
      plan.coverage_complete = false;
      add_note(
        &mut plan,
        &path,
        "scan entry or depth budget exceeded",
        &mut staged_bytes,
        budget,
      );
      break;
    }

    ensure_within_scope(&plan.root, &path)?;
    plan.visited_entries += 1;

    let info = match backend.inspect(&path) {
      Ok(info) => info,
      Err(error) => {
        plan.coverage_complete = false;
        if !add_note(
          &mut plan,
          &path,
          error.to_string(),
          &mut staged_bytes,
          budget,
        ) {
          break 'walk;
        }
        continue;
      }
    };

    if info.kind() == ItemKind::Directory {
      match info.is_package {
        Some(true) => {
          if !stage_path(&mut staged_bytes, &path, budget) {
            plan.coverage_complete = false;
            break 'walk;
          }
          plan.skipped.push(SkippedItem {
            path,
            reason: SkipReason::Package.to_string(),
          });
          continue;
        }
        None => {
          if !stage_path(&mut staged_bytes, &path, budget) {
            plan.coverage_complete = false;
            break 'walk;
          }
          plan.skipped.push(SkippedItem {
            path,
            reason: SkipReason::UnknownPackageState.to_string(),
          });
          continue;
        }
        Some(false) => {}
      }

      let entries = match fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) => {
          plan.coverage_complete = false;
          if !add_note(
            &mut plan,
            &path,
            error.to_string(),
            &mut staged_bytes,
            budget,
          ) {
            break 'walk;
          }
          continue;
        }
      };

      let mut children = Vec::new();
      let mut staging_limit_hit = false;
      for entry in entries {
        match entry {
          Ok(entry) => {
            let child = entry.path();
            if !stage_path(&mut staged_bytes, &child, budget) {
              plan.coverage_complete = false;
              staging_limit_hit = true;
              break;
            }
            children.push(child);
          }
          Err(error) => {
            plan.coverage_complete = false;
            if !add_note(
              &mut plan,
              &path,
              error.to_string(),
              &mut staged_bytes,
              budget,
            ) {
              staging_limit_hit = true;
              break;
            }
          }
        }
      }
      children.sort();
      pending.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
      if staging_limit_hit {
        break 'walk;
      }
      continue;
    }

    if let Some(reason) = eviction_skip_reason(&info) {
      if info.download_state == DownloadState::CloudOnly {
        if !stage_path(&mut staged_bytes, &path, budget) {
          plan.coverage_complete = false;
          break 'walk;
        }
        plan.cloud_only.push(ItemSummary {
          path,
          logical_bytes: info.logical_bytes(),
          allocated_bytes: info.allocated_bytes,
          state: info.download_state,
          reason: Some(reason.to_string()),
        });
      } else {
        if !stage_path(&mut staged_bytes, &path, budget) {
          plan.coverage_complete = false;
          break 'walk;
        }
        plan.skipped.push(SkippedItem {
          path,
          reason: reason.to_string(),
        });
      }
      continue;
    }

    let identity = (info.fingerprint.device, info.fingerprint.inode);
    if identity != (0, 0) && !seen_identities.insert(identity) {
      if !stage_path(&mut staged_bytes, &path, budget) {
        plan.coverage_complete = false;
        break 'walk;
      }
      plan.skipped.push(SkippedItem {
        path,
        reason: SkipReason::HardLinked.to_string(),
      });
      continue;
    }

    let Some(allocated_bytes) = info.allocated_bytes else {
      if !stage_path(&mut staged_bytes, &path, budget) {
        plan.coverage_complete = false;
        break 'walk;
      }
      plan.skipped.push(SkippedItem {
        path,
        reason: SkipReason::UnknownLocalAllocation.to_string(),
      });
      continue;
    };

    if budget.is_some_and(|budget| plan.candidates.len() >= budget.max_candidates) {
      plan.coverage_complete = false;
      break 'walk;
    }
    if !stage_path(&mut staged_bytes, &path, budget) {
      plan.coverage_complete = false;
      break 'walk;
    }
    plan.candidates.push(EvictionCandidate {
      path,
      logical_bytes: info.logical_bytes(),
      allocated_bytes,
      fingerprint: info.fingerprint,
    });
  }

  Ok(plan)
}

pub fn execute_eviction_plan<B: CloudBackend + ?Sized>(
  backend: &B,
  plan: &EvictionPlan,
) -> Result<EvictionOutcome> {
  execute_eviction_plan_with_concurrency(backend, plan, DEFAULT_EVICTION_CONCURRENCY)
}

pub fn execute_eviction_plan_with_concurrency<B: CloudBackend + ?Sized>(
  backend: &B,
  plan: &EvictionPlan,
  max_concurrency: usize,
) -> Result<EvictionOutcome> {
  let state = EvictionProgressState::new();
  execute_eviction_plan_with_state(backend, plan, max_concurrency, &state)
}

pub fn execute_eviction_plan_with_state<B: CloudBackend + ?Sized>(
  backend: &B,
  plan: &EvictionPlan,
  max_concurrency: usize,
  state: &EvictionProgressState,
) -> Result<EvictionOutcome> {
  execute_eviction_plan_with_state_and_observer(
    backend,
    plan,
    max_concurrency,
    state,
    &|_| Ok(()),
    &|_, _| Ok(()),
  )
}

/// Calls `on_started` durably before a native action and `on_result` after it.
/// Observer failures cancel admission of further work and fail the entire run.
pub fn execute_eviction_plan_with_state_and_observer<B: CloudBackend + ?Sized>(
  backend: &B,
  plan: &EvictionPlan,
  max_concurrency: usize,
  state: &EvictionProgressState,
  on_started: &(dyn Fn(usize) -> Result<()> + Sync),
  on_result: &(dyn Fn(usize, &EvictionResult) -> Result<()> + Sync),
) -> Result<EvictionOutcome> {
  // A plan may be partial when traversal hits a time, depth, entry, or
  // metadata-read boundary. Every candidate is still revalidated immediately
  // before eviction, so executing a partial plan only touches items already
  // discovered by the plan and leaves the unvisited remainder untouched.
  state.begin(plan.candidates.len());
  let worker_count = max_concurrency
    .clamp(1, MAX_EVICTION_CONCURRENCY)
    .min(plan.candidates.len().max(1));
  let next_index = AtomicUsize::new(0);
  let results = Mutex::new(
    (0..plan.candidates.len())
      .map(|_| None)
      .collect::<Vec<Option<EvictionResult>>>(),
  );
  let observer_error = Mutex::new(None);

  std::thread::scope(|scope| {
    for _ in 0..worker_count {
      scope.spawn(|| {
        loop {
          if !state.wait_if_paused() {
            break;
          }
          let index = next_index.fetch_add(1, Ordering::Relaxed);
          if index >= plan.candidates.len() {
            break;
          }

          if let Err(error) = on_started(index) {
            state.cancel();
            let mut slot = observer_error
              .lock()
              .expect("observer error mutex poisoned");
            if slot.is_none() {
              *slot = Some(error);
            }
            break;
          }

          state.active.fetch_add(1, Ordering::AcqRel);
          let result = execute_eviction_candidate(backend, &plan.candidates[index]);
          state.active.fetch_sub(1, Ordering::AcqRel);
          if let Err(error) = on_result(index, &result) {
            state.cancel();
            let mut slot = observer_error
              .lock()
              .expect("observer error mutex poisoned");
            if slot.is_none() {
              *slot = Some(error);
            }
            break;
          }
          state.record(&result);
          results.lock().expect("eviction result mutex poisoned")[index] = Some(result);
        }
      });
    }
  });

  if let Some(error) = observer_error
    .into_inner()
    .map_err(|_| CloudError::new(ErrorKind::InvalidState, "observer error mutex poisoned"))?
  {
    state.finish();
    return Err(error);
  }

  let cancelled = state.cancelled.load(Ordering::Acquire);
  state.finish();

  let results = results
    .into_inner()
    .map_err(|_| CloudError::new(ErrorKind::InvalidState, "eviction result mutex poisoned"))?;
  Ok(EvictionOutcome {
    // A cancelled run reports only entries that reached a worker. The caller
    // can distinguish the remaining unprocessed candidates via `cancelled`.
    results: results.into_iter().flatten().collect(),
    cancelled,
  })
}

fn execute_eviction_candidate<B: CloudBackend + ?Sized>(
  backend: &B,
  candidate: &EvictionCandidate,
) -> EvictionResult {
  let allocated_bytes_before = candidate.allocated_bytes;
  match backend.inspect(&candidate.path) {
    Ok(current) if current.fingerprint == candidate.fingerprint => {
      if let Some(reason) = eviction_skip_reason(&current) {
        EvictionResult {
          path: candidate.path.clone(),
          status: EvictionStatus::Skipped,
          allocated_bytes_before,
          error: Some(CloudError::new(
            ErrorKind::InvalidState,
            format!("candidate is no longer eligible: {reason}"),
          )),
        }
      } else {
        match backend.evict_local_copy(&candidate.path, &candidate.fingerprint) {
          Ok(()) => match backend.inspect(&candidate.path) {
            Ok(after)
              if after.download_state == DownloadState::CloudOnly
                && after.fingerprint == candidate.fingerprint =>
            {
              EvictionResult {
                path: candidate.path.clone(),
                status: EvictionStatus::Evicted,
                allocated_bytes_before,
                error: None,
              }
            }
            Ok(_) => EvictionResult {
              path: candidate.path.clone(),
              status: EvictionStatus::RequestedUnverified,
              allocated_bytes_before,
              error: None,
            },
            Err(error) => EvictionResult {
              path: candidate.path.clone(),
              status: EvictionStatus::RequestedUnverified,
              allocated_bytes_before,
              error: Some(error),
            },
          },
          Err(error) => EvictionResult {
            path: candidate.path.clone(),
            status: EvictionStatus::Failed,
            allocated_bytes_before,
            error: Some(error),
          },
        }
      }
    }
    Ok(_) => EvictionResult {
      path: candidate.path.clone(),
      status: EvictionStatus::Skipped,
      allocated_bytes_before,
      error: Some(CloudError::new(
        ErrorKind::ChangedSincePlan,
        "file changed since the eviction plan was created",
      )),
    },
    Err(error) => EvictionResult {
      path: candidate.path.clone(),
      status: EvictionStatus::Skipped,
      allocated_bytes_before,
      error: Some(error),
    },
  }
}

pub fn request_download_one<B: CloudBackend + ?Sized>(backend: &B, path: &Path) -> Result<()> {
  let info = backend.inspect(path)?;
  if !info.is_icloud || info.kind() != ItemKind::RegularFile {
    return Err(CloudError::new(
      ErrorKind::InvalidState,
      "download requests require an iCloud regular file",
    ));
  }
  backend.request_download(path, &info.fingerprint)
}

fn path_storage_bytes(path: &Path) -> usize {
  // Covers path bytes, PathBuf/Vec bookkeeping, allocator slack, and per-entry
  // metadata in the pending stack or plan vectors.
  path.as_os_str().len().saturating_add(256)
}

fn stage_path(staged_bytes: &mut usize, path: &Path, budget: Option<PlanBudget>) -> bool {
  let Some(budget) = budget else {
    return true;
  };
  let Some(next) = staged_bytes.checked_add(path_storage_bytes(path)) else {
    return false;
  };
  if next > budget.max_staged_bytes {
    return false;
  }
  *staged_bytes = next;
  true
}

fn add_note(
  plan: &mut EvictionPlan,
  path: &Path,
  message: impl Into<String>,
  staged_bytes: &mut usize,
  budget: Option<PlanBudget>,
) -> bool {
  if !stage_path(staged_bytes, path, budget) {
    return false;
  }
  plan.notes_total += 1;
  plan.skipped.push(SkippedItem {
    path: path.to_path_buf(),
    reason: message.into(),
  });
  true
}
