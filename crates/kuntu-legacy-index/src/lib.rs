//! SQLite-compatible metadata index for the file search prototype, backed by
//! the `turso` engine (an in-process Rust rewrite of SQLite).
//!
//! This crate owns a separate file-search database. It does not use the
//! desktop app database and can be rebuilt or queried by the CLI, daemon, or
//! future adapters. The public API is synchronous; turso's async API is
//! driven through a shared executor (`block_on`), because every caller (CLI,
//! daemon, NAPI libuv workers) is synchronous.
//!
//! Engine note: `turso` is pinned to the same generation xross-store uses;
//! upgrades must move both sides together or xross builds compile two
//! different SQL engines.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kuntu_core::{
  ranking::{score_candidate, tokenize},
  BackendError, EntryKind, IndexRebuildStats, IndexRefreshStats, IndexRepairStats, IndexRootStatus,
  MatchKind, MetadataIndex, SearchCandidate, SearchConfig, SearchQuery, SearchResult, SearchRoot,
};
use kuntu_crawler::{crawl, CrawlOptions};
use turso::{params, params_from_iter, Value};

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
pub const SCHEMA_VERSION: i64 = 2;

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
  Engine(turso::Error),
  Io(std::io::Error),
  SchemaTooNew { found: i64, supported: i64 },
  RecreateRequired,
  RecreateFailed(Box<IndexError>),
  UnrecognizedDatabase { path: PathBuf },
}

impl std::fmt::Display for IndexError {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
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

impl From<turso::Error> for IndexError {
  fn from(error: turso::Error) -> Self {
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
pub const MAX_KFS_CANDIDATE_ROWS: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateCoverage {
  /// Every candidate the configured roots could offer was ranked.
  Complete,
  /// The candidate work bound was reached; results are deterministic but
  /// partial and never claim to be the best over the whole root.
  Truncated,
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
  conn: turso::Connection,
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
  async fn begin_writer_txn(&mut self) -> Result<turso::transaction::Transaction<'_>> {
    let tx = self.conn.transaction().await?;
    let mut rows = tx.query("PRAGMA user_version", ()).await?;
    let found = rows.next().await?.expect("version row").get::<i64>(0)?;
    if found != SCHEMA_VERSION {
      return Err(IndexError::SchemaTooNew {
        found,
        supported: SCHEMA_VERSION,
      });
    }
    Ok(tx)
  }

  pub fn open(path: impl AsRef<Path>) -> Result<Self> {
    block_on(Self::open_async(path.as_ref()))
  }

  pub fn open_memory() -> Result<Self> {
    block_on(async {
      let database = turso::Builder::new_local(":memory:").build().await?;
      let conn = database.connect()?;
      let index = KuntuIndex { conn };
      index.enable_foreign_key_cascades().await?;
      index.create_schema().await?;
      index.set_schema_version(SCHEMA_VERSION).await?;
      Ok(index)
    })
  }

  async fn open_async(path: &Path) -> Result<Self> {
    // One retry: a legacy (pre-turso) or unreadable database file is deleted
    // and recreated, because the index is derived data. A second failure is
    // real and propagates.
    let mut attempts = 0;
    loop {
      attempts += 1;
      match Self::open_once(path).await {
        Ok(index) => return Ok(index),
        Err(IndexError::RecreateRequired) if attempts == 1 => {
          remove_database_files(path);
        }
        Err(IndexError::RecreateRequired) => {
          return Err(IndexError::RecreateFailed(Box::new(
            IndexError::RecreateRequired,
          )));
        }
        Err(error) => return Err(error),
      }
    }
  }

  async fn open_once(path: &Path) -> Result<Self> {
    // turso's builder takes a string; rather than alias a database to a
    // lossy-converted path (distinct byte paths collapsing onto one), refuse
    // what cannot be represented exactly.
    let Some(path_text) = path.to_str() else {
      return Err(IndexError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("index database path is not valid UTF-8: {}", path.display()),
      )));
    };
    let database = turso::Builder::new_local(path_text).build().await?;
    let conn = database.connect()?;
    let index = KuntuIndex { conn };
    index.enable_foreign_key_cascades().await?;

    let version = index.schema_version().await?;
    if version > SCHEMA_VERSION {
      return Err(IndexError::SchemaTooNew {
        found: version,
        supported: SCHEMA_VERSION,
      });
    }
    if version == 0 {
      // A fresh file reports version 0 with no tables. Version 0 with the
      // complete KFS schema is a pre-versioning rusqlite database: recreate.
      // Anything else with user tables is not ours and must never be touched
      // automatically — `roots` alone proves nothing.
      let has_tables = any_table_exists(&index.conn).await?;
      if has_tables {
        let kuntu_tables = ["roots", "entries", "terms", "root_state"];
        let mut complete = true;
        for table in kuntu_tables {
          if !table_exists(&index.conn, table).await? {
            complete = false;
            break;
          }
        }
        if complete {
          return Err(IndexError::RecreateRequired);
        }
        return Err(IndexError::UnrecognizedDatabase {
          path: path.to_path_buf(),
        });
      }
    }

    index.create_schema().await?;
    if version < SCHEMA_VERSION {
      index.set_schema_version(SCHEMA_VERSION).await?;
    }
    Ok(index)
  }

  /// The schema leans on `terms.entry_id ... ON DELETE CASCADE`; the engine
  /// only honors that while the per-connection pragma is on. Without it a
  /// rebuild's bulk entry delete leaves orphan terms behind, and row id reuse
  /// then aliases old terms onto new files.
  async fn enable_foreign_key_cascades(&self) -> Result<()> {
    self.conn.execute("PRAGMA foreign_keys = ON", ()).await?;
    Ok(())
  }

  async fn create_schema(&self) -> Result<()> {
    self.conn.execute_batch(SCHEMA_DDL).await?;
    Ok(())
  }

  async fn schema_version(&self) -> Result<i64> {
    let mut rows = self.conn.query("PRAGMA user_version", ()).await?;
    let row = rows
      .next()
      .await?
      .expect("PRAGMA user_version returns a row");
    Ok(row.get::<i64>(0)?)
  }

  async fn set_schema_version(&self, version: i64) -> Result<()> {
    self
      .conn
      .execute_batch(&format!("PRAGMA user_version = {version};"))
      .await?;
    Ok(())
  }

  pub fn rebuild(&mut self, config: &SearchConfig) -> Result<RebuildStats> {
    let now = unix_now();
    let mut total_entries = 0;
    let mut total_skipped = 0;
    let mut errors = Vec::new();

    block_on(async {
      let tx = self.begin_writer_txn().await?;
      for root in config.roots.iter().filter(|root| root.enabled) {
        let root_id = upsert_root_tx(&tx, root).await?;
        let root_config = SearchConfig {
          roots: vec![root.clone()],
        };
        let options = CrawlOptions::new(root_config);
        let (entries, stats) = crawl(&options);
        let root_has_errors = !stats.errors.is_empty();
        total_skipped += stats.skipped;
        errors.extend(stats.errors);

        if root_has_errors {
          mark_root_state_dirty_tx(&tx, root_id).await?;
          continue;
        }

        tx.execute("DELETE FROM entries WHERE root_id = ?1", params![root_id])
          .await?;

        for entry in entries {
          let entry_id = insert_entry_tx(&tx, root_id, &entry, now).await?;
          insert_terms_tx(&tx, entry_id, &entry.path).await?;
          total_entries += 1;
        }

        let count = count_entries_tx(&tx, root_id).await?;
        tx.execute(
          "INSERT INTO root_state (root_id, generation, last_full_scan_at, last_incremental_at, dirty, entry_count)
           VALUES (?1, COALESCE((SELECT generation + 1 FROM root_state WHERE root_id = ?1), 1), ?2, NULL, 0, ?3)
           ON CONFLICT(root_id) DO UPDATE SET
             generation = root_state.generation + 1,
             last_full_scan_at = excluded.last_full_scan_at,
             dirty = 0,
             entry_count = excluded.entry_count",
          params![root_id, now, count],
        )
        .await?;
      }
      tx.commit().await?;
      Ok::<(), IndexError>(())
    })?;

    Ok(RebuildStats {
      roots: config.roots.iter().filter(|root| root.enabled).count(),
      entries: total_entries,
      skipped: total_skipped,
      errors,
    })
  }

  pub fn refresh(&mut self, config: &SearchConfig) -> Result<RefreshStats> {
    let now = unix_now();
    let mut inserted = 0;
    let mut updated = 0;
    let mut deleted = 0;
    let mut unchanged = 0;
    let mut total_skipped = 0;
    let mut errors = Vec::new();

    block_on(async {
      let tx = self.begin_writer_txn().await?;
      for root in config.roots.iter().filter(|root| root.enabled) {
        let root_id = upsert_root_tx(&tx, root).await?;
        let mut existing = load_existing_entries_tx(&tx, root_id).await?;
        let root_config = SearchConfig {
          roots: vec![root.clone()],
        };
        let options = CrawlOptions::new(root_config);
        let (entries, stats) = crawl(&options);
        let root_has_errors = !stats.errors.is_empty();
        total_skipped += stats.skipped;
        errors.extend(stats.errors);

        if root_has_errors {
          mark_root_state_dirty_tx(&tx, root_id).await?;
          continue;
        }

        let mut seen_paths = HashSet::new();
        for entry in entries {
          let path_key = entry.path.to_string_lossy().into_owned();
          seen_paths.insert(path_key.clone());
          if let Some(existing_entry) = existing.remove(&path_key) {
            if entry_changed(&existing_entry, &entry) {
              update_entry_tx(&tx, existing_entry.id, &entry, now).await?;
              updated += 1;
            } else {
              unchanged += 1;
            }
          } else {
            let entry_id = insert_entry_tx(&tx, root_id, &entry, now).await?;
            insert_terms_tx(&tx, entry_id, &entry.path).await?;
            inserted += 1;
          }
        }

        for (path, existing_entry) in existing {
          if !seen_paths.contains(&path) && !existing_entry.deleted {
            mark_entry_deleted_tx(&tx, existing_entry.id, now).await?;
            deleted += 1;
          }
        }

        let count = count_entries_tx(&tx, root_id).await?;
        tx.execute(
          "INSERT INTO root_state (root_id, generation, last_full_scan_at, last_incremental_at, dirty, entry_count)
           VALUES (?1, COALESCE((SELECT generation + 1 FROM root_state WHERE root_id = ?1), 1), NULL, ?2, ?3, ?4)
           ON CONFLICT(root_id) DO UPDATE SET
             generation = root_state.generation + 1,
             last_incremental_at = excluded.last_incremental_at,
             dirty = excluded.dirty,
             entry_count = excluded.entry_count",
          params![root_id, now, root_has_errors, count],
        )
        .await?;
      }
      tx.commit().await?;
      Ok::<(), IndexError>(())
    })?;

    Ok(RefreshStats {
      roots: config.roots.iter().filter(|root| root.enabled).count(),
      inserted,
      updated,
      deleted,
      unchanged,
      skipped: total_skipped,
      errors,
    })
  }

  pub fn repair(&mut self, config: &SearchConfig) -> Result<RepairStats> {
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
    block_on(async {
      let tx = self.begin_writer_txn().await?;
      let root_id = upsert_root_tx(&tx, root).await?;
      let count = count_entries_tx(&tx, root_id).await?;
      tx.execute(
        "INSERT INTO root_state (root_id, generation, last_full_scan_at, last_incremental_at, dirty, entry_count)
         VALUES (?1, COALESCE((SELECT generation FROM root_state WHERE root_id = ?1), 0), NULL, NULL, 1, ?2)
         ON CONFLICT(root_id) DO UPDATE SET dirty = 1",
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
    let budget = options.fetch_budget.clamp(1, MAX_KFS_CANDIDATE_ROWS);
    block_on(async {
      let empty = IndexedSearchOutcome {
        candidate_count: 0,
        results: Vec::new(),
        truncated: false,
        coverage: CandidateCoverage::Complete,
      };
      if !query.include_files && !query.include_directories {
        return Ok(empty);
      }
      let root_ids = root_ids_for_config(&self.conn, config).await?;
      if root_ids.is_empty() {
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
      let scope = CandidateScope::load(&self.conn, &root_ids, query).await?;
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
      let coverage = if candidate_saturated {
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
    block_on(async {
      let mut rows = self.conn.query(
        "SELECT r.path, COALESCE(s.entry_count, 0), COALESCE(s.generation, 0), COALESCE(s.dirty, 0),
                s.last_full_scan_at, s.last_incremental_at
         FROM roots r
         LEFT JOIN root_state s ON s.root_id = r.id
         ORDER BY r.path",
        (),
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
    "SELECT id, root_id, path, kind, size, mtime, extension, hidden, ignored, sensitive, deleted
       FROM entries WHERE id IN ({})",
    placeholders(id_count)
  )
}

async fn fetch_entries_by_id(conn: &turso::Connection, ids: &[i64]) -> Result<Vec<FetchedEntry>> {
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
}

impl CandidateScope {
  async fn load(conn: &turso::Connection, root_ids: &[i64], query: &SearchQuery) -> Result<Self> {
    let mut root_meta = HashMap::new();
    let mut rows = conn
      .query(
        "SELECT id, priority, include_hidden, include_ignored FROM roots",
        (),
      )
      .await?;
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
      allowed_roots: root_ids.iter().copied().collect(),
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
    if entry.deleted || entry.sensitive || !self.allowed_roots.contains(&entry.root_id) {
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
    if entry.hidden && !(query.include_hidden || root_hidden) {
      return None;
    }
    if entry.ignored && !(query.include_ignored || root_ignored) {
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
/// terms, including its extension term, through `idx_terms_entry`.
fn entry_terms_sql() -> &'static str {
  "SELECT term FROM terms WHERE entry_id = ?1"
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
  conn: &turso::Connection,
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
    let term = row.get::<String>(0)?;
    remaining.retain(|token| !term.starts_with(token));
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
  conn: &turso::Connection,
  term: &str,
  row_limit: usize,
  distinct_cap: usize,
) -> Result<TokenPostings> {
  let (clause, mut values) = term_prefix_clause(term);
  let sql = format!("SELECT entry_id, field FROM terms WHERE {clause} LIMIT ?");
  values.push(Value::from(
    i64::try_from(row_limit).unwrap_or(i64::MAX - 1) + 1,
  ));
  let mut rows = conn.query(&sql, params_from_iter(values)).await?;
  let mut ids = std::collections::BTreeMap::new();
  let mut seen_rows = 0usize;
  let mut complete = true;
  while let Some(row) = rows.next().await? {
    if seen_rows == row_limit {
      complete = false;
      break;
    }
    seen_rows += 1;
    let id = row.get::<i64>(0)?;
    let name_hit = row.get::<i64>(1)? == 1;
    if !ids.contains_key(&id) && ids.len() == distinct_cap {
      complete = false;
      break;
    }
    let entry = ids.entry(id).or_insert(false);
    *entry = *entry || name_hit;
  }
  Ok(TokenPostings { ids, complete })
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

#[derive(Debug, Clone)]
struct ExistingEntry {
  id: i64,
  kind: EntryKind,
  size: Option<i64>,
  mtime: Option<i64>,
  hidden: bool,
  ignored: bool,
  sensitive: bool,
  deleted: bool,
}

async fn upsert_root_tx(
  tx: &turso::transaction::Transaction<'_>,
  root: &SearchRoot,
) -> Result<i64> {
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

async fn load_existing_entries_tx(
  tx: &turso::transaction::Transaction<'_>,
  root_id: i64,
) -> Result<HashMap<String, ExistingEntry>> {
  let mut rows = tx
    .query(
      "SELECT path, id, kind, size, mtime, hidden, ignored, sensitive, deleted
           FROM entries
           WHERE root_id = ?1",
      params![root_id],
    )
    .await?;

  let mut entries = HashMap::new();
  while let Some(row) = rows.next().await? {
    let path: String = row.get(0)?;
    entries.insert(
      path,
      ExistingEntry {
        id: row.get(1)?,
        kind: int_to_kind(row.get(2)?),
        size: row.get(3)?,
        mtime: row.get(4)?,
        hidden: row.get::<i64>(5)? != 0,
        ignored: row.get::<i64>(6)? != 0,
        sensitive: row.get::<i64>(7)? != 0,
        deleted: row.get::<i64>(8)? != 0,
      },
    );
  }
  Ok(entries)
}

async fn insert_entry_tx(
  tx: &turso::transaction::Transaction<'_>,
  root_id: i64,
  entry: &kuntu_crawler::CrawledEntry,
  indexed_at: i64,
) -> Result<i64> {
  let name = entry
    .path
    .file_name()
    .and_then(|value| value.to_str())
    .unwrap_or_default()
    .to_string();
  let extension = entry
    .path
    .extension()
    .and_then(|value| value.to_str())
    .map(|value| value.to_ascii_lowercase());
  let name_lower = name.to_ascii_lowercase();
  tx.execute(
        "INSERT INTO entries
         (root_id, path, path_lower, name, name_lower, extension, kind, size, mtime, hidden, ignored, sensitive, deleted, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13)",
        params![
            root_id,
            entry.path.to_string_lossy(),
            entry.path.to_string_lossy().to_ascii_lowercase(),
            name,
            name_lower,
            extension,
            kind_to_int(entry.kind),
            entry_size_i64(entry),
            entry.mtime,
            entry.decision.hidden,
            entry.decision.ignored,
            entry.decision.sensitive,
            indexed_at
        ],
    )
    .await?;
  // The (root_id, path) pair is unique, so the id of the row this call just
  // wrote is recovered explicitly rather than through a rowid cursor API.
  let mut rows = tx
    .query(
      "SELECT id FROM entries WHERE root_id = ?1 AND path = ?2",
      params![root_id, entry.path.to_string_lossy()],
    )
    .await?;
  let row = rows.next().await?.expect("inserted entry row exists");
  Ok(row.get::<i64>(0)?)
}

async fn update_entry_tx(
  tx: &turso::transaction::Transaction<'_>,
  entry_id: i64,
  entry: &kuntu_crawler::CrawledEntry,
  indexed_at: i64,
) -> Result<()> {
  let name = entry
    .path
    .file_name()
    .and_then(|value| value.to_str())
    .unwrap_or_default()
    .to_string();
  let extension = entry
    .path
    .extension()
    .and_then(|value| value.to_str())
    .map(|value| value.to_ascii_lowercase());
  let name_lower = name.to_ascii_lowercase();
  tx.execute(
    "UPDATE entries
         SET path = ?1,
             path_lower = ?2,
             name = ?3,
             name_lower = ?4,
             extension = ?5,
             kind = ?6,
             size = ?7,
             mtime = ?8,
             hidden = ?9,
             ignored = ?10,
             sensitive = ?11,
             deleted = 0,
             indexed_at = ?12
         WHERE id = ?13",
    params![
      entry.path.to_string_lossy(),
      entry.path.to_string_lossy().to_ascii_lowercase(),
      name,
      name_lower,
      extension,
      kind_to_int(entry.kind),
      entry_size_i64(entry),
      entry.mtime,
      entry.decision.hidden,
      entry.decision.ignored,
      entry.decision.sensitive,
      indexed_at,
      entry_id
    ],
  )
  .await?;
  tx.execute("DELETE FROM terms WHERE entry_id = ?1", params![entry_id])
    .await?;
  insert_terms_tx(tx, entry_id, &entry.path).await
}

async fn mark_entry_deleted_tx(
  tx: &turso::transaction::Transaction<'_>,
  entry_id: i64,
  indexed_at: i64,
) -> Result<()> {
  tx.execute(
    "UPDATE entries SET deleted = 1, indexed_at = ?1 WHERE id = ?2",
    params![indexed_at, entry_id],
  )
  .await?;
  tx.execute("DELETE FROM terms WHERE entry_id = ?1", params![entry_id])
    .await?;
  Ok(())
}

async fn insert_terms_tx(
  tx: &turso::transaction::Transaction<'_>,
  entry_id: i64,
  path: &Path,
) -> Result<()> {
  let name = path
    .file_name()
    .and_then(|value| value.to_str())
    .unwrap_or_default();
  for term in tokenize(name) {
    tx.execute(
      "INSERT OR IGNORE INTO terms (term, entry_id, field, weight) VALUES (?1, ?2, 1, 100)",
      params![term, entry_id],
    )
    .await?;
  }
  if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
    tx.execute(
      "INSERT OR IGNORE INTO terms (term, entry_id, field, weight) VALUES (?1, ?2, 2, 50)",
      params![extension.to_ascii_lowercase(), entry_id],
    )
    .await?;
  }
  for component in path
    .components()
    .filter_map(|component| component.as_os_str().to_str())
  {
    for term in tokenize(component) {
      tx.execute(
        "INSERT OR IGNORE INTO terms (term, entry_id, field, weight) VALUES (?1, ?2, 3, 10)",
        params![term, entry_id],
      )
      .await?;
    }
  }
  Ok(())
}

fn entry_changed(existing: &ExistingEntry, entry: &kuntu_crawler::CrawledEntry) -> bool {
  existing.deleted
    || existing.kind != entry.kind
    || existing.size != entry_size_i64(entry)
    || existing.mtime != entry.mtime
    || existing.hidden != entry.decision.hidden
    || existing.ignored != entry.decision.ignored
    || existing.sensitive != entry.decision.sensitive
}

fn entry_size_i64(entry: &kuntu_crawler::CrawledEntry) -> Option<i64> {
  entry.size.and_then(|size| i64::try_from(size).ok())
}

async fn count_entries_tx(tx: &turso::transaction::Transaction<'_>, root_id: i64) -> Result<i64> {
  let mut rows = tx
    .query(
      "SELECT COUNT(*) FROM entries WHERE root_id = ?1 AND deleted = 0",
      params![root_id],
    )
    .await?;
  let row = rows.next().await?.expect("count returns a row");
  Ok(row.get::<i64>(0)?)
}

async fn mark_root_state_dirty_tx(
  tx: &turso::transaction::Transaction<'_>,
  root_id: i64,
) -> Result<()> {
  let count = count_entries_tx(tx, root_id).await?;
  tx.execute(
        "INSERT INTO root_state (root_id, generation, last_full_scan_at, last_incremental_at, dirty, entry_count)
         VALUES (?1, COALESCE((SELECT generation FROM root_state WHERE root_id = ?1), 0), NULL, NULL, 1, ?2)
         ON CONFLICT(root_id) DO UPDATE SET
           dirty = 1,
           entry_count = excluded.entry_count",
        params![root_id, count],
    )
    .await?;
  Ok(())
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

async fn root_ids_for_config(conn: &turso::Connection, config: &SearchConfig) -> Result<Vec<i64>> {
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

async fn table_exists(conn: &turso::Connection, table: &str) -> Result<bool> {
  let mut rows = conn
    .query(
      "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
      params![table],
    )
    .await?;
  Ok(rows.next().await?.is_some())
}

async fn any_table_exists(conn: &turso::Connection) -> Result<bool> {
  let mut rows = conn
    .query(
      "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
      (),
    )
    .await?;
  Ok(rows.next().await?.is_some())
}

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

pub const SCHEMA_DDL: &str = "
CREATE TABLE IF NOT EXISTS roots (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  enabled INTEGER NOT NULL,
  priority INTEGER NOT NULL DEFAULT 0,
  include_hidden INTEGER NOT NULL DEFAULT 0,
  include_ignored INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS entries (
  id INTEGER PRIMARY KEY,
  root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  path_lower TEXT NOT NULL DEFAULT '',
  name TEXT NOT NULL,
  name_lower TEXT NOT NULL,
  extension TEXT,
  kind INTEGER NOT NULL,
  size INTEGER,
  mtime INTEGER,
  hidden INTEGER NOT NULL DEFAULT 0,
  ignored INTEGER NOT NULL DEFAULT 0,
  sensitive INTEGER NOT NULL DEFAULT 0,
  deleted INTEGER NOT NULL DEFAULT 0,
  indexed_at INTEGER NOT NULL,
  UNIQUE(root_id, path)
);

CREATE TABLE IF NOT EXISTS terms (
  term TEXT NOT NULL,
  entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
  field INTEGER NOT NULL,
  weight INTEGER NOT NULL,
  PRIMARY KEY(term, entry_id, field)
);

CREATE TABLE IF NOT EXISTS root_state (
  root_id INTEGER PRIMARY KEY REFERENCES roots(id) ON DELETE CASCADE,
  generation INTEGER NOT NULL,
  last_full_scan_at INTEGER,
  last_incremental_at INTEGER,
  dirty INTEGER NOT NULL DEFAULT 0,
  entry_count INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_entries_root_deleted ON entries(root_id, deleted);
CREATE INDEX IF NOT EXISTS idx_entries_path ON entries(path);
CREATE INDEX IF NOT EXISTS idx_entries_ext ON entries(extension);
CREATE INDEX IF NOT EXISTS idx_terms_term ON terms(term);
CREATE INDEX IF NOT EXISTS idx_terms_entry ON terms(entry_id);
CREATE INDEX IF NOT EXISTS idx_entries_root_deleted_path_lower ON entries(root_id, deleted, path_lower);
";
