//! M2 SQLite WAL metadata index; query shape retained from M1.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kuntu_core::{
  ranking::{score_candidate, tokenize},
  BackendError, EntryKind, IndexRebuildStats, IndexRefreshStats, IndexRepairStats, IndexRootStatus,
  MatchKind, MetadataIndex, SearchCandidate, SearchConfig, SearchQuery, SearchResult, SearchRoot,
};
use kuntu_crawler::CrawlOptions;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter};
mod engine;
mod shared;
use kuntu_store::{Home, ReaderGuard};
mod walk;
pub use shared::{Access, WriterService};

pub type RebuildStats = IndexRebuildStats;
pub type RefreshStats = IndexRefreshStats;
pub type RepairStats = IndexRepairStats;
pub type RootStatus = IndexRootStatus;

/// Monotonic schema version of the index database, stored in `PRAGMA
/// user_version`.
///
/// History: 1 was the rusqlite era. 2 marks the turso engine. Databases from
/// version 0 (pre-versioning) or 1 are recreated rather than migrated
/// in-place — the index is derived data and rebuilding from source
/// directories never touches user data, while turso-era builds avoid
/// turso's `ALTER TABLE ADD COLUMN` bug cluster. Databases from a newer
/// version are refused with `IndexError::SchemaTooNew`.
pub const SCHEMA_VERSION: i64 = 3;

/// Synchronous facade over turso's async API. Every public method runs on
/// the caller's thread through a minimal executor; turso's base crate does
/// not require tokio (its futures are waker-based).
fn block_on<F: std::future::Future>(future: F) -> F::Output {
  futures::executor::block_on(future)
}

/// Error surface of the index. Callers only rely on `Display`; the typed
/// variants exist so the crate and its tests can distinguish recoverable
/// states (recreate the file and rebuild) from real engine failures.
#[derive(Debug)]
pub enum IndexError {
  Engine(rusqlite::Error),
  Io(std::io::Error),
  SchemaTooNew { found: i64, supported: i64 },
  RecreateRequired,
  ReadOnly,
  LeaseRequired,
  EmptyScope,
  StaleScan,
  Cancelled,
  RecreateFailed(Box<IndexError>),
  UnrecognizedDatabase { path: PathBuf },
}

impl std::fmt::Display for IndexError {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      IndexError::ReadOnly => write!(formatter,"Store is read-only."),
      IndexError::LeaseRequired => write!(formatter,"An owned writer lease is required."),
      IndexError::EmptyScope => write!(formatter,"Search scope must be non-empty."),
      IndexError::Cancelled => write!(formatter,"The scan was cancelled; its root remains recoverable."),
      IndexError::StaleScan => write!(formatter,"Scan generation or consumer policy changed."),
      IndexError::Engine(error) => write!(formatter, "index engine error: {error}"),
      IndexError::Io(error) => write!(formatter, "index io error: {error}"),
      IndexError::SchemaTooNew { found, supported } => write!(
        formatter,
        "file-search index schema version {found} is newer than supported version {supported}; recreate the index file and rebuild"
      ),
      IndexError::RecreateRequired => write!(
        formatter,
        "index database predates the turso engine and must be recreated"
      ),
      IndexError::UnrecognizedDatabase { path } => write!(
        formatter,
        "{} has user tables but is not a file-search index; move it aside or choose another --db, it was not modified",
        path.display()
      ),
      IndexError::RecreateFailed(error) => write!(
        formatter,
        "index recreation after legacy detection failed: {error}"
      ),
    }
  }
}

impl std::error::Error for IndexError {}

impl From<rusqlite::Error> for IndexError {
  fn from(error: rusqlite::Error) -> Self {
    IndexError::Engine(error)
  }
}

impl From<std::io::Error> for IndexError {
  fn from(error: std::io::Error) -> Self {
    IndexError::Io(error)
  }
}

pub type Result<T> = std::result::Result<T, IndexError>;

/// The candidate work bound for one search. A broad indexed prefix stops
/// yielding candidates here and reports [`CandidateCoverage::Truncated`]
/// instead of claiming complete coverage over an unbounded root.
/// Highest schema minor this compiled engine can maintain.
pub const BUILD_MINOR: i64 = 1;
pub const MAX_KFS_CANDIDATE_ROWS: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateCoverage {
  /// Every candidate the configured roots could offer was ranked.
  Complete,
  /// The candidate work bound was reached; results are deterministic but
  /// partial and never claim to be the best over the whole root.
  Truncated,
  Building,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedSearchOutcome {
  /// Candidates the SQL stages produced, after the work bound.
  pub candidate_count: usize,
  pub results: Vec<SearchResult>,
  /// More valid results existed than the requested limit.
  pub truncated: bool,
  pub coverage: CandidateCoverage,
}

#[derive(Debug)]
pub struct KuntuIndex {
  conn: engine::Connection,
  legacy: Option<kuntu_legacy_index::KuntuIndex>,
  home: Option<Home>,
  cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
  _reader: Option<ReaderGuard>,
  shared: bool,
  consumer: String,
  data_version: std::cell::Cell<i64>,
}

/// Default number of candidates one search loads and ranks. Candidates are
/// counted after scope, kind, extension and policy filtering, so rows the
/// caller may not see never consume the budget. Reaching it reports
/// [`CandidateCoverage::Truncated`].
pub const DEFAULT_FETCH_BUDGET: usize = 5_000;

/// Ids per `IN (...)` statement when loading candidate rows.
const FETCH_CHUNK_IDS: usize = 1_000;

/// Per-search tuning. `search_with_metrics` uses the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
  /// Maximum admitted candidates to load and rank. Clamped to
  /// `1..=MAX_KFS_CANDIDATE_ROWS`.
  pub fetch_budget: usize,
}

impl Default for SearchOptions {
  fn default() -> Self {
    Self {
      fetch_budget: DEFAULT_FETCH_BUDGET,
    }
  }
}

impl MetadataIndex for KuntuIndex {
  fn rebuild_index(
    &mut self,
    config: &SearchConfig,
  ) -> std::result::Result<IndexRebuildStats, BackendError> {
    self.rebuild(config).map_err(index_error_to_backend)
  }

  fn refresh_index(
    &mut self,
    config: &SearchConfig,
  ) -> std::result::Result<IndexRefreshStats, BackendError> {
    self.refresh(config).map_err(index_error_to_backend)
  }

  fn repair_index(
    &mut self,
    config: &SearchConfig,
  ) -> std::result::Result<IndexRepairStats, BackendError> {
    self.repair(config).map_err(index_error_to_backend)
  }

  fn search_index(
    &self,
    config: &SearchConfig,
    query: &SearchQuery,
  ) -> std::result::Result<Vec<SearchResult>, BackendError> {
    self.search(config, query).map_err(index_error_to_backend)
  }

  fn status(&self) -> std::result::Result<Vec<IndexRootStatus>, BackendError> {
    KuntuIndex::status(self).map_err(index_error_to_backend)
  }
}

impl KuntuIndex {
  pub fn open(path: impl AsRef<Path>) -> Result<Self> {
    Self::open_private(path.as_ref(), false)
  }
  pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
    Self::open_private(path.as_ref(), true)
  }
  pub fn open_memory() -> Result<Self> {
    kuntu_vfs::register()?;
    Self::from_raw(
      rusqlite::Connection::open_in_memory()?,
      None,
      false,
      false,
      "private",
      0,
    )
  }
  #[cfg(test)]
  async fn schema_version(&self) -> Result<i64> {
    Ok(
      self
        .conn
        .raw
        .query_row("PRAGMA user_version", [], |r| r.get(0))?,
    )
  }
  pub fn rebuild(&mut self, config: &SearchConfig) -> Result<RebuildStats> {
    self.ensure_lease()?;
    if let Some(legacy) = &mut self.legacy {
      return legacy.rebuild(config).map_err(legacy_error);
    }
    let mut entries_count = 0;
    let mut skipped = 0;
    let mut errors = Vec::new();
    for root in config.roots.iter().filter(|r| r.enabled) {
      let root_id = if self.shared {
        self.conn.raw.query_row("SELECT r.id FROM roots r JOIN root_consumers c ON c.root_id=r.id WHERE r.path=?1 AND c.consumer=?2 AND c.enabled=1",params![root.path.to_string_lossy(),self.consumer],|r|r.get(0))?
      } else {
        self.register_root(root)?
      };
      let gen = self.begin_scan(root_id)?;
      // The unmodified M1 crawler returns a snapshot; SQL batches never span
      // this blocking walk. Re-read policy after the walk and each commit.
      let crawl_config = self.index_config(root_id)?;
      let (entries, stats) =
        walk::crawl_cancellable(&CrawlOptions::new(crawl_config), &self.cancel)?;
      self.check_cancelled()?;
      skipped += stats.skipped;
      if !stats.errors.is_empty() {
        errors.extend(stats.errors);
        self.mark_root_dirty(root)?;
        continue;
      }
      self.check_scan(root_id, gen)?;
      let mut offset = 0;
      while offset < entries.len() {
        self.check_cancelled()?;
        let started = std::time::Instant::now();
        let tx = self.conn.begin_writer_txn(false)?;
        self.check_scan(root_id, gen)?;
        let mut rows = 0;
        while offset < entries.len()
          && rows < 5_000
          && (rows == 0 || started.elapsed() < std::time::Duration::from_millis(100))
        {
          self.check_cancelled()?;
          let entry = &entries[offset];
          if !entry.decision.sensitive {
            let (id, needs_postings) = block_on(insert_entry_tx(&tx, root_id, entry, gen))?;
            if needs_postings {
              block_on(insert_terms_tx(&tx, id, &entry.path))?;
            }
            entries_count += 1;
          }
          offset += 1;
          rows += 1;
          #[cfg(test)]
          if let Ok(ms) = std::env::var("KUNTU_TEST_ROW_DELAY_MS") {
            std::thread::sleep(std::time::Duration::from_millis(ms.parse().unwrap()));
          }
        }
        tx.conn.raw.execute("UPDATE root_state SET entry_count=(SELECT count(*) FROM entries WHERE root_id=?1) WHERE root_id=?1",params![root_id])?;
        block_on(tx.commit())?;
        #[cfg(test)]
        m2_tests::record_batch(rows);
        let threshold = std::env::var("KUNTU_FAULT_ROWS")
          .ok()
          .and_then(|v| v.parse::<usize>().ok())
          .unwrap_or(0);
        if offset >= threshold {
          shared::fault("batch_committed");
        }
      }
      self.complete_scan(root_id, gen)?;
    }
    Ok(RebuildStats {
      roots: config.roots.iter().filter(|r| r.enabled).count(),
      entries: entries_count,
      skipped,
      errors,
    })
  }
  pub fn refresh(&mut self, config: &SearchConfig) -> Result<RefreshStats> {
    self.ensure_lease()?;
    if let Some(legacy) = &mut self.legacy {
      return legacy.refresh(config).map_err(legacy_error);
    }
    let before = self.snapshot()?;
    let stats = self.rebuild(config)?;
    let after = self.snapshot()?;
    let inserted = after.keys().filter(|p| !before.contains_key(*p)).count();
    let deleted = before.keys().filter(|p| !after.contains_key(*p)).count();
    let unchanged = after
      .iter()
      .filter(|(p, v)| before.get(*p) == Some(*v))
      .count();
    let updated = after.len() - inserted - unchanged;
    Ok(RefreshStats {
      roots: stats.roots,
      inserted,
      updated,
      deleted,
      unchanged,
      skipped: stats.skipped,
      errors: stats.errors,
    })
  }
  pub fn repair(&mut self, config: &SearchConfig) -> Result<RepairStats> {
    self.ensure_lease()?;
    if let Some(legacy) = &mut self.legacy {
      return legacy.repair(config).map_err(legacy_error);
    }
    let configured_roots = config
      .roots
      .iter()
      .filter(|root| root.enabled)
      .cloned()
      .collect::<Vec<_>>();
    let configured_paths = configured_roots
      .iter()
      .map(|root| root.path.clone())
      .collect::<HashSet<_>>();
    let dirty_paths = self
      .status()?
      .into_iter()
      .filter(|status| status.dirty && configured_paths.contains(&status.path))
      .map(|status| status.path)
      .collect::<HashSet<_>>();
    let dirty_roots = dirty_paths.len();
    if dirty_roots == 0 {
      return Ok(RepairStats {
        roots: configured_roots.len(),
        dirty_roots: 0,
        repaired_roots: 0,
        errors: Vec::new(),
      });
    }

    let repair_config = SearchConfig {
      roots: configured_roots
        .into_iter()
        .filter(|root| dirty_paths.contains(&root.path))
        .collect(),
    };
    let refresh_stats = self.refresh(&repair_config)?;
    let repaired_roots = self
      .status()?
      .into_iter()
      .filter(|status| dirty_paths.contains(&status.path) && !status.dirty)
      .count();

    Ok(RepairStats {
      roots: config.roots.iter().filter(|root| root.enabled).count(),
      dirty_roots,
      repaired_roots,
      errors: refresh_stats.errors,
    })
  }

  pub fn mark_root_dirty(&mut self, root: &SearchRoot) -> Result<()> {
    self.ensure_lease()?;
    if let Some(legacy) = &mut self.legacy {
      return legacy.mark_root_dirty(root).map_err(legacy_error);
    }
    block_on(async {
      let tx = self.conn.transaction().await?;
      let root_id = if self.shared {
        tx.conn.raw.query_row("SELECT r.id FROM roots r JOIN root_consumers c ON c.root_id=r.id WHERE r.path=?1 AND c.consumer=?2 AND c.enabled=1",params![root.path.to_string_lossy(),self.consumer],|r|r.get(0))?
      } else {
        upsert_root_tx(&tx, root).await?
      };
      let count = count_entries_tx(&tx, root_id).await?;
      tx.execute(
        "INSERT INTO root_state (root_id, generation, last_full_scan_at, last_incremental_at, dirty, entry_count)
         VALUES (?1, COALESCE((SELECT generation FROM root_state WHERE root_id = ?1), 0), NULL, NULL, 1, ?2)
         ON CONFLICT(root_id) DO UPDATE SET dirty = 1, status = 'dirty'",
        params![root_id, count],
      )
      .await?;
      tx.commit().await?;
      Ok::<(), IndexError>(())
    })
  }

  pub fn search(&self, config: &SearchConfig, query: &SearchQuery) -> Result<Vec<SearchResult>> {
    Ok(self.search_with_metrics(config, query)?.results)
  }

  pub fn search_with_metrics(
    &self,
    config: &SearchConfig,
    query: &SearchQuery,
  ) -> Result<IndexedSearchOutcome> {
    self.search_with_options(config, query, SearchOptions::default())
  }

  /// Search with explicit options. Scope, kind, extension and policy filters
  /// apply to every candidate before ranking and before the fetch budget and
  /// the result window, exactly as in `search_with_metrics`.
  pub fn search_with_options(
    &self,
    config: &SearchConfig,
    query: &SearchQuery,
    options: SearchOptions,
  ) -> Result<IndexedSearchOutcome> {
    self.recheck_read_access()?;
    if !config.roots.iter().any(|r| r.enabled) {
      return Err(IndexError::EmptyScope);
    }
    if let Some(legacy) = &self.legacy {
      let o = legacy
        .search_with_options(
          config,
          query,
          kuntu_legacy_index::SearchOptions {
            fetch_budget: options.fetch_budget,
          },
        )
        .map_err(legacy_error)?;
      return Ok(IndexedSearchOutcome {
        candidate_count: o.candidate_count,
        results: o.results,
        truncated: o.truncated,
        coverage: match o.coverage {
          kuntu_legacy_index::CandidateCoverage::Complete => CandidateCoverage::Complete,
          kuntu_legacy_index::CandidateCoverage::Truncated => CandidateCoverage::Truncated,
        },
      });
    }
    let scope = kuntu_core::SearchScope::new(block_on(root_ids_for_config(&self.conn, config))?)
      .map_err(|_| IndexError::EmptyScope)?;
    self.search_scoped(config, &scope, query, options)
  }

  /// Explicit v3 scope; config roots and persisted consumer membership intersect it.
  pub fn search_scoped(
    &self,
    config: &SearchConfig,
    requested: &kuntu_core::SearchScope,
    query: &SearchQuery,
    options: SearchOptions,
  ) -> Result<IndexedSearchOutcome> {
    self.recheck_read_access()?;
    if self.legacy.is_some() {
      return Err(IndexError::RecreateRequired);
    }
    let configured = block_on(root_ids_for_config(&self.conn, config))?;
    let root_ids = kuntu_core::SearchScope::new(
      requested
        .ids()
        .iter()
        .filter(|id| configured.contains(id))
        .copied()
        .collect(),
    )
    .map_err(|_| IndexError::EmptyScope)?;
    let root_ids = root_ids.ids();
    let budget = options.fetch_budget.clamp(1, MAX_KFS_CANDIDATE_ROWS);
    self.poll_changes()?;
    let admission = block_on(CandidateScope::load(
      &self.conn,
      root_ids,
      query,
      &self.consumer,
      self.shared,
      config,
    ))?;
    if admission.allowed_roots.is_empty() {
      return Err(IndexError::EmptyScope);
    }
    block_on(async {
      let empty = IndexedSearchOutcome {
        candidate_count: 0,
        results: Vec::new(),
        truncated: false,
        coverage: if self.has_building(root_ids)? {
          CandidateCoverage::Building
        } else {
          CandidateCoverage::Complete
        },
      };
      if !query.include_files && !query.include_directories {
        return Ok(empty);
      }
      let query_terms = tokenize(&query.query);
      if query_terms.is_empty() {
        // A term-less or fully-unknown query is a miss: it returns an empty
        // complete result and never falls back to scanning the whole root.
        return Ok(empty);
      }

      // Stage 1: posting lists, one range seek per unique token.
      //
      // A token's list is cheap to load, but two broad tokens cannot be
      // intersected by capping each list (a selective token plus a common one
      // would silently lose rows: the common list is cut at the work bound and
      // most of the selective token's rows fall outside it). Instead the
      // smallest *complete* list drives the search: its entries are fetched
      // and every other token is verified against the row's own path, which is
      // exactly the predicate the terms table encodes. Only when every token
      // is too broad to load completely are the capped lists intersected, and
      // the result is then reported as truncated.
      let unique_terms: std::collections::BTreeSet<String> = query_terms.into_iter().collect();

      let mut lists: Vec<(String, TokenPostings)> = Vec::new();
      for term in &unique_terms {
        let postings = load_postings(&self.conn, term, DRIVER_PROBE_ROWS, usize::MAX).await?;
        if postings.complete && postings.ids.is_empty() {
          // A token that matches nothing ends the search: empty and complete.
          return Ok(empty);
        }
        lists.push((term.clone(), postings));
      }
      let mut driver = smallest_complete(&lists);
      if driver.is_none() {
        lists.clear();
        for term in &unique_terms {
          let postings = load_postings(
            &self.conn,
            term,
            MAX_KFS_CANDIDATE_ROWS * 3,
            MAX_KFS_CANDIDATE_ROWS,
          )
          .await?;
          lists.push((term.clone(), postings));
        }
        driver = smallest_complete(&lists);
      }
      let (id_list, verify_tokens, any_token_saturated): (Vec<i64>, Vec<String>, bool) =
        match driver {
          Some(position) => {
            let verify_tokens = lists
              .iter()
              .enumerate()
              .filter(|(index, _)| *index != position)
              .map(|(_, (term, _))| term.clone())
              .collect();
            (rank_ids(&[&lists[position].1.ids]), verify_tokens, false)
          }
          None => {
            let id_maps: Vec<&std::collections::BTreeMap<i64, bool>> =
              lists.iter().map(|(_, postings)| &postings.ids).collect();
            (rank_ids(&id_maps), Vec::new(), true)
          }
        };
      // Stage 2: load the candidate rows by id and filter in Rust.
      //
      // The fetch statement carries no predicate at all. turso's planner turns
      // any indexed predicate beside the id list (root_id, extension) into a
      // scan of that index, which costs a whole-root scan per batch; the bare
      // `id IN (...)` is a rowid seek. Scope, kind, extension and policy are
      // therefore applied to each fetched row before it can become a
      // candidate, so nothing outside the configured scope or policy is ever
      // ranked, and the budget counts only admitted rows.
      let scope = CandidateScope::load(
        &self.conn,
        root_ids,
        query,
        &self.consumer,
        self.shared,
        config,
      )
      .await?;
      let mut candidates: Vec<CandidateRow> = Vec::new();
      let mut examined = 0usize;
      'chunks: for chunk in id_list.chunks(FETCH_CHUNK_IDS) {
        let mut admitted: HashMap<i64, CandidateRow> = HashMap::new();
        for entry in fetch_entries_by_id(&self.conn, chunk).await? {
          if !verify_tokens.is_empty()
            && !verify_candidate_tokens(&self.conn, entry.id, &entry.path, &verify_tokens).await?
          {
            continue;
          }
          let id = entry.id;
          if let Some(candidate) = scope.admit(query, entry) {
            admitted.insert(id, candidate);
          }
        }
        for id in chunk {
          examined += 1;
          if let Some(candidate) = admitted.remove(id) {
            candidates.push(candidate);
            if candidates.len() == budget {
              break 'chunks;
            }
          }
        }
      }
      // The budget stopped the search before every id was examined.
      let budget_truncated = candidates.len() == budget && examined < id_list.len();
      let any_token_saturated = any_token_saturated || budget_truncated;
      let candidate_count = candidates.len();
      let candidate_saturated = any_token_saturated;

      let mut results = Vec::new();
      for row in candidates {
        let candidate = SearchCandidate {
          path: row.path,
          kind: row.kind,
          provider: "sqlite".to_string(),
        };
        let (score, matches) = score_candidate(query, &candidate, row.root_priority);
        if score > 0 {
          results.push(SearchResult {
            path: candidate.path,
            score,
            provider: candidate.provider,
            matches,
            kind: row.kind,
            byte_size: row.byte_size,
            modified_unix_seconds: row.modified_unix_seconds,
          });
        }
      }

      results.sort_by(|left, right| {
        right
          .score
          .cmp(&left.score)
          .then_with(|| left.path.cmp(&right.path))
      });
      let truncated = results.len() > query.limit;
      results.truncate(query.limit);
      let coverage = if self.has_building(root_ids)? {
        CandidateCoverage::Building
      } else if candidate_saturated {
        CandidateCoverage::Truncated
      } else {
        CandidateCoverage::Complete
      };
      Ok(IndexedSearchOutcome {
        candidate_count,
        results,
        truncated,
        coverage,
      })
    })
  }

  pub fn status(&self) -> Result<Vec<RootStatus>> {
    self.recheck_read_access()?;
    if let Some(legacy) = &self.legacy {
      return legacy.status().map_err(legacy_error);
    }
    block_on(async {
      let mut rows = self.conn.query(
        "SELECT r.path, COALESCE(s.entry_count, 0), COALESCE(s.generation, 0), COALESCE(s.dirty, 0),
                s.last_full_scan_at, s.last_incremental_at
         FROM roots r
         LEFT JOIN root_state s ON s.root_id = r.id
         WHERE (?1=0 OR r.id IN (SELECT root_id FROM root_consumers WHERE consumer=?2 AND enabled=1))
         ORDER BY r.path",
        params![self.shared,self.consumer],
      )
      .await?;
      let mut collected = Vec::new();
      while let Some(row) = rows.next().await? {
        collected.push(RootStatus {
          path: PathBuf::from(row.get::<String>(0)?),
          entry_count: usize::try_from(row.get::<i64>(1)?).unwrap_or(0),
          generation: row.get::<i64>(2)?,
          dirty: row.get::<i64>(3)? != 0,
          last_full_scan_at: row.get::<Option<i64>>(4)?,
          last_incremental_at: row.get::<Option<i64>>(5)?,
        });
      }
      Ok(collected)
    })
  }
}

/// One `entries` row as stored, fetched by id.
struct FetchedEntry {
  id: i64,
  root_id: i64,
  path: PathBuf,
  kind: EntryKind,
  size: Option<u64>,
  mtime: Option<u64>,
  extension: Option<String>,
  hidden: bool,
  ignored: bool,
  sensitive: bool,
  deleted: bool,
}

/// The candidate fetch. Keep it free of predicates: see the note at stage 2 of
/// `search_with_options` and the plan test that pins this statement.
fn entry_fetch_sql(id_count: usize) -> String {
  format!(
    "SELECT id, root_id, path, kind, size, mtime, extension, (flags & 1), (flags & 2), (flags & 4), 0
       FROM entries WHERE id IN ({})",
    placeholders(id_count)
  )
}

async fn fetch_entries_by_id(conn: &engine::Connection, ids: &[i64]) -> Result<Vec<FetchedEntry>> {
  let values: Vec<Value> = ids.iter().copied().map(Value::from).collect();
  let mut rows = conn
    .query(&entry_fetch_sql(ids.len()), params_from_iter(values))
    .await?;
  let mut fetched = Vec::with_capacity(ids.len());
  while let Some(row) = rows.next().await? {
    let mtime: Option<i64> = row.get(5)?;
    fetched.push(FetchedEntry {
      id: row.get::<i64>(0)?,
      root_id: row.get::<i64>(1)?,
      path: PathBuf::from(row.get::<String>(2)?),
      kind: int_to_kind(row.get::<i64>(3)?),
      size: u64::try_from(row.get::<i64>(4).unwrap_or(-1)).ok(),
      mtime: u64::try_from(mtime.unwrap_or(-1)).ok(),
      extension: row.get(6)?,
      hidden: row.get::<i64>(7)? != 0,
      ignored: row.get::<i64>(8)? != 0,
      sensitive: row.get::<i64>(9)? != 0,
      deleted: row.get::<i64>(10)? != 0,
    });
  }
  Ok(fetched)
}

/// Everything a fetched row is checked against before it can be ranked.
struct CandidateScope {
  allowed_roots: HashSet<i64>,
  /// root id -> (priority, include_hidden, include_ignored)
  root_meta: HashMap<i64, (i32, bool, bool)>,
  extensions: Vec<String>,
  shared: bool,
}

impl CandidateScope {
  async fn load(
    conn: &engine::Connection,
    root_ids: &[i64],
    query: &SearchQuery,
    consumer: &str,
    shared: bool,
    _config: &SearchConfig,
  ) -> Result<Self> {
    let mut root_meta = HashMap::new();
    let mut rows=conn.query("SELECT root_id,priority,include_hidden,include_ignored FROM root_consumers WHERE consumer=?1 AND enabled=1",params![consumer]).await?;
    while let Some(row) = rows.next().await? {
      root_meta.insert(
        row.get::<i64>(0)?,
        (
          row.get::<i32>(1)?,
          row.get::<i64>(2)? != 0,
          row.get::<i64>(3)? != 0,
        ),
      );
    }
    Ok(Self {
      shared,
      allowed_roots: root_ids
        .iter()
        .copied()
        .filter(|id| root_meta.contains_key(id))
        .collect(),
      root_meta,
      extensions: query
        .extensions
        .iter()
        .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
        .collect(),
    })
  }

  /// The v0.4.0 admission rules, unchanged: a row is a candidate only if it is
  /// live, not sensitive, inside a configured root, of a requested kind, has a
  /// requested extension, and is either clean or allowed by the query flag or
  /// by its own root's allowance.
  fn admit(&self, query: &SearchQuery, entry: FetchedEntry) -> Option<CandidateRow> {
    if entry.deleted
      || entry.sensitive
      || kuntu_core::denies_sensitive_path(&entry.path)
      || !self.allowed_roots.contains(&entry.root_id)
    {
      return None;
    }
    let kind_allowed = match entry.kind {
      EntryKind::File => query.include_files,
      EntryKind::Directory => query.include_directories,
      EntryKind::Other => query.include_files && query.include_directories,
    };
    if !kind_allowed {
      return None;
    }
    let (priority, root_hidden, root_ignored) = self
      .root_meta
      .get(&entry.root_id)
      .copied()
      .unwrap_or((0, false, false));
    if entry.hidden && !(root_hidden || (!self.shared && query.include_hidden)) {
      return None;
    }
    if entry.ignored && !(root_ignored || (!self.shared && query.include_ignored)) {
      return None;
    }
    if !self.extensions.is_empty()
      && !entry
        .extension
        .as_ref()
        .is_some_and(|extension| self.extensions.contains(extension))
    {
      return None;
    }
    Some(CandidateRow {
      path: entry.path,
      kind: entry.kind,
      root_priority: priority,
      byte_size: entry.size,
      modified_unix_seconds: entry.mtime,
    })
  }
}

/// True when every token is a prefix of some token of the path, counting only
/// the components that were certainly indexed: `insert_terms_tx` derives the
/// terms component by component from valid UTF-8, so a component containing
/// the U+FFFD replacement character is ambiguous (see
/// [`verify_candidate_tokens`]) and is not trusted here.
fn path_has_prefix_tokens(path: &Path, tokens: &[String]) -> bool {
  let path_tokens = trusted_path_tokens(path);
  tokens.iter().all(|token| {
    path_tokens
      .iter()
      .any(|candidate| candidate.starts_with(token.as_str()))
  })
}

/// The tokens of the path components that were certainly indexed.
fn trusted_path_tokens(path: &Path) -> Vec<String> {
  path
    .components()
    .filter_map(|component| component.as_os_str().to_str())
    .filter(|component| !component.contains('\u{FFFD}'))
    .flat_map(tokenize)
    .collect()
}

/// True when any component of the stored path carries the U+FFFD replacement
/// character.
fn path_has_lossy_component(path: &Path) -> bool {
  path
    .components()
    .any(|component| component.as_os_str().to_string_lossy().contains('\u{FFFD}'))
}

/// The one statement the lossy-path fallback reads: a single entry's stored
/// terms, including its extension term, through `reverse_postings`.
fn entry_terms_sql() -> &'static str {
  "SELECT encoded FROM reverse_postings WHERE entry_id = ?1"
}
fn postings_delete_sql(name: bool) -> String {
  format!("DELETE FROM {} WHERE term IN (SELECT value FROM json_each(kuntu_terms((SELECT encoded FROM reverse_postings WHERE entry_id=?1),?2))) AND entry_id=?1",if name{"name_postings"}else{"path_postings"})
}
fn encode_terms(name: &[String], path: &[String]) -> Result<Vec<u8>> {
  let json = serde_json::to_vec(&(name, path)).map_err(|e| std::io::Error::other(e.to_string()))?;
  Ok(zstd::stream::encode_all(json.as_slice(), 3)?)
}
fn decode_terms(encoded: &[u8]) -> Result<(Vec<String>, Vec<String>)> {
  let bytes = zstd::stream::decode_all(encoded)?;
  serde_json::from_slice(&bytes).map_err(|e| IndexError::Io(std::io::Error::other(e.to_string())))
}

/// Stage-2 verification of the driver's remaining tokens against one row.
///
/// A stored path cannot tell a legitimate U+FFFD character from the shadow of
/// invalid bytes: insertion indexes the first and skips the second (while
/// still indexing its valid extension), so neither inventing nor dropping a
/// match can be decided from the path text alone. Only the U+FFFD-free
/// components are trusted; when a token matches none of them and the path
/// carries U+FFFD anywhere, the tokens are verified against the entry's own
/// stored terms — exactly the predicate `v0.4.0`'s posting intersection
/// required. A path without U+FFFD is byte-identical to what was indexed, so
/// it needs no lookup.
async fn verify_candidate_tokens(
  conn: &engine::Connection,
  entry_id: i64,
  path: &Path,
  tokens: &[String],
) -> Result<bool> {
  if path_has_prefix_tokens(path, tokens) {
    return Ok(true);
  }
  if !path_has_lossy_component(path) {
    return Ok(false);
  }
  let mut remaining: Vec<&str> = tokens.iter().map(String::as_str).collect();
  let mut rows = conn.query(entry_terms_sql(), params![entry_id]).await?;
  while let Some(row) = rows.next().await? {
    let encoded = row.get::<Vec<u8>>(0)?;
    let (name, path) = decode_terms(&encoded)?;
    for term in name.iter().chain(&path) {
      remaining.retain(|token| !term.starts_with(token));
    }
    if remaining.is_empty() {
      return Ok(true);
    }
  }
  Ok(false)
}

/// Rows read from the terms table to decide whether a token's whole posting
/// list is small enough to drive a search.
const DRIVER_PROBE_ROWS: usize = 1_000;

/// One token's posting list: entry id -> whether the token hit the file name
/// (field 1) of that entry.
struct TokenPostings {
  ids: std::collections::BTreeMap<i64, bool>,
  /// False when a row or distinct-id limit cut the list short.
  complete: bool,
}

/// Load the posting rows of every term that starts with `term`, stopping after
/// `row_limit` rows or `distinct_cap` distinct ids. The statement is a plain
/// range over the covering primary-key index: no `DISTINCT`, no `ORDER BY`
/// (either makes turso choose the non-covering `idx_terms_term` and a sorter).
async fn load_postings(
  conn: &engine::Connection,
  term: &str,
  row_limit: usize,
  distinct_cap: usize,
) -> Result<TokenPostings> {
  let mut ids = std::collections::BTreeMap::new();
  let mut complete = true;
  for name in [true, false] {
    let values = vec![
      Value::from(term.to_string()),
      Value::from(term_prefix_upper_bound(term)),
      Value::from(i64::try_from(row_limit).unwrap_or(i64::MAX - 1) + 1),
    ];
    let mut statement = conn.raw.prepare_cached(&postings_sql(name))?;
    let mut rows = statement.query(params_from_iter(values))?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
      let id = row.get::<_, i64>(0)?;
      if count == row_limit || (!ids.contains_key(&id) && ids.len() == distinct_cap) {
        complete = false;
        break;
      }
      count += 1;
      let hit = ids.entry(id).or_insert(false);
      *hit |= name;
    }
  }
  Ok(TokenPostings { ids, complete })
}
fn postings_sql(name: bool) -> String {
  format!(
    "SELECT entry_id FROM {} WHERE term >= ?1 AND term < ?2 LIMIT ?3",
    if name {
      "name_postings"
    } else {
      "path_postings"
    }
  )
}

/// Position of the complete list with the fewest entries.
fn smallest_complete(lists: &[(String, TokenPostings)]) -> Option<usize> {
  lists
    .iter()
    .enumerate()
    .filter(|(_, (_, postings))| postings.complete)
    .min_by_key(|(_, (_, postings))| postings.ids.len())
    .map(|(position, _)| position)
}

/// Ids present in every list, file-name hits first (by how many tokens hit
/// the file name), then by id, so a truncated search keeps the strongest rows
/// and stays deterministic.
fn rank_ids(lists: &[&std::collections::BTreeMap<i64, bool>]) -> Vec<i64> {
  let Some((first, rest)) = lists.split_first() else {
    return Vec::new();
  };
  let mut ranked: Vec<(usize, i64)> = Vec::new();
  for (&id, &name_hit) in first.iter() {
    let mut name_hits = usize::from(name_hit);
    let mut in_every_list = true;
    for other in rest {
      match other.get(&id) {
        Some(&hit) => name_hits += usize::from(hit),
        None => {
          in_every_list = false;
          break;
        }
      }
    }
    if in_every_list {
      ranked.push((name_hits, id));
    }
  }
  ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
  ranked.into_iter().map(|(_, id)| id).collect()
}

#[derive(Debug)]
struct CandidateRow {
  path: PathBuf,
  kind: EntryKind,
  root_priority: i32,
  byte_size: Option<u64>,
  modified_unix_seconds: Option<u64>,
}

async fn upsert_root_tx(tx: &engine::Transaction<'_>, root: &SearchRoot) -> Result<i64> {
  let now = unix_now();
  tx.execute(
    "INSERT INTO roots (path, enabled, priority, include_hidden, include_ignored, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(path) DO UPDATE SET
           enabled = excluded.enabled,
           priority = excluded.priority,
           include_hidden = excluded.include_hidden,
           include_ignored = excluded.include_ignored,
           updated_at = excluded.updated_at",
    params![
      root.path.to_string_lossy(),
      root.enabled,
      root.priority,
      root.include_hidden,
      root.include_ignored,
      now
    ],
  )
  .await?;
  let mut rows = tx
    .query(
      "SELECT id FROM roots WHERE path = ?1",
      params![root.path.to_string_lossy()],
    )
    .await?;
  let row = rows.next().await?.expect("root row exists after upsert");
  Ok(row.get::<i64>(0)?)
}

async fn insert_entry_tx(
  tx: &engine::Transaction<'_>,
  root_id: i64,
  entry: &kuntu_crawler::CrawledEntry,
  gen: i64,
) -> Result<(i64, bool)> {
  let flags = i64::from(entry.decision.hidden)
    | (i64::from(entry.decision.ignored) * 2)
    | (i64::from(entry.decision.sensitive) * 4);
  tx.execute("INSERT INTO entries(root_id,path,name,extension,kind,size,mtime,flags,scan_gen) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(root_id,path) DO UPDATE SET name=excluded.name,extension=excluded.extension,kind=excluded.kind,size=excluded.size,mtime=excluded.mtime,flags=excluded.flags,scan_gen=excluded.scan_gen",params![root_id,entry.path.to_string_lossy(),entry.path.file_name().and_then(|v|v.to_str()).unwrap_or_default(),entry.path.extension().and_then(|v|v.to_str()).map(str::to_ascii_lowercase),kind_to_int(entry.kind),entry.size.and_then(|v|i64::try_from(v).ok()),entry.mtime,flags,gen]).await?;
  let id = tx.conn.raw.query_row(
    "SELECT id FROM entries WHERE root_id=?1 AND path=?2",
    params![root_id, entry.path.to_string_lossy()],
    |r| r.get(0),
  )?;
  tx.execute(&postings_delete_sql(true), params![id, 1])
    .await?;
  tx.execute(&postings_delete_sql(false), params![id, 2])
    .await?;
  Ok((id, true))
}
async fn insert_terms_tx(tx: &engine::Transaction<'_>, entry_id: i64, path: &Path) -> Result<()> {
  let name = path
    .file_name()
    .and_then(|v| v.to_str())
    .unwrap_or_default();
  let names: Vec<_> = tokenize(name)
    .into_iter()
    .chain(
      path
        .extension()
        .and_then(|v| v.to_str())
        .map(str::to_ascii_lowercase),
    )
    .collect();
  let paths: Vec<_> = path
    .parent()
    .into_iter()
    .flat_map(Path::components)
    .filter_map(|v| v.as_os_str().to_str())
    .flat_map(tokenize)
    .collect();
  for term in &names {
    tx.execute(
      "INSERT OR IGNORE INTO name_postings(term,entry_id) VALUES(?1,?2)",
      params![term, entry_id],
    )
    .await?;
  }
  for term in &paths {
    tx.execute(
      "INSERT OR IGNORE INTO path_postings(term,entry_id) VALUES(?1,?2)",
      params![term, entry_id],
    )
    .await?;
  }
  tx.execute("INSERT INTO reverse_postings(entry_id,encoded) VALUES(?1,?2) ON CONFLICT(entry_id) DO UPDATE SET encoded=excluded.encoded",params![entry_id,encode_terms(&names,&paths)?]).await?;
  Ok(())
}
async fn count_entries_tx(tx: &engine::Transaction<'_>, root_id: i64) -> Result<i64> {
  Ok(tx.conn.raw.query_row(
    "SELECT count(*) FROM entries WHERE root_id=?1",
    params![root_id],
    |r| r.get(0),
  )?)
}
fn placeholders(count: usize) -> String {
  std::iter::repeat_n("?", count)
    .collect::<Vec<_>>()
    .join(",")
}

/// Range predicate for one query token: every term that starts with `term`.
/// Terms are compared bytewise, so the exclusive upper bound is the prefix with
/// its last ASCII byte incremented, or, for anything non-ASCII, the prefix
/// followed by U+10FFFF (a noncharacter that never occurs in a term). A range
/// is a seek on the covering primary-key index; `LIKE` is a scan of the table.
#[cfg(test)]
fn term_prefix_clause(term: &str) -> (String, Vec<Value>) {
  (
    "term >= ? AND term < ?".to_string(),
    vec![
      Value::from(term.to_string()),
      Value::from(term_prefix_upper_bound(term)),
    ],
  )
}

fn term_prefix_upper_bound(prefix: &str) -> String {
  ascii_prefix_upper_bound(prefix).unwrap_or_else(|| format!("{prefix}\u{10FFFF}"))
}
fn ascii_prefix_upper_bound(prefix: &str) -> Option<String> {
  if prefix.is_empty() || !prefix.is_ascii() {
    return None;
  }
  let mut bytes = prefix.as_bytes().to_vec();
  for index in (0..bytes.len()).rev() {
    if bytes[index] < 0x7f {
      bytes[index] += 1;
      bytes.truncate(index + 1);
      return String::from_utf8(bytes).ok();
    }
  }
  None
}

async fn root_ids_for_config(conn: &engine::Connection, config: &SearchConfig) -> Result<Vec<i64>> {
  let mut ids = Vec::new();
  for root in config.roots.iter().filter(|root| root.enabled) {
    let mut rows = conn
      .query(
        "SELECT id FROM roots WHERE path = ?1",
        params![root.path.to_string_lossy()],
      )
      .await?;
    if let Some(row) = rows.next().await? {
      ids.push(row.get::<i64>(0)?);
    }
  }
  Ok(ids)
}

fn kind_to_int(kind: EntryKind) -> i64 {
  match kind {
    EntryKind::File => 1,
    EntryKind::Directory => 2,
    EntryKind::Other => 3,
  }
}

fn int_to_kind(value: i64) -> EntryKind {
  match value {
    1 => EntryKind::File,
    2 => EntryKind::Directory,
    _ => EntryKind::Other,
  }
}

fn unix_now() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .ok()
    .and_then(|duration| i64::try_from(duration.as_secs()).ok())
    .unwrap_or(0)
}

#[cfg(test)]
fn remove_database_files(path: &Path) {
  let raw = path.to_string_lossy().to_string();
  for candidate in [raw.clone(), format!("{raw}-wal"), format!("{raw}-shm")] {
    let _ = std::fs::remove_file(candidate);
  }
}

fn index_error_to_backend(error: IndexError) -> BackendError {
  BackendError::new(error.to_string())
}

pub fn match_names(matches: &[MatchKind]) -> Vec<&'static str> {
  matches
    .iter()
    .map(|kind| match kind {
      MatchKind::Empty => "Empty",
      MatchKind::ExactBasename => "ExactBasename",
      MatchKind::BasenamePrefix => "BasenamePrefix",
      MatchKind::BasenameToken => "BasenameToken",
      MatchKind::PathComponent => "PathComponent",
      MatchKind::Substring => "Substring",
      MatchKind::Fuzzy => "Fuzzy",
      MatchKind::Extension => "Extension",
      MatchKind::RootPriority => "RootPriority",
    })
    .collect()
}

const SCHEMA_DDL: &str = "
CREATE TABLE meta(key TEXT PRIMARY KEY,value NOT NULL) WITHOUT ROWID;
INSERT INTO meta VALUES('schema_epoch',3),('schema_minor',0),('min_reader_minor',0),('min_writer_minor',0),('generation',0),('policy_hash','1'),('store_id','private');
CREATE TABLE roots(id INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE,enabled INTEGER NOT NULL,priority INTEGER NOT NULL DEFAULT 0,include_hidden INTEGER NOT NULL DEFAULT 0,include_ignored INTEGER NOT NULL DEFAULT 0,updated_at INTEGER NOT NULL);
CREATE TABLE root_consumers(root_id INTEGER REFERENCES roots(id),consumer TEXT,priority INTEGER NOT NULL,include_hidden INTEGER NOT NULL,include_ignored INTEGER NOT NULL,enabled INTEGER NOT NULL,PRIMARY KEY(root_id,consumer)) WITHOUT ROWID;
CREATE TABLE entries(id INTEGER PRIMARY KEY,root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,path TEXT NOT NULL,name TEXT NOT NULL,extension TEXT,kind INTEGER NOT NULL,size INTEGER,mtime INTEGER,flags INTEGER NOT NULL DEFAULT 0,scan_gen INTEGER NOT NULL DEFAULT 0,UNIQUE(root_id,path));
CREATE TABLE name_postings(term TEXT,entry_id INTEGER,PRIMARY KEY(term,entry_id)) WITHOUT ROWID;
CREATE TABLE path_postings(term TEXT,entry_id INTEGER,PRIMARY KEY(term,entry_id)) WITHOUT ROWID;
CREATE TABLE reverse_postings(entry_id INTEGER PRIMARY KEY REFERENCES entries(id) ON DELETE CASCADE,encoded BLOB NOT NULL);
CREATE TABLE root_state(root_id INTEGER PRIMARY KEY REFERENCES roots(id),generation INTEGER NOT NULL DEFAULT 0,active_scan_gen INTEGER NOT NULL DEFAULT 0,status TEXT NOT NULL DEFAULT 'dirty',last_full_scan_at INTEGER,last_incremental_at INTEGER,dirty INTEGER NOT NULL DEFAULT 1,entry_count INTEGER NOT NULL DEFAULT 0,policy_revision INTEGER NOT NULL DEFAULT 0,scan_policy_revision INTEGER NOT NULL DEFAULT 0);
CREATE TABLE watch_state(root_id INTEGER PRIMARY KEY,backend TEXT,volume_uuid TEXT,cursor TEXT,updated_at INTEGER);
CREATE TABLE dirty_paths(root_id INTEGER,path TEXT,reason TEXT,queued_at INTEGER,PRIMARY KEY(root_id,path)) WITHOUT ROWID;
CREATE TABLE requests(id INTEGER PRIMARY KEY,kind TEXT NOT NULL,payload TEXT NOT NULL,created_by TEXT NOT NULL,created_at INTEGER NOT NULL,state TEXT NOT NULL DEFAULT 'pending');
CREATE TABLE writers(pid INTEGER PRIMARY KEY,version TEXT,role TEXT,heartbeat_at INTEGER);
PRAGMA user_version=3;
";

#[cfg(test)]
mod fixture;

#[cfg(test)]
mod tests {
  use std::fs;

  use kuntu_crawler::remove_dir_all_if_exists;

  use super::*;

  fn temp_dir(name: &str) -> PathBuf {
    let nonce = unix_now();
    std::env::temp_dir().join(format!("kuntu-index-{name}-{nonce}-{}", std::process::id()))
  }

  #[test]
  fn rebuild_indexes_allowed_files_and_searches_terms() {
    let root = temp_dir("rebuild");
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    fs::write(root.join("docs/auth-providers.md"), "auth\n").unwrap();
    fs::write(root.join("node_modules/pkg/auth-providers.md"), "ignored\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    let stats = index.rebuild(&config).unwrap();
    let results = index
      .search(&config, &SearchQuery::new("auth providers"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert!(stats.entries >= 2);
    assert!(stats.skipped >= 1);
    assert_eq!(results.len(), 1);
    assert!(results[0].path.ends_with("docs/auth-providers.md"));
    assert_eq!(results[0].provider, "sqlite");
  }

  #[test]
  fn rebuild_preserves_existing_entries_and_marks_dirty_when_crawl_errors() {
    let root = temp_dir("rebuild-error");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("preserved-file.md"), "old\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    let stats = index.rebuild(&config).unwrap();
    let status = index.status().unwrap();
    let results = index
      .search(&config, &SearchQuery::new("preserved file"))
      .unwrap();

    assert!(!stats.errors.is_empty());
    assert_eq!(status.len(), 1);
    assert!(status[0].dirty);
    assert_eq!(results.len(), 1);
  }

  #[test]
  fn status_reports_rebuilt_root() {
    let root = temp_dir("status");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    let expected_root = config.roots[0].path.clone();
    index.rebuild(&config).unwrap();
    let status = index.status().unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(status.len(), 1);
    assert_eq!(status[0].path, expected_root);
    assert!(status[0].entry_count >= 1);
    assert!(!status[0].dirty);
  }

  #[test]
  fn refresh_indexes_added_files_and_marks_missing_files_deleted() {
    let root = temp_dir("refresh");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("old-file.md"), "old\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    fs::remove_file(root.join("old-file.md")).unwrap();
    fs::write(root.join("new-file.md"), "new\n").unwrap();

    let stats = index.refresh(&config).unwrap();
    let old_results = index
      .search(&config, &SearchQuery::new("old file"))
      .unwrap();
    let new_results = index
      .search(&config, &SearchQuery::new("new file"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert!(stats.inserted >= 1);
    assert!(stats.deleted >= 1);
    assert!(old_results.is_empty());
    assert_eq!(new_results.len(), 1);
  }

  #[test]
  fn refresh_preserves_existing_entries_and_marks_dirty_when_crawl_errors() {
    let root = temp_dir("refresh-error");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("still-indexed.md"), "old\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    let stats = index.refresh(&config).unwrap();
    let status = index.status().unwrap();
    let results = index
      .search(&config, &SearchQuery::new("still indexed"))
      .unwrap();

    assert!(!stats.errors.is_empty());
    assert_eq!(stats.deleted, 0);
    assert_eq!(status.len(), 1);
    assert!(status[0].dirty);
    assert_eq!(results.len(), 1);
  }

  #[test]
  fn refresh_updates_changed_file_metadata() {
    let root = temp_dir("refresh-update");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("same-file.md"), "a\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    fs::write(root.join("same-file.md"), "longer content\n").unwrap();

    let stats = index.refresh(&config).unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert!(stats.updated >= 1);
  }

  #[test]
  fn repair_refreshes_dirty_roots() {
    let root = temp_dir("repair");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("repair-file.md"), "repair\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    index.mark_root_dirty(&config.roots[0]).unwrap();
    assert!(index.status().unwrap()[0].dirty);

    let stats = index.repair(&config).unwrap();
    let status = index.status().unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(stats.dirty_roots, 1);
    assert_eq!(stats.repaired_roots, 1);
    assert!(!status[0].dirty);
  }

  #[test]
  fn search_with_metrics_reports_narrowed_candidate_count() {
    let root = temp_dir("metrics");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("needleunique-target.md"), "needle\n").unwrap();
    fs::write(root.join("other-target.md"), "other\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    let outcome = index
      .search_with_metrics(&config, &SearchQuery::new("needleunique"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 1);
    assert_eq!(outcome.results.len(), 1);
    assert!(outcome.results[0].path.ends_with("needleunique-target.md"));
  }

  #[test]
  fn compact_single_token_miss_returns_empty_complete_without_root_scan() {
    // Plan 0062 Task 1: the full-root fallback is gone. A compacted query
    // whose token matches no indexed term is a miss, not a scan; ordered
    // fuzzy matching is no longer promised by the index.
    let root = temp_dir("substring");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "lib\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    let outcome = index
      .search_with_metrics(&config, &SearchQuery::new("srclib"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 0);
    assert_eq!(outcome.results, Vec::new());
    assert!(!outcome.truncated);
    assert_eq!(outcome.coverage, CandidateCoverage::Complete);
  }

  #[test]
  fn compacted_ordered_queries_across_tokens_are_empty_complete() {
    // Same no-fallback contract for the ordered-fuzzy shape: "awspdf" is one
    // token that matches no indexed term, so the answer is empty and complete.
    let root = temp_dir("ordered-fuzzy");
    fs::create_dir_all(&root).unwrap();
    fs::write(
      root.join("AWS Certified Solutions Architect Associate SAA-C03.pdf"),
      "pdf\n",
    )
    .unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    let outcome = index
      .search_with_metrics(&config, &SearchQuery::new("awspdf"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 0);
    assert_eq!(outcome.results, Vec::new());
    assert_eq!(outcome.coverage, CandidateCoverage::Complete);
  }

  /// Plant one entry (and its basename terms) directly, bypassing the
  /// crawler: the point is that stale or hostile rows already in the store
  /// cannot leak through search.
  fn plant_entry(
    index: &KuntuIndex,
    root: &Path,
    relative: &str,
    kind_int: i64,
    policy_flags: (bool, bool, bool),
    terms: &[&str],
  ) {
    let (hidden, ignored, sensitive) = policy_flags;
    let root = kuntu_core::normalize_root_path(root.to_path_buf());
    let full = root.join(relative);
    let name = full.file_name().unwrap().to_string_lossy().to_string();
    let extension = name
      .rsplit_once('.')
      .map(|(_, extension)| extension.to_ascii_lowercase());
    block_on(async {
      let root_id: i64 = {
        let mut rows = index
          .conn
          .query(
            "SELECT id FROM roots WHERE path = ?1",
            params![root.to_string_lossy()],
          )
          .await
          .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
      };
      index
        .conn
        .execute(
          "INSERT INTO entries (root_id,path,name,extension,kind,size,flags) VALUES(?1,?2,?3,?4,?5,4,?6)",
          params![
            root_id,
            full.to_string_lossy(),
            name,
            extension,
            kind_int,
            i64::from(hidden)|(i64::from(ignored)*2)|(i64::from(sensitive)*4),
          ],
        )
        .await
        .unwrap();
      let entry_id: i64 = {
        let mut rows = index
          .conn
          .query(
            "SELECT id FROM entries WHERE root_id = ?1 AND path = ?2",
            params![root_id, full.to_string_lossy()],
          )
          .await
          .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
      };
      index
        .conn
        .raw
        .execute(
          "INSERT INTO reverse_postings(entry_id,encoded) VALUES(?1,?2)",
          params![
            entry_id,
            encode_terms(
              &terms.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
              &[]
            )
            .unwrap()
          ],
        )
        .unwrap();
      for term in terms {
        index
          .conn
          .execute(
            "INSERT OR IGNORE INTO name_postings(term,entry_id) VALUES (?1,?2)",
            params![term, entry_id],
          )
          .await
          .unwrap();
      }
    });
  }

  #[test]
  fn search_default_query_excludes_hidden_ignored_and_sensitive_rows() {
    let root = temp_dir("policy-search");
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/plan-report.md"), "doc\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();

    // Plant policy-forbidden rows that outrank the legitimate hit: same
    // basename (same score) on lexically earlier paths, so a post-window
    // filter would visibly return them before the allowed file.
    plant_entry(
      &index,
      &root,
      ".cached/plan-report.md",
      1,
      (true, false, false),
      &["plan", "report"],
    );
    plant_entry(
      &index,
      &root,
      "ignored_dir/plan-report.md",
      1,
      (false, true, false),
      &["plan", "report"],
    );
    plant_entry(
      &index,
      &root,
      ".ssh/plan-report.md",
      1,
      (false, false, true),
      &["plan", "report"],
    );

    let outcome = index
      .search_with_metrics(&config, &SearchQuery::new("plan-report").with_limit(1))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 1);
    assert_eq!(outcome.results.len(), 1);
    assert!(outcome.results[0].path.ends_with("docs/plan-report.md"));
  }

  #[test]
  fn default_query_still_sees_hidden_rows_of_a_hidden_allowing_root() {
    let root = temp_dir("root-allowance");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("open-plan.md"), "a\n").unwrap();
    fs::write(root.join(".hidden-plan.md"), "b\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root).include_hidden(true)],
    };
    index.rebuild(&config).unwrap();
    let outcome = index
      .search_with_metrics(&config, &SearchQuery::new("plan").with_limit(10))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(
      outcome.results.len(),
      2,
      "the root's allowance must survive the query flag"
    );
  }

  #[test]
  fn dotted_and_bare_extension_filters_agree() {
    let root = temp_dir("dotted-ext");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("plan-report.md"), "a\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();

    let mut dotted = SearchQuery::new("plan-report");
    dotted.extensions = vec![".md".to_string()];
    let dotted_outcome = index.search_with_metrics(&config, &dotted).unwrap();

    let mut bare = SearchQuery::new("plan-report");
    bare.extensions = vec!["md".to_string()];
    let bare_outcome = index.search_with_metrics(&config, &bare).unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(
      dotted_outcome.results.len(),
      1,
      "leading dot must be normalized"
    );
    assert_eq!(dotted_outcome.results, bare_outcome.results);
  }

  #[test]
  fn search_scope_confines_candidates_to_configured_roots() {
    let root_a = temp_dir("scope-a");
    let root_b = temp_dir("scope-b");
    fs::create_dir_all(root_a.join("docs")).unwrap();
    fs::create_dir_all(&root_b).unwrap();
    let root_a = kuntu_core::normalize_root_path(root_a);
    let root_b = kuntu_core::normalize_root_path(root_b);
    fs::write(root_a.join("docs/plan-report.md"), "a\n").unwrap();
    // Root B holds the stronger match: exact basename "plan".
    fs::write(root_b.join("plan.md"), "b\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let index_config = SearchConfig {
      roots: vec![SearchRoot::new(&root_a), SearchRoot::new(&root_b)],
    };
    index.rebuild(&index_config).unwrap();
    let query_config = SearchConfig {
      roots: vec![SearchRoot::new(&root_a)],
    };
    let outcome = index
      .search_with_metrics(&query_config, &SearchQuery::new("plan").with_limit(10))
      .unwrap();
    remove_dir_all_if_exists(&root_a).unwrap();
    remove_dir_all_if_exists(&root_b).unwrap();

    assert_eq!(outcome.candidate_count, 1);
    assert_eq!(outcome.results.len(), 1);
    assert!(outcome.results[0].path.starts_with(&root_a));
  }

  #[test]
  fn files_only_query_returns_file_behind_directory_window() {
    let root = temp_dir("kind-window");
    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();

    // 101 directories matching at the same rank as the file, planted so the
    // file sorts behind all of them under the old post-window behaviour.
    for index_number in 0..101 {
      plant_entry(
        &index,
        &root,
        &format!("plan-dir-{index_number:03}"),
        2,
        (false, false, false),
        &["plan"],
      );
    }
    plant_entry(
      &index,
      &root,
      "plan-file.md",
      1,
      (false, false, false),
      &["plan"],
    );

    let files_only = SearchQuery {
      include_directories: false,
      ..SearchQuery::new("plan").with_limit(10)
    };
    let outcome = index.search_with_metrics(&config, &files_only).unwrap();
    assert_eq!(outcome.results.len(), 1);
    assert!(outcome.results[0].path.ends_with("plan-file.md"));

    let unfiltered = SearchQuery::new("plan").with_limit(5);
    let outcome = index.search_with_metrics(&config, &unfiltered).unwrap();
    assert_eq!(outcome.results.len(), 5);
    remove_dir_all_if_exists(&root).unwrap();
  }

  #[test]
  fn no_term_hit_returns_empty_complete_without_scanning_the_root() {
    let root = temp_dir("no-term");
    fs::create_dir_all(&root).unwrap();
    for index_number in 0..50 {
      fs::write(root.join(format!("document-{index_number:02}.txt")), "x\n").unwrap();
    }

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    let outcome = index
      .search_with_metrics(&config, &SearchQuery::new("zzqqx"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 0);
    assert_eq!(outcome.results, Vec::new());
    assert!(!outcome.truncated);
    assert_eq!(outcome.coverage, CandidateCoverage::Complete);
  }

  fn run_bounded_fixture(name: &str, rows: i64, miss_ceiling_secs: u64, broad_ceiling_secs: u64) {
    let root = temp_dir(name);
    fs::create_dir_all(&root).unwrap();
    let root = kuntu_core::normalize_root_path(root);
    // File-backed on purpose: an in-memory turso store costs kilobytes per
    // row, and a fixture at this scale is exactly where that shows. The
    // database lives outside the crawled root so the rebuild does not index
    // the fixture's own store.
    let db_dir = temp_dir(&format!("{name}-db"));
    fs::create_dir_all(&db_dir).unwrap();
    let db_path = db_dir.join("fixture.sqlite");
    let mut index = KuntuIndex::open(&db_path).unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();

    let total_rows: i64 = rows;
    // Values are literals generated by this test, so large multi-row
    // statements keep the fixture to a few hundred round-trips.
    let batch_rows: i64 = 200;
    block_on(async {
      let tx = index.conn.transaction().await.unwrap();
      let root_id: i64 = {
        let mut rows = tx
          .query(
            "SELECT id FROM roots WHERE path = ?1",
            params![root.to_string_lossy()],
          )
          .await
          .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
      };
      // The rebuild indexed the (empty) root directory itself; the fixture
      // owns the whole entries table.
      tx.execute("DELETE FROM entries", []).await.unwrap();
      let mut batch_start: i64 = 0;
      while batch_start < total_rows {
        let batch_end = (batch_start + batch_rows).min(total_rows);
        let mut entries_sql =
          String::from("INSERT INTO entries(id,root_id,path,name,extension,kind,size) VALUES ");
        let mut terms_sql =
          String::from("INSERT OR IGNORE INTO name_postings(term,entry_id) VALUES ");
        for ordinal in batch_start..batch_end {
          // Row ids start at 1: turso treats an explicit 0 as "assign one".
          let entry_id = ordinal + 1;
          if ordinal > batch_start {
            entries_sql.push(',');
            terms_sql.push(',');
          }
          let path = format!("dir{}/batchfile-{ordinal}.dat", ordinal % 997);
          let name = format!("batchfile-{ordinal}.dat");
          entries_sql.push_str(&format!(
            "({entry_id}, {root_id}, '{path}', '{name}', 'dat', 1, 8)"
          ));
          terms_sql.push_str(&format!("('batchfile', {entry_id})"));
        }
        tx.execute(&entries_sql, []).await.unwrap();
        tx.execute(&terms_sql, []).await.unwrap();
        batch_start = batch_end;
      }
      tx.commit().await.unwrap();
    });

    let miss_started = std::time::Instant::now();
    let miss = index
      .search_with_metrics(&config, &SearchQuery::new("zzqqx"))
      .unwrap();
    let miss_elapsed = miss_started.elapsed();
    assert_eq!(miss.candidate_count, 0);
    assert_eq!(miss.results, Vec::new());
    assert_eq!(miss.coverage, CandidateCoverage::Complete);

    let broad_started = std::time::Instant::now();
    let broad = index
      .search_with_options(
        &config,
        &SearchQuery::new("batchfile").with_limit(100),
        SearchOptions {
          fetch_budget: MAX_KFS_CANDIDATE_ROWS,
        },
      )
      .unwrap();
    let broad_elapsed = broad_started.elapsed();

    assert_eq!(broad.candidate_count, MAX_KFS_CANDIDATE_ROWS);
    assert_eq!(broad.coverage, CandidateCoverage::Truncated);
    assert!(broad.truncated);
    assert_eq!(broad.results.len(), 100);
    // Observed ceilings are recorded in the release notes; these guards only
    // catch a regression back to the unbounded full-root fallback.
    assert!(
      miss_elapsed.as_secs() < miss_ceiling_secs,
      "empty miss must not scan the root; took {miss_elapsed:?}"
    );
    assert!(
      broad_elapsed.as_secs() < broad_ceiling_secs,
      "bounded broad query took {broad_elapsed:?}"
    );
    remove_database_files(&db_path);
    let _ = remove_dir_all_if_exists(&db_dir);
    let _ = remove_dir_all_if_exists(&root);
  }

  /// Quick-scale guard: still larger than the 50,000-candidate work bound so
  /// the broad query must report truncation. Ignored because a debug-profile
  /// turso costs ~1.5 ms per joined candidate row; run via `just benchmark`.
  #[test]
  #[ignore = "bounded-fixture benchmark; run in release mode via just benchmark"]
  fn broad_prefix_and_miss_stay_bounded_at_60k_rows() {
    run_bounded_fixture("bounded-60k", 60_000, 5, 60);
  }

  /// The single-term one-million-row benchmark: one posting list of 1,000,000
  /// rows. The miss is a seek (about 1 ms) and the wide-budget broad query
  /// loads exactly `MAX_KFS_CANDIDATE_ROWS` candidates. This fixture is
  /// pathological on purpose; the realistic 1M benchmark is
  /// `realistic_fixture_latency_gate` (`just benchmark-1m`). Debug-profile
  /// turso inserts grow superlinearly past ~200k rows, so this runs ignored;
  /// measure with
  /// `cargo test --release -p kuntu-index one_million -- --ignored --nocapture`.
  ///
  /// History: before v0.5.0 this test failed its broad ceiling (1,005 s) because
  /// the candidate fetch scanned the whole root once per batch. The README's
  /// earlier note blaming the terms range predicate (a "7.5 s miss") was wrong:
  /// the range was always a seek.
  #[test]
  #[ignore = "one-million-row benchmark; run in release mode"]
  fn one_million_row_root_stays_bounded_on_miss_and_broad_prefix() {
    // The broad ceiling absorbs the deterministic-window ORDER BY over a
    // pathological single-token store (1M entries sharing one term); real
    // stores spread tokens, and the 60k guard's 60 s covers that shape.
    run_bounded_fixture("million", 1_000_000, 30, 300);
  }

  #[test]
  fn equal_scores_break_ties_by_path_deterministically() {
    let root_a = temp_dir("tie-a");
    let root_b = temp_dir("tie-b");
    fs::create_dir_all(&root_a).unwrap();
    fs::create_dir_all(&root_b).unwrap();
    fs::write(root_a.join("same-name.md"), "a\n").unwrap();
    fs::write(root_b.join("same-name.md"), "b\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root_a), SearchRoot::new(&root_b)],
    };
    index.rebuild(&config).unwrap();

    let first = index
      .search_with_metrics(&config, &SearchQuery::new("same-name").with_limit(10))
      .unwrap();
    let second = index
      .search_with_metrics(&config, &SearchQuery::new("same-name").with_limit(10))
      .unwrap();
    remove_dir_all_if_exists(&root_a).unwrap();
    remove_dir_all_if_exists(&root_b).unwrap();

    let first_paths: Vec<_> = first.results.iter().map(|r| r.path.clone()).collect();
    let second_paths: Vec<_> = second.results.iter().map(|r| r.path.clone()).collect();
    assert_eq!(first_paths, second_paths);
    let mut sorted = first_paths.clone();
    sorted.sort();
    assert_eq!(first_paths, sorted, "equal scores tie-break by path");
  }

  #[test]
  fn non_ascii_names_rank_deterministically() {
    let root = temp_dir("non-ascii");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("计划书A.md"), "a\n").unwrap();
    fs::write(root.join("计划书B.md"), "b\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();

    let first = index
      .search_with_metrics(&config, &SearchQuery::new("计划").with_limit(10))
      .unwrap();
    let second = index
      .search_with_metrics(&config, &SearchQuery::new("计划").with_limit(10))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(first.results.len(), 2);
    let first_paths: Vec<_> = first.results.iter().map(|r| r.path.clone()).collect();
    let second_paths: Vec<_> = second.results.iter().map(|r| r.path.clone()).collect();
    assert_eq!(first_paths, second_paths);
  }

  #[test]
  fn ascii_prefix_upper_bound_advances_last_ascii_byte() {
    assert_eq!(ascii_prefix_upper_bound("abc"), Some("abd".to_string()));
    assert_eq!(ascii_prefix_upper_bound("abz"), Some("ab{".to_string()));
    assert_eq!(ascii_prefix_upper_bound("é"), None);
  }

  #[test]
  fn open_memory_stamps_current_schema_version() {
    let index = KuntuIndex::open_memory().unwrap();
    assert_eq!(block_on(index.schema_version()).unwrap(), SCHEMA_VERSION);
  }

  #[test]
  fn deleting_entries_cascades_to_terms() {
    // The rebuild's bulk `DELETE FROM entries` relies on the terms foreign
    // key; without the pragma on, row id reuse aliases stale terms onto new
    // files and searches return unrelated results.
    let root = temp_dir("cascade");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("cascade-target.md"), "cascade\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();

    let terms_before: i64 = block_on(async {
      let mut rows = index
        .conn
        .query("SELECT COUNT(*) FROM name_postings", [])
        .await
        .unwrap();
      rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    });
    assert!(terms_before > 0);

    block_on(async {
      index.conn.execute("DELETE FROM entries", []).await.unwrap();
    });

    let terms_after: i64 = block_on(async {
      let mut rows = index
        .conn
        .query("SELECT COUNT(*) FROM name_postings", [])
        .await
        .unwrap();
      rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    });
    assert_eq!(terms_after, 0);
    remove_dir_all_if_exists(&root).unwrap();
  }

  /// Release-mode latency gate on the generated realistic fixture (see
  /// `fixture.rs`). Rows come from `KUNTU_BENCH_ROWS` (default 100,000; the
  /// owner runs 1,000,000 via `just benchmark-1m`), the cache directory from
  /// `KUNTU_BENCH_DIR`. Each query class spends one untimed warm-up run and
  /// then compares the best of `fixture::TIMED_RUNS` measured runs with its
  /// ceiling, so a cold page cache does not fail the gate.
  ///
  /// Ceilings are the measured warm numbers of the fixed implementation at
  /// 1,000,000 rows (turso 0.7.2, M4 Pro, external disk) with generous headroom
  /// for a slow disk and a loaded machine. They are regression guards, not
  /// targets. The unfixed implementation fails the non-ASCII miss (full terms
  /// scan) and then the broad classes (per-batch root scan) by orders of
  /// magnitude.
  #[test]
  #[ignore = "latency gate on a generated fixture; run in release mode via just benchmark"]
  fn realistic_fixture_latency_gate() {
    let rows: usize = std::env::var("KUNTU_BENCH_ROWS")
      .ok()
      .and_then(|value| value.parse().ok())
      .unwrap_or(100_000);
    let fixture = fixture::open_or_build(rows);
    eprintln!(
      "GATE fixture rows={} bytes={} build_secs={:?}",
      fixture.rows, fixture.db_bytes, fixture.build_secs
    );
    let queries = &fixture.queries;
    // (class, query text, ceiling in milliseconds)
    let classes: Vec<(&str, String, u128)> = vec![
      ("miss", "zzqqx".to_string(), 25),
      ("miss_non_ascii", "zzqqé".to_string(), 25),
      ("rare", queries.rare.clone(), 50),
      ("mid", queries.mid.clone(), 100),
      ("broad", queries.broad.clone(), 400),
      ("prefix3", queries.prefix3.clone(), 400),
      ("mixed", queries.mixed.clone(), 300),
      ("all_rows", queries.all_rows.clone(), 600),
      ("prefix1", queries.prefix1.clone(), 800),
    ];
    for (class, text, ceiling_ms) in classes {
      let query = SearchQuery::new(text).with_limit(20);
      let runs = std::cell::Cell::new(0usize);
      let mut outcome = None;
      // One untimed warm-up run, then the measured runs; none of them is
      // abandoned early, so the cold first run can never decide the gate.
      let best = fixture::measure_warm_best(|| {
        let started = std::time::Instant::now();
        let current = fixture
          .index
          .search_with_metrics(&fixture.config, &query)
          .unwrap();
        let elapsed = started.elapsed();
        let run = runs.get();
        runs.set(run + 1);
        eprintln!(
          "GATE rows={rows} class={class} run={run}{} ms={:.1} candidates={} results={} coverage={:?}",
          if run == 0 { " (warm-up, not gated)" } else { "" },
          elapsed.as_secs_f64() * 1000.0,
          current.candidate_count,
          current.results.len(),
          current.coverage
        );
        outcome = Some(current);
        elapsed
      });
      let outcome = outcome.unwrap();
      match class {
        "miss" | "miss_non_ascii" => {
          assert_eq!(outcome.candidate_count, 0, "{class}");
          assert_eq!(outcome.coverage, CandidateCoverage::Complete, "{class}");
        }
        "rare" => assert!(outcome.candidate_count >= 1, "{class}"),
        "mid" => assert!(outcome.candidate_count >= 10, "{class}"),
        "broad" | "prefix3" => assert!(outcome.candidate_count >= 1_000, "{class}"),
        "mixed" => {
          // Mixed selectivity must be exact: every row with both tokens.
          assert_eq!(outcome.candidate_count, queries.mixed_truth, "{class}");
          assert_eq!(outcome.coverage, CandidateCoverage::Complete, "{class}");
        }
        "all_rows" | "prefix1" => {
          assert_eq!(outcome.coverage, CandidateCoverage::Truncated, "{class}")
        }
        other => unreachable!("{other}"),
      }
      assert!(
        best.as_millis() <= ceiling_ms,
        "{class}: best of {} warm runs took {best:?}, ceiling {ceiling_ms} ms",
        fixture::TIMED_RUNS
      );
    }
  }

  #[test]
  fn non_ascii_prefixes_use_a_range_not_like() {
    for term in ["é", "café", "计划", "zzqqé"] {
      let (clause, values) = term_prefix_clause(term);
      assert!(!clause.contains("LIKE"), "{term}: {clause}");
      assert_eq!(values.len(), 2, "{term}");
    }
  }

  #[test]
  fn non_ascii_tokens_are_found_by_prefix_and_misses_are_empty() {
    let root = temp_dir("non-ascii-prefix");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("café-menu.txt"), "a\n").unwrap();
    fs::write(root.join("计划书A.md"), "b\n").unwrap();
    fs::write(root.join("plain.txt"), "c\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    for (query, expected) in [
      ("caf", "café-menu.txt"),
      ("café", "café-menu.txt"),
      ("计", "计划书A.md"),
      ("计划", "计划书A.md"),
    ] {
      let outcome = index
        .search_with_metrics(&config, &SearchQuery::new(query))
        .unwrap();
      assert_eq!(outcome.results.len(), 1, "{query}");
      assert!(outcome.results[0].path.ends_with(expected), "{query}");
    }
    let miss = index
      .search_with_metrics(&config, &SearchQuery::new("zzqqé"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(miss.candidate_count, 0);
    assert_eq!(miss.coverage, CandidateCoverage::Complete);
  }

  #[test]
  fn entry_fetch_is_a_rowid_seek_not_an_index_scan() {
    let index = KuntuIndex::open_memory().unwrap();
    let sql = format!("EXPLAIN QUERY PLAN {}", entry_fetch_sql(3));
    let details = block_on(async {
      let mut rows = index
        .conn
        .query(
          &sql,
          params_from_iter(vec![
            Value::from(1i64),
            Value::from(2i64),
            Value::from(3i64),
          ]),
        )
        .await
        .unwrap();
      let mut details = Vec::new();
      while let Some(row) = rows.next().await.unwrap() {
        details.push(row.get::<String>(3).unwrap());
      }
      details
    });
    let plan = details.join(" | ");
    assert!(plan.contains("rowid=?"), "plan: {plan}");
    assert!(!plan.contains("autoindex"), "plan: {plan}");
  }

  #[test]
  fn default_fetch_budget_is_five_thousand() {
    assert_eq!(DEFAULT_FETCH_BUDGET, 5_000);
    assert_eq!(SearchOptions::default().fetch_budget, 5_000);
  }

  #[test]
  fn fetch_budget_counts_admitted_candidates_not_ids() {
    // 120 directories hold the lowest ids, then 30 files. A files-only search
    // with a budget of 10 must still return files: the budget counts admitted
    // rows, it is not a window over raw ids.
    let root = temp_dir("budget-window");
    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    for number in 0..120 {
      plant_entry(
        &index,
        &root,
        &format!("plan-dir-{number:03}"),
        2,
        (false, false, false),
        &["plan"],
      );
    }
    for number in 0..30 {
      plant_entry(
        &index,
        &root,
        &format!("plan-file-{number:02}.md"),
        1,
        (false, false, false),
        &["plan"],
      );
    }
    let files_only = SearchQuery {
      include_directories: false,
      ..SearchQuery::new("plan").with_limit(100)
    };

    let tight = index
      .search_with_options(&config, &files_only, SearchOptions { fetch_budget: 10 })
      .unwrap();
    assert_eq!(tight.candidate_count, 10);
    assert_eq!(tight.results.len(), 10);
    assert!(tight.results.iter().all(|r| r.kind == EntryKind::File));
    assert_eq!(tight.coverage, CandidateCoverage::Truncated);

    let exact = index
      .search_with_options(&config, &files_only, SearchOptions { fetch_budget: 30 })
      .unwrap();
    assert_eq!(exact.candidate_count, 30);
    assert_eq!(exact.coverage, CandidateCoverage::Complete);

    let roomy = index.search_with_metrics(&config, &files_only).unwrap();
    assert_eq!(roomy.candidate_count, 30);
    assert_eq!(roomy.coverage, CandidateCoverage::Complete);
    remove_dir_all_if_exists(&root).unwrap();
  }

  #[test]
  fn policy_filters_apply_before_the_budget_not_after() {
    // Forbidden rows (hidden, ignored, sensitive) take the lowest ids. With a
    // budget of 2 the two allowed files behind them must still be returned.
    let root = temp_dir("budget-policy");
    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    for number in 0..6 {
      let flags = match number % 3 {
        0 => (true, false, false),
        1 => (false, true, false),
        _ => (false, false, true),
      };
      plant_entry(
        &index,
        &root,
        &format!("hidden-{number}/plan-secret-{number}.md"),
        1,
        flags,
        &["plan"],
      );
    }
    for number in 0..2 {
      plant_entry(
        &index,
        &root,
        &format!("docs/plan-ok-{number}.md"),
        1,
        (false, false, false),
        &["plan"],
      );
    }
    let outcome = index
      .search_with_options(
        &config,
        &SearchQuery::new("plan").with_limit(10),
        SearchOptions { fetch_budget: 2 },
      )
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 2);
    assert!(outcome
      .results
      .iter()
      .all(|r| r.path.to_string_lossy().contains("plan-ok-")));
  }

  #[test]
  fn fetch_budget_is_clamped_to_a_sane_range() {
    let root = temp_dir("budget-clamp");
    fs::create_dir_all(&root).unwrap();
    for name in ["plan-a.md", "plan-b.md", "plan-c.md"] {
      fs::write(root.join(name), "x\n").unwrap();
    }
    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    let query = SearchQuery::new("plan").with_limit(10);

    let zero = index
      .search_with_options(&config, &query, SearchOptions { fetch_budget: 0 })
      .unwrap();
    assert_eq!(zero.candidate_count, 1);
    assert_eq!(zero.coverage, CandidateCoverage::Truncated);

    let huge = index
      .search_with_options(
        &config,
        &query,
        SearchOptions {
          fetch_budget: usize::MAX,
        },
      )
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();
    assert_eq!(huge.candidate_count, 3);
    assert_eq!(huge.coverage, CandidateCoverage::Complete);
  }

  /// 60,000 file entries named `common-N.md`; the entries at `rare_positions`
  /// are named `rareword-common-N.md`. File-backed, planted in bulk.
  struct RecallFixture {
    index: KuntuIndex,
    config: SearchConfig,
    root: PathBuf,
    db_dir: PathBuf,
  }

  impl RecallFixture {
    fn cleanup(self) {
      let _ = remove_dir_all_if_exists(&self.db_dir);
      let _ = remove_dir_all_if_exists(&self.root);
    }
  }

  fn recall_fixture(name: &str, rare_positions: &[usize]) -> RecallFixture {
    let root = temp_dir(name);
    fs::create_dir_all(&root).unwrap();
    let root = kuntu_core::normalize_root_path(root);
    let db_dir = temp_dir(&format!("{name}-db"));
    fs::create_dir_all(&db_dir).unwrap();
    let mut index = KuntuIndex::open(db_dir.join("recall.sqlite")).unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    let root_id: i64 = block_on(async {
      let mut rows = index
        .conn
        .query(
          "SELECT id FROM roots WHERE path = ?1",
          params![root.to_string_lossy()],
        )
        .await
        .unwrap();
      rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    });
    block_on(async {
      index.conn.execute("DELETE FROM entries", []).await.unwrap();
    });
    let rare: HashSet<usize> = rare_positions.iter().copied().collect();
    let rows: Vec<fixture::RowSpec> = (0..60_000usize)
      .map(|number| {
        let file_name = if rare.contains(&number) {
          format!("rareword-common-{number}.md")
        } else {
          format!("common-{number}.md")
        };
        fixture::RowSpec {
          path: format!("{}/{file_name}", root.to_string_lossy()),
          is_dir: false,
        }
      })
      .collect();
    fixture::insert_rows(&mut index, root_id, &rows, 1);
    RecallFixture {
      index,
      config,
      root,
      db_dir,
    }
  }

  #[test]
  #[ignore = "60k-row fixture; run in release mode via just benchmark"]
  fn default_budget_bounds_a_broad_single_token() {
    let fixture = recall_fixture("budget-broad", &[]);
    let query = SearchQuery::new("common").with_limit(50);

    let outcome = fixture
      .index
      .search_with_metrics(&fixture.config, &query)
      .unwrap();
    assert_eq!(outcome.candidate_count, DEFAULT_FETCH_BUDGET);
    assert_eq!(outcome.coverage, CandidateCoverage::Truncated);
    assert_eq!(outcome.results.len(), 50);
    assert!(outcome.truncated);

    let wide = fixture
      .index
      .search_with_options(
        &fixture.config,
        &query,
        SearchOptions {
          fetch_budget: MAX_KFS_CANDIDATE_ROWS,
        },
      )
      .unwrap();
    assert_eq!(wide.candidate_count, MAX_KFS_CANDIDATE_ROWS);
    assert_eq!(wide.coverage, CandidateCoverage::Truncated);
    fixture.cleanup();
  }

  #[test]
  fn every_query_token_must_match_and_tokens_match_by_prefix() {
    let root = temp_dir("multi-token");
    fs::create_dir_all(root.join("alphabet")).unwrap();
    fs::write(root.join("alphabet/beta-notes.md"), "a\n").unwrap();
    fs::write(root.join("alphabet/gamma-notes.md"), "b\n").unwrap();
    fs::write(root.join("other.md"), "c\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    let both = index
      .search_with_metrics(&config, &SearchQuery::new("alph bet"))
      .unwrap();
    let one_missing = index
      .search_with_metrics(&config, &SearchQuery::new("alph delta"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(both.candidate_count, 1);
    assert!(both.results[0].path.ends_with("alphabet/beta-notes.md"));
    assert_eq!(both.coverage, CandidateCoverage::Complete);
    assert_eq!(one_missing.candidate_count, 0);
    assert_eq!(one_missing.coverage, CandidateCoverage::Complete);
  }

  /// Plant `relative` with `token` recorded as a directory-component hit only
  /// (field 3), as the indexer does for a token that appears only in the path.
  fn plant_path_only_entry(index: &KuntuIndex, root: &Path, relative: &str, token: &str) {
    plant_entry(index, root, relative, 1, (false, false, false), &[token]);
    let full = kuntu_core::normalize_root_path(root.to_path_buf()).join(relative);
    block_on(async {
      index
        .conn
        .execute(
          "INSERT OR IGNORE INTO path_postings SELECT term,entry_id FROM name_postings WHERE entry_id=(SELECT id FROM entries WHERE path=?1)",
          params![full.to_string_lossy()],
        )
        .await
        .unwrap();
      index
        .conn
        .execute(
          "DELETE FROM name_postings WHERE entry_id=(SELECT id FROM entries WHERE path=?1)",
          params![full.to_string_lossy()],
        )
        .await
        .unwrap();
    });
  }

  #[test]
  fn file_name_hits_are_fetched_before_path_only_hits_when_the_budget_binds() {
    // Five path-only matches take the lowest ids, five file-name matches come
    // after them. With a budget of 5 the file-name matches must win.
    let root = temp_dir("name-first");
    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    for number in 0..5 {
      plant_path_only_entry(
        &index,
        &root,
        &format!("tokenword-dir/aaa-{number}.md"),
        "tokenword",
      );
    }
    for number in 0..5 {
      plant_entry(
        &index,
        &root,
        &format!("zzz-tokenword-{number}.md"),
        1,
        (false, false, false),
        &["tokenword"],
      );
    }
    let outcome = index
      .search_with_options(
        &config,
        &SearchQuery::new("tokenword").with_limit(20),
        SearchOptions { fetch_budget: 5 },
      )
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 5);
    assert_eq!(outcome.coverage, CandidateCoverage::Truncated);
    assert!(outcome.results.iter().all(|result| result
      .path
      .file_name()
      .is_some_and(|name| name.to_string_lossy().starts_with("zzz-"))));
  }

  /// The multi-token recall bug: a selective token plus a common one. 60,000
  /// entries carry `common`; 33 also carry `rareword`. Eight of them have ids
  /// below the 50,000-id work bound, 25 above it. Capping each token's list
  /// and intersecting returns only the 8; driving from the rare token returns
  /// all 33 and stays `Complete`.
  #[test]
  #[ignore = "60k-row fixture; run in release mode via just benchmark"]
  fn rarest_token_drives_the_search_and_recall_is_exact() {
    let rare_positions: Vec<usize> = (0..8).chain(55_000..55_025).collect();
    let fixture = recall_fixture("recall", &rare_positions);
    for text in ["rareword common", "common rareword", "rare common"] {
      let outcome = fixture
        .index
        .search_with_metrics(&fixture.config, &SearchQuery::new(text).with_limit(1_000))
        .unwrap();
      assert_eq!(outcome.candidate_count, 33, "{text}");
      assert_eq!(outcome.coverage, CandidateCoverage::Complete, "{text}");
      assert_eq!(outcome.results.len(), 33, "{text}");
    }
    fixture.cleanup();
  }

  #[test]
  fn rank_ids_intersects_and_puts_file_name_hits_first() {
    let first = std::collections::BTreeMap::from([(1, false), (2, true), (3, true), (4, false)]);
    let second = std::collections::BTreeMap::from([(2, false), (3, true), (4, true), (9, true)]);
    // Present in both: 2, 3, 4. Name hits: 3 -> 2, 2 -> 1, 4 -> 1.
    assert_eq!(rank_ids(&[&first, &second]), vec![3, 2, 4]);
    assert_eq!(rank_ids(&[&first]), vec![2, 3, 1, 4]);
    assert!(rank_ids(&[]).is_empty());
  }

  #[test]
  fn smallest_complete_ignores_truncated_lists() {
    let list = |ids: &[i64], complete: bool| TokenPostings {
      ids: ids.iter().map(|id| (*id, false)).collect(),
      complete,
    };
    let lists = vec![
      ("big".to_string(), list(&[1, 2, 3, 4], true)),
      ("cut".to_string(), list(&[1], false)),
      ("small".to_string(), list(&[1, 2], true)),
    ];
    assert_eq!(smallest_complete(&lists), Some(2));
    assert_eq!(smallest_complete(&lists[1..2]), None);
  }

  #[test]
  fn path_tokens_are_matched_by_prefix_case_insensitively() {
    let path = Path::new("/home/me/Alphabet/beta-notes.md");
    assert!(path_has_prefix_tokens(
      path,
      &["alph".to_string(), "bet".to_string()]
    ));
    assert!(!path_has_prefix_tokens(
      path,
      &["alph".to_string(), "gamma".to_string()]
    ));
    assert!(path_has_prefix_tokens(path, &[]));
  }

  /// The lossy-path fallback must stay an index seek: one entry's stored
  /// terms are read through `reverse_postings`, never by scanning the terms
  /// table (recheck P2).
  #[test]
  fn entry_terms_fallback_uses_the_entry_id_index() {
    let index = KuntuIndex::open_memory().unwrap();
    let sql = format!("EXPLAIN QUERY PLAN {}", entry_terms_sql());
    let details = block_on(async {
      let mut rows = index
        .conn
        .query(&sql, params![Value::from(1i64)])
        .await
        .unwrap();
      let mut details = Vec::new();
      while let Some(row) = rows.next().await.unwrap() {
        details.push(row.get::<String>(3).unwrap());
      }
      details
    });
    let plan = details.join(" | ");
    assert!(plan.contains("rowid=?"), "plan: {plan}");
  }

  /// A file name whose bytes are not valid UTF-8 (`common<0xff>.md`).
  /// `upsert_entry_tx` stores the path lossy and `insert_terms_tx` skips the
  /// component, so the terms table holds no `common*` token for that row —
  /// and path verification must not invent one (review P2: tokenizing the
  /// stored lossy path turns `common\ufffd` into a `common*` match, which
  /// v0.4.0's posting intersection excluded). macOS refuses to create such a
  /// file on disk (`EILSEQ` from `open`), so the invalid bytes go through the
  /// real insertion path instead of the filesystem.
  /// Plant the row the write path stores for a file named `common<0xff>.md`:
  /// the lossy path, and terms derived by `insert_terms_tx` itself (the file
  /// name and its invalid component are skipped, the `md` extension and the
  /// valid path components survive).
  #[cfg(unix)]
  fn plant_invalid_byte_entry(index: &mut KuntuIndex, root: &Path) {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let root = kuntu_core::normalize_root_path(root.to_path_buf());
    let invalid_path = root
      .join("rareword")
      .join(OsStr::from_bytes(b"common\xff.md"));
    block_on(async {
      let root_id: i64 = {
        let mut rows = index
          .conn
          .query(
            "SELECT id FROM roots WHERE path = ?1",
            params![root.to_string_lossy()],
          )
          .await
          .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
      };
      let stored = invalid_path.to_string_lossy().into_owned();
      let name = invalid_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
      index
        .conn
        .execute(
          "INSERT INTO entries(root_id,path,name,extension,kind,size) VALUES(?1,?2,?3,'md',1,4)",
          params![root_id, stored.clone(), name.clone(),],
        )
        .await
        .unwrap();
      let entry_id: i64 = {
        let mut rows = index
          .conn
          .query(
            "SELECT id FROM entries WHERE root_id = ?1 AND path = ?2",
            params![root_id, stored],
          )
          .await
          .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
      };
      let tx = index.conn.transaction().await.unwrap();
      insert_terms_tx(&tx, entry_id, &invalid_path).await.unwrap();
      tx.commit().await.unwrap();
    });
  }

  #[cfg(unix)]
  #[test]
  fn path_verification_skips_components_the_indexer_never_tokenized() {
    let root = temp_dir("non-utf8-verify");
    fs::create_dir_all(root.join("rareword")).unwrap();
    for number in 0..3 {
      fs::write(root.join(format!("plain-common-{number}.md")), "b\n").unwrap();
    }

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    plant_invalid_byte_entry(&mut index, &root);

    // The rare token drives the search and `common` is verified against the
    // driving row's own path; the invalid-name file must not pass that check.
    let outcome = index
      .search_with_metrics(&config, &SearchQuery::new("rareword common"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 0);
    assert_eq!(outcome.coverage, CandidateCoverage::Complete);
    assert!(outcome.results.is_empty());
  }

  /// A legitimate U+FFFD is valid UTF-8: `insert_terms_tx` indexes a file
  /// named `common<U+FFFD>.md` like any other name, so `rareword common`
  /// must find it even though the stored path is indistinguishable from a
  /// lossy one (recheck P2: verification must not drop this match).
  #[test]
  fn legitimate_replacement_characters_are_found_by_their_indexed_terms() {
    let root = temp_dir("legit-replacement");
    fs::create_dir_all(root.join("rareword")).unwrap();
    fs::write(root.join("rareword").join("common\u{FFFD}.md"), "a\n").unwrap();
    for number in 0..3 {
      fs::write(root.join(format!("plain-common-{number}.md")), "b\n").unwrap();
    }

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();

    let outcome = index
      .search_with_metrics(&config, &SearchQuery::new("rareword common"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 1);
    assert_eq!(outcome.coverage, CandidateCoverage::Complete);
    assert!(outcome.results[0].path.ends_with("common\u{FFFD}.md"));
  }

  /// The invalid-byte file's extension term is real: `insert_terms_tx` skips
  /// the name component but still indexes the valid `md` extension, so
  /// `rareword md` must find the row through its stored terms (recheck P2).
  #[cfg(unix)]
  #[test]
  fn invalid_byte_names_are_found_through_their_stored_extension_term() {
    let root = temp_dir("non-utf8-extension");
    fs::create_dir_all(root.join("rareword")).unwrap();
    for number in 0..3 {
      fs::write(root.join(format!("plain-common-{number}.md")), "b\n").unwrap();
    }

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    plant_invalid_byte_entry(&mut index, &root);

    let outcome = index
      .search_with_metrics(&config, &SearchQuery::new("rareword md"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 1);
    assert_eq!(outcome.coverage, CandidateCoverage::Complete);
    assert!(outcome.results[0]
      .path
      .to_string_lossy()
      .ends_with("common\u{FFFD}.md"));
  }

  /// The latency gate's timing policy (review P2): the first run pays the
  /// cold page cache, so it is spent as an untimed warm-up; the gate judges
  /// the best of the measured runs after it, and no measured run is
  /// abandoned early (an early break lets the cold run decide the gate).
  #[test]
  fn gate_timing_warms_up_untimed_then_judges_the_best_of_three() {
    use std::cell::Cell;

    let calls = Cell::new(0usize);
    // The cold call first (1001 ms against a 25 ms ceiling), then the warm
    // ones that must decide the gate.
    let best = fixture::measure_warm_best(|| {
      let call = calls.get();
      calls.set(call + 1);
      std::time::Duration::from_millis([1001, 4, 1, 3][call])
    });

    assert_eq!(calls.get(), 4, "one warm-up plus three measured runs");
    assert_eq!(best, std::time::Duration::from_millis(1));
  }
}

#[cfg(test)]
mod m2_acceptance;
#[cfg(test)]
mod m2_tests;

fn legacy_error(e: kuntu_legacy_index::IndexError) -> IndexError {
  IndexError::Io(std::io::Error::other(e.to_string()))
}
