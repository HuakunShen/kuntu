//! Local Node-API binding for the Kunkun file search Rust workspace.
//!
//! The binding intentionally exposes plain JavaScript DTOs and keeps the Rust
//! search/index crates as the source of truth. Each operation opens the SQLite
//! index inside a libuv task so Node/Electron callers do not hold a shared Rust
//! database connection in JavaScript state.

use kuntu_index::WriterService;
use kuntu_store::Home;
use std::fmt;
use std::sync::{Arc, Mutex};
type ReadPool = Arc<Mutex<Option<KuntuIndex>>>;

use kuntu_core::{
  IndexRebuildStats, IndexRefreshStats, IndexRepairStats, IndexRootStatus, MatchKind, SearchConfig,
  SearchQuery, SearchResult, SearchRoot,
};
use kuntu_index::{IndexedSearchOutcome, KuntuIndex, SearchOptions};
use napi::bindgen_prelude::*;
use napi_derive::napi;

#[napi(object)]
#[derive(Clone, Debug)]
pub struct SearchRootInput {
  pub path: String,
  pub enabled: Option<bool>,
  pub priority: Option<i32>,
  pub include_hidden: Option<bool>,
  pub include_ignored: Option<bool>,
}

#[napi(object)]
#[derive(Clone, Debug)]
pub struct SearchRequest {
  pub roots: Vec<SearchRootInput>,
  pub query: String,
  pub limit: Option<u32>,
  pub include_hidden: Option<bool>,
  pub include_ignored: Option<bool>,
  pub extensions: Option<Vec<String>>,
}

#[napi(object)]
pub struct IndexRebuildStatsDto {
  pub roots: u32,
  pub entries: u32,
  pub skipped: u32,
  pub errors: Vec<String>,
}

#[napi(object)]
pub struct IndexRefreshStatsDto {
  pub roots: u32,
  pub inserted: u32,
  pub updated: u32,
  pub deleted: u32,
  pub unchanged: u32,
  pub skipped: u32,
  pub errors: Vec<String>,
}

#[napi(object)]
pub struct IndexRepairStatsDto {
  pub roots: u32,
  pub dirty_roots: u32,
  pub repaired_roots: u32,
  pub errors: Vec<String>,
}

#[napi(object)]
pub struct IndexRootStatusDto {
  pub status: String,
  pub path: String,
  pub entry_count: u32,
  pub generation: f64,
  pub dirty: bool,
  pub last_full_scan_at: Option<f64>,
  pub last_incremental_at: Option<f64>,
}

#[napi(object)]
pub struct SearchResultDto {
  pub path: String,
  pub score: i32,
  pub provider: String,
  pub matches: Vec<String>,
}

#[napi(object)]
pub struct IndexedSearchOutcomeDto {
  pub coverage: String,
  pub candidate_count: u32,
  pub results: Vec<SearchResultDto>,
}

#[napi(object)]
pub struct SharedStoreOptions {
  pub home: Option<String>,
  pub consumer: Option<String>,
}

#[napi]
pub struct FileSearchIndex {
  db_path: String,
  read_only: bool,
  shared: bool,
  consumer: String,
  _service: Option<Arc<WriterService>>,
  _reader: Option<ReadPool>,
}

#[napi]
impl FileSearchIndex {
  #[napi(factory)]
  pub fn open_shared(home: Option<String>, consumer: Option<String>) -> Result<Self> {
    let path = home
      .map(std::path::PathBuf::from)
      .map(Ok)
      .unwrap_or_else(kuntu_store::resolve)
      .map_err(napi_error)?;
    let h = Home::open(&path).map_err(napi_error)?;
    let checked = KuntuIndex::open_shared(h.clone(), false, consumer.as_deref().unwrap_or("local"))
      .map_err(napi_error)?;
    let readonly = checked.access() == kuntu_index::Access::ReadOnly;
    drop(checked);
    let read = KuntuIndex::open_shared(h.clone(), true, consumer.as_deref().unwrap_or("local"))
      .map_err(napi_error)?;
    Ok(Self {
      db_path: path.to_string_lossy().into_owned(),
      read_only: readonly,
      shared: true,
      consumer: consumer.unwrap_or_else(|| "local".into()),
      _service: (!readonly).then(|| Arc::new(WriterService::start(h))),
      _reader: Some(Arc::new(Mutex::new(Some(read)))),
    })
  }
  #[napi(factory)]
  pub fn open_client(home: Option<String>, consumer: Option<String>) -> Result<Self> {
    let path = home
      .map(std::path::PathBuf::from)
      .map(Ok)
      .unwrap_or_else(kuntu_store::resolve)
      .map_err(napi_error)?;
    let h = Home::open(&path).map_err(napi_error)?;
    let name = consumer.unwrap_or_else(|| "local".into());
    let checked = KuntuIndex::open_shared(h.clone(), false, &name).map_err(napi_error)?;
    let readonly = checked.access() == kuntu_index::Access::ReadOnly;
    drop(checked);
    let reader = KuntuIndex::open_shared(h, true, &name).map_err(napi_error)?;
    Ok(Self {
      db_path: path.to_string_lossy().into_owned(),
      read_only: readonly,
      shared: true,
      consumer: name,
      _service: None,
      _reader: Some(Arc::new(Mutex::new(Some(reader)))),
    })
  }
  #[napi]
  pub fn request_rebuild(&self, roots: Vec<SearchRootInput>) -> Result<String> {
    if self.read_only {
      return Err(napi_error("Store is read-only."));
    }
    if !self.shared {
      return Err(napi_error("Inbox requires a shared store."));
    }
    let index = open_task_index(&self.db_path, false, true, &self.consumer)?;
    index
      .enqueue("rebuild", &config_from_roots(&roots))
      .map(|id| id.to_string())
      .map_err(napi_error)
  }
  #[napi(factory)]
  pub fn open_read_only(home: Option<String>, consumer: Option<String>) -> Result<Self> {
    let path = home
      .map(std::path::PathBuf::from)
      .map(Ok)
      .unwrap_or_else(kuntu_store::resolve)
      .map_err(napi_error)?;
    let h = Home::open(&path).map_err(napi_error)?;
    let read = KuntuIndex::open_shared(h, true, consumer.as_deref().unwrap_or("local"))
      .map_err(napi_error)?;
    Ok(Self {
      db_path: path.to_string_lossy().into_owned(),
      read_only: true,
      shared: true,
      consumer: consumer.unwrap_or_else(|| "local".into()),
      _service: None,
      _reader: Some(Arc::new(Mutex::new(Some(read)))),
    })
  }
  #[napi]
  pub fn close(&mut self) {
    self._reader.take();
    self._service.take();
  }
  /// Informational capability state; bulk methods still validate the owned lease.
  #[napi]
  pub fn writer_lease_held(&self) -> bool {
    self
      ._service
      .as_ref()
      .is_some_and(|service| service.is_holder())
  }
  #[napi]
  pub fn mode(&self) -> String {
    format!(
      "{}:{}",
      if self.shared { "shared" } else { "private" },
      if self.read_only {
        "read-only"
      } else {
        "read-write"
      }
    )
  }
  #[napi]
  pub fn register_roots(&self, roots: Vec<SearchRootInput>) -> Result<()> {
    if self.read_only {
      return Err(napi_error("Store is read-only."));
    }
    let index = open_task_index(&self.db_path, false, self.shared, &self.consumer)?;
    for root in config_from_roots(&roots).roots {
      index.register_root(&root).map_err(napi_error)?;
    }
    Ok(())
  }

  #[napi(constructor)]
  pub fn new(input: Either<String, SharedStoreOptions>) -> Result<Self> {
    match input {
      Either::A(db_path) => Ok(Self {
        db_path,
        read_only: false,
        shared: false,
        consumer: "private".into(),
        _service: None,
        _reader: None,
      }),
      Either::B(options) => Self::open_shared(options.home, options.consumer),
    }
  }
  #[napi(factory)]
  pub fn with_home(home: Option<String>) -> Result<Self> {
    Self::open_shared(home, None)
  }

  #[napi(ts_return_type = "Promise<IndexRebuildStatsDto>")]
  pub fn rebuild(&self, roots: Vec<SearchRootInput>) -> AsyncTask<RebuildTask> {
    AsyncTask::new(RebuildTask {
      db_path: self.db_path.clone(),
      read_only: self.read_only,
      shared: self.shared,
      consumer: self.consumer.clone(),
      _service: self._service.clone(),
      _reader: self._reader.clone(),
      roots,
    })
  }

  #[napi(ts_return_type = "Promise<IndexRefreshStatsDto>")]
  pub fn refresh(&self, roots: Vec<SearchRootInput>) -> AsyncTask<RefreshTask> {
    AsyncTask::new(RefreshTask {
      db_path: self.db_path.clone(),
      read_only: self.read_only,
      shared: self.shared,
      consumer: self.consumer.clone(),
      _service: self._service.clone(),
      _reader: self._reader.clone(),
      roots,
    })
  }

  #[napi(ts_return_type = "Promise<IndexRepairStatsDto>")]
  pub fn repair(&self, roots: Vec<SearchRootInput>) -> AsyncTask<RepairTask> {
    AsyncTask::new(RepairTask {
      db_path: self.db_path.clone(),
      read_only: self.read_only,
      shared: self.shared,
      consumer: self.consumer.clone(),
      _service: self._service.clone(),
      _reader: self._reader.clone(),
      roots,
    })
  }

  #[napi(ts_return_type = "Promise<Array<IndexRootStatusDto>>")]
  pub fn status(&self) -> AsyncTask<StatusTask> {
    AsyncTask::new(StatusTask {
      db_path: self.db_path.clone(),
      read_only: self.read_only,
      shared: self.shared,
      consumer: self.consumer.clone(),
      _service: self._service.clone(),
      _reader: self._reader.clone(),
    })
  }

  #[napi(ts_return_type = "Promise<IndexedSearchOutcomeDto>")]
  pub fn search(&self, request: SearchRequest) -> AsyncTask<SearchTask> {
    AsyncTask::new(SearchTask {
      db_path: self.db_path.clone(),
      read_only: self.read_only,
      shared: self.shared,
      consumer: self.consumer.clone(),
      _service: self._service.clone(),
      _reader: self._reader.clone(),
      request,
    })
  }
}

pub struct RebuildTask {
  db_path: String,
  read_only: bool,
  shared: bool,
  consumer: String,
  _service: Option<Arc<WriterService>>,
  _reader: Option<ReadPool>,
  roots: Vec<SearchRootInput>,
}

impl Task for RebuildTask {
  type Output = IndexRebuildStatsDto;
  type JsValue = IndexRebuildStatsDto;

  fn compute(&mut self) -> Result<Self::Output> {
    let config = config_from_roots(&self.roots);
    let mut index = open_task_index(&self.db_path, self.read_only, self.shared, &self.consumer)?;
    if self.read_only {
      return Err(napi_error("Store is read-only."));
    }
    if self.shared {
      index.enqueue("rebuild", &config).map_err(napi_error)?;
      return Err(napi_error("Index request queued."));
    }
    let stats = index.rebuild(&config).map_err(napi_error)?;
    Ok(stats.into())
  }

  fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
    Ok(output)
  }
}

pub struct RefreshTask {
  db_path: String,
  read_only: bool,
  shared: bool,
  consumer: String,
  _service: Option<Arc<WriterService>>,
  _reader: Option<ReadPool>,
  roots: Vec<SearchRootInput>,
}

impl Task for RefreshTask {
  type Output = IndexRefreshStatsDto;
  type JsValue = IndexRefreshStatsDto;

  fn compute(&mut self) -> Result<Self::Output> {
    let config = config_from_roots(&self.roots);
    let mut index = open_task_index(&self.db_path, self.read_only, self.shared, &self.consumer)?;
    if self.read_only {
      return Err(napi_error("Store is read-only."));
    }
    if self.shared {
      index.enqueue("refresh", &config).map_err(napi_error)?;
      return Err(napi_error("Index request queued."));
    }
    let stats = index.refresh(&config).map_err(napi_error)?;
    Ok(stats.into())
  }

  fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
    Ok(output)
  }
}

pub struct RepairTask {
  db_path: String,
  read_only: bool,
  shared: bool,
  consumer: String,
  _service: Option<Arc<WriterService>>,
  _reader: Option<ReadPool>,
  roots: Vec<SearchRootInput>,
}

impl Task for RepairTask {
  type Output = IndexRepairStatsDto;
  type JsValue = IndexRepairStatsDto;

  fn compute(&mut self) -> Result<Self::Output> {
    let config = config_from_roots(&self.roots);
    let mut index = open_task_index(&self.db_path, self.read_only, self.shared, &self.consumer)?;
    if self.read_only {
      return Err(napi_error("Store is read-only."));
    }
    if self.shared {
      index.enqueue("repair", &config).map_err(napi_error)?;
      return Err(napi_error("Index request queued."));
    }
    let stats = index.repair(&config).map_err(napi_error)?;
    Ok(stats.into())
  }

  fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
    Ok(output)
  }
}

pub struct StatusTask {
  db_path: String,
  read_only: bool,
  shared: bool,
  consumer: String,
  _service: Option<Arc<WriterService>>,
  _reader: Option<ReadPool>,
}

impl Task for StatusTask {
  type Output = Vec<IndexRootStatusDto>;
  type JsValue = Vec<IndexRootStatusDto>;

  fn compute(&mut self) -> Result<Self::Output> {
    with_reader(
      &self.db_path,
      self.read_only,
      self.shared,
      &self.consumer,
      &self._reader,
      |index| {
        let status = index.status().map_err(napi_error)?;
        status
          .into_iter()
          .map(|row| {
            let phase = index.root_state_name(&row.path).map_err(napi_error)?;
            let mut dto: IndexRootStatusDto = row.into();
            dto.status = phase;
            Ok(dto)
          })
          .collect()
      },
    )
  }

  fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
    Ok(output)
  }
}

/// Candidate budget of the JS search entry points. The addon's DTO exposes
/// only `candidate_count` and `results` — never `IndexedSearchOutcome::
/// coverage` — so JavaScript has no way to learn that a search was cut short.
/// The budget therefore stays at the v0.4.0 hard bound: a match beyond
/// `DEFAULT_FETCH_BUDGET` would otherwise disappear with no truncation
/// signal (M1 review P2). The frozen JS surface is unchanged.
const JS_CANDIDATE_BUDGET: SearchOptions = SearchOptions {
  fetch_budget: kuntu_index::MAX_KFS_CANDIDATE_ROWS,
};

pub struct SearchTask {
  db_path: String,
  read_only: bool,
  shared: bool,
  consumer: String,
  _service: Option<Arc<WriterService>>,
  _reader: Option<ReadPool>,
  request: SearchRequest,
}

impl Task for SearchTask {
  type Output = IndexedSearchOutcomeDto;
  type JsValue = IndexedSearchOutcomeDto;

  fn compute(&mut self) -> Result<Self::Output> {
    with_reader(
      &self.db_path,
      self.read_only,
      self.shared,
      &self.consumer,
      &self._reader,
      |index| {
        let config = config_from_roots(&self.request.roots);
        let query = query_from_request(&self.request);
        index
          .search_with_options(
            &config,
            &query,
            if self.shared {
              SearchOptions::default()
            } else {
              JS_CANDIDATE_BUDGET
            },
          )
          .map(Into::into)
          .map_err(napi_error)
      },
    )
  }

  fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
    Ok(output)
  }
}

fn with_reader<T>(
  path: &str,
  readonly: bool,
  shared: bool,
  consumer: &str,
  pool: &Option<ReadPool>,
  read: impl FnOnce(&KuntuIndex) -> Result<T>,
) -> Result<T> {
  if let Some(pool) = pool {
    let mut cached = pool.lock().map_err(napi_error)?;
    if cached
      .as_ref()
      .is_some_and(|index| index.needs_revalidation().unwrap_or(true))
    {
      cached.take();
    }
    if cached.is_none() {
      *cached = Some(open_task_index(path, true, shared, consumer)?);
    }
    let index = cached.as_ref().expect("opened native reader");
    let result = index
      .recheck_read_access()
      .map_err(napi_error)
      .and_then(|()| read(index));
    // All task clones share this slot. Release its connection and epoch
    // registration before the service attempts exclusive revalidation.
    if index.needs_revalidation().unwrap_or(true) {
      cached.take();
    }
    result
  } else {
    let index = open_task_index(path, readonly, shared, consumer)?;
    index.recheck_read_access().map_err(napi_error)?;
    read(&index)
  }
}
fn open_task_index(
  db_path: &str,
  read_only: bool,
  shared: bool,
  consumer: &str,
) -> Result<KuntuIndex> {
  if shared {
    KuntuIndex::open_shared(
      Home::open(std::path::Path::new(db_path)).map_err(napi_error)?,
      read_only,
      consumer,
    )
    .map_err(napi_error)
  } else if read_only {
    KuntuIndex::open_read_only(db_path).map_err(napi_error)
  } else {
    KuntuIndex::open(db_path).map_err(napi_error)
  }
}

fn config_from_roots(roots: &[SearchRootInput]) -> SearchConfig {
  SearchConfig {
    roots: roots
      .iter()
      .map(|input| {
        let mut root = SearchRoot::new(&input.path);
        if let Some(enabled) = input.enabled {
          root.enabled = enabled;
        }
        if let Some(priority) = input.priority {
          root.priority = priority;
        }
        if let Some(include_hidden) = input.include_hidden {
          root.include_hidden = include_hidden;
        }
        if let Some(include_ignored) = input.include_ignored {
          root.include_ignored = include_ignored;
        }
        root
      })
      .collect(),
  }
}

fn query_from_request(request: &SearchRequest) -> SearchQuery {
  let mut query = SearchQuery::new(&request.query);
  if let Some(limit) = request.limit {
    query.limit = limit as usize;
  }
  if let Some(include_hidden) = request.include_hidden {
    query.include_hidden = include_hidden;
  }
  if let Some(include_ignored) = request.include_ignored {
    query.include_ignored = include_ignored;
  }
  if let Some(extensions) = &request.extensions {
    query.extensions = extensions.clone();
  }
  query
}

fn napi_error(error: impl fmt::Display) -> Error {
  Error::new(Status::GenericFailure, error.to_string())
}

fn count_to_u32(value: usize) -> u32 {
  u32::try_from(value).unwrap_or(u32::MAX)
}

fn option_i64_to_f64(value: Option<i64>) -> Option<f64> {
  value.map(|value| value as f64)
}

fn match_kind_name(kind: MatchKind) -> String {
  format!("{kind:?}")
}

impl From<IndexRebuildStats> for IndexRebuildStatsDto {
  fn from(stats: IndexRebuildStats) -> Self {
    Self {
      roots: count_to_u32(stats.roots),
      entries: count_to_u32(stats.entries),
      skipped: count_to_u32(stats.skipped),
      errors: stats.errors,
    }
  }
}

impl From<IndexRefreshStats> for IndexRefreshStatsDto {
  fn from(stats: IndexRefreshStats) -> Self {
    Self {
      roots: count_to_u32(stats.roots),
      inserted: count_to_u32(stats.inserted),
      updated: count_to_u32(stats.updated),
      deleted: count_to_u32(stats.deleted),
      unchanged: count_to_u32(stats.unchanged),
      skipped: count_to_u32(stats.skipped),
      errors: stats.errors,
    }
  }
}

impl From<IndexRepairStats> for IndexRepairStatsDto {
  fn from(stats: IndexRepairStats) -> Self {
    Self {
      roots: count_to_u32(stats.roots),
      dirty_roots: count_to_u32(stats.dirty_roots),
      repaired_roots: count_to_u32(stats.repaired_roots),
      errors: stats.errors,
    }
  }
}

impl From<IndexRootStatus> for IndexRootStatusDto {
  fn from(status: IndexRootStatus) -> Self {
    Self {
      status: if status.dirty { "dirty" } else { "ready" }.into(),
      path: status.path.to_string_lossy().into_owned(),
      entry_count: count_to_u32(status.entry_count),
      generation: status.generation as f64,
      dirty: status.dirty,
      last_full_scan_at: option_i64_to_f64(status.last_full_scan_at),
      last_incremental_at: option_i64_to_f64(status.last_incremental_at),
    }
  }
}

impl From<SearchResult> for SearchResultDto {
  fn from(result: SearchResult) -> Self {
    Self {
      path: result.path.to_string_lossy().into_owned(),
      score: result.score,
      provider: result.provider,
      matches: result.matches.into_iter().map(match_kind_name).collect(),
    }
  }
}

impl From<IndexedSearchOutcome> for IndexedSearchOutcomeDto {
  fn from(outcome: IndexedSearchOutcome) -> Self {
    Self {
      coverage: match outcome.coverage {
        kuntu_index::CandidateCoverage::Complete => "complete",
        kuntu_index::CandidateCoverage::Truncated => "truncated",
        kuntu_index::CandidateCoverage::Building => "building",
      }
      .into(),
      candidate_count: count_to_u32(outcome.candidate_count),
      results: outcome.results.into_iter().map(Into::into).collect(),
    }
  }
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::path::PathBuf;

  use kuntu_core::{SearchConfig, SearchRoot};
  use kuntu_index::KuntuIndex;

  use super::*;
  #[test]
  fn cached_native_reader_releases_corrupt_epoch_before_revalidation() {
    use std::os::unix::fs::FileExt;
    let base = std::env::temp_dir().join(format!(
      "napi-corrupt-{}",
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
    ));
    let home = Home::open(&base).unwrap();
    let root = base.join("files");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("visible-file.md"), "x").unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    let mut seed = KuntuIndex::open_shared(home.clone(), false, "client").unwrap();
    seed.register_root(&config.roots[0]).unwrap();
    assert!(seed.acquire_writer().unwrap());
    seed.rebuild(&config).unwrap();
    drop(seed);
    let raw = rusqlite::Connection::open(base.join("index.v3.db")).unwrap();
    let page: u32 = raw
      .query_row(
        "SELECT rootpage FROM sqlite_master WHERE name='entries'",
        [],
        |r| r.get(0),
      )
      .unwrap();
    let size: u32 = raw.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
    raw
      .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
      .unwrap();
    drop(raw);
    let client = FileSearchIndex::open_client(
      Some(base.to_string_lossy().into_owned()),
      Some("client".into()),
    )
    .unwrap();
    let file = home.file("index.v3.db", false).unwrap();
    file
      .write_all_at(&[0xff], u64::from(page - 1) * u64::from(size))
      .unwrap();
    file.sync_all().unwrap();
    let result = with_reader(
      &client.db_path,
      false,
      true,
      "client",
      &client._reader,
      |index| {
        index
          .search_with_metrics(&config, &kuntu_core::SearchQuery::new("visible"))
          .map_err(napi_error)
      },
    );
    assert!(result.is_err(), "damaged data page was not diagnosed");
    assert!(
      home.lock("epoch-3.lock", false).unwrap().is_some(),
      "native read cache pinned the corrupt epoch after diagnosis"
    );
    let rebuilt = KuntuIndex::open_shared(home.clone(), false, "client").unwrap();
    assert_eq!(rebuilt.doctor().unwrap(), "ok");
    drop(rebuilt);
    let status = with_reader(
      &client.db_path,
      false,
      true,
      "client",
      &client._reader,
      |index| index.status().map_err(napi_error),
    )
    .unwrap();
    assert!(
      status.is_empty(),
      "native cache reused the quarantined connection"
    );
    drop(client);
    fs::remove_dir_all(base).unwrap();
  }

  /// The JS surface cannot see `IndexedSearchOutcome::coverage` — the addon's
  /// DTO keeps only `candidate_count` and `results` — so a search through the
  /// addon must keep the v0.4.0 result quality on its own. With 6,001
  /// matching files, the exact basename planted last (highest id, strongest
  /// match) has to come back; the 5,000-candidate Rust default would drop it
  /// with no truncation signal in JavaScript (M1 review P2).
  #[test]
  fn search_returns_the_exact_basename_beyond_the_rust_default_budget() {
    let nonce = std::process::id();
    let root: PathBuf = std::env::temp_dir().join(format!("kuntu-napi-budget-root-{nonce}"));
    let db_dir: PathBuf = std::env::temp_dir().join(format!("kuntu-napi-budget-db-{nonce}"));
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&db_dir);
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&db_dir).unwrap();
    for number in 0..6_000 {
      fs::write(root.join(format!("common-{number:05}.md")), "x\n").unwrap();
    }
    // Planted last: the highest id, and the exact basename for the query.
    fs::write(root.join("common.md"), "x\n").unwrap();
    let db_path = db_dir.join("budget.sqlite");

    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    let mut index = KuntuIndex::open(&db_path).unwrap();
    index.rebuild(&config).unwrap();
    drop(index);

    let mut task = SearchTask {
      db_path: db_path.to_string_lossy().into_owned(),
      read_only: false,
      shared: false,
      consumer: "private".into(),
      _service: None,
      _reader: None,
      request: SearchRequest {
        roots: vec![SearchRootInput {
          path: root.to_string_lossy().into_owned(),
          enabled: None,
          priority: None,
          include_hidden: None,
          include_ignored: None,
        }],
        query: "common".to_string(),
        limit: None,
        include_hidden: None,
        include_ignored: None,
        extensions: None,
      },
    };
    let outcome = task.compute().unwrap();

    let _ = fs::remove_dir_all(&db_dir);
    let _ = fs::remove_dir_all(&root);

    assert_eq!(outcome.candidate_count, 6_001);
    assert!(
      outcome
        .results
        .iter()
        .any(|result| result.path.ends_with("common.md")),
      "the exact basename must survive the candidate budget; results: {:?}",
      outcome
        .results
        .iter()
        .map(|result| result.path.clone())
        .collect::<Vec<_>>()
    );
  }
  #[test]
  fn old_constructor_opens_exact_v2_path_and_refuses_unrelated_database() {
    let base = std::env::temp_dir().join(format!("napi-v2-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let root = base.join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("v2-file.md"), "x").unwrap();
    let path = base.join("original.db");
    let c = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    let mut legacy = kuntu_legacy_index::KuntuIndex::open(&path).unwrap();
    legacy.rebuild(&c).unwrap();
    drop(legacy);
    let instance = FileSearchIndex::new(Either::A(path.to_string_lossy().into_owned())).unwrap();
    assert!(!instance.shared);
    let mut status = StatusTask {
      db_path: instance.db_path.clone(),
      read_only: instance.read_only,
      shared: instance.shared,
      consumer: instance.consumer.clone(),
      _service: None,
      _reader: None,
    };
    assert_eq!(status.compute().unwrap().len(), 1);
    let db =
      rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    assert_eq!(
      db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .unwrap(),
      2
    );
    drop(db);
    let foreign = base.join("foreign.db");
    let db = rusqlite::Connection::open(&foreign).unwrap();
    db.execute_batch("CREATE TABLE valuable(value TEXT);INSERT INTO valuable VALUES('keep');")
      .unwrap();
    drop(db);
    let before = fs::read(&foreign).unwrap();
    let instance = FileSearchIndex::new(Either::A(foreign.to_string_lossy().into_owned())).unwrap();
    let mut task = StatusTask {
      db_path: instance.db_path.clone(),
      read_only: instance.read_only,
      shared: instance.shared,
      consumer: instance.consumer.clone(),
      _service: None,
      _reader: None,
    };
    assert!(task.compute().is_err());
    assert_eq!(before, fs::read(&foreign).unwrap());
    fs::remove_dir_all(base).unwrap();
  }
}
