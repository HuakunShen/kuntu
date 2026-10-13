//! Shared access, lease capabilities, scan state, additive migration and hosts.
use super::*;
type EntrySnapshot = HashMap<String, (i64, Option<i64>, Option<i64>)>;
pub(crate) struct CleanupBatch {
  pub inspected: usize,
  pub deleted: usize,
  pub done: bool,
}
pub(crate) fn cleanup_seek_sql() -> &'static str {
  "SELECT id,root_id,scan_gen FROM entries WHERE id>?1 ORDER BY id LIMIT 5000"
}
use std::cell::Cell;
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
  ReadOnly,
  ReadWrite,
}
pub fn fault(point: &str) {
  if std::env::var("KUNTU_FAULT").ok().as_deref() == Some(point) {
    if let Ok(marker) = std::env::var("KUNTU_FAULT_MARKER") {
      std::fs::write(marker, point).expect("fault handshake");
    }
    loop {
      thread::sleep(Duration::from_millis(10));
    }
  }
}
#[cfg(test)]
thread_local! {
  pub(crate) static OPEN_CHECK_ERRORS: std::cell::RefCell<std::collections::VecDeque<i32>> = const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
  pub(crate) static OPEN_INTEGRITY_CHECKS: Cell<usize> = const { Cell::new(0) };
  pub(crate) static OPEN_CHECK_TIMEOUT: Cell<i64> = const { Cell::new(0) };
}
fn injected_open_error(_raw: &rusqlite::Connection) -> rusqlite::Result<()> {
  #[cfg(test)]
  {
    OPEN_CHECK_TIMEOUT.with(|t| {
      t.set(
        _raw
          .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
          .unwrap(),
      )
    });
    if let Some(code) = OPEN_CHECK_ERRORS.with(|errors| errors.borrow_mut().pop_front()) {
      return Err(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(code),
        None,
      ));
    }
  }
  Ok(())
}
fn opening_check(raw: &rusqlite::Connection) -> rusqlite::Result<()> {
  injected_open_error(raw)?;
  // Header validation is bounded; schema, identity and floors follow in from_raw.
  raw
    .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
    .map(|_| ())
}
fn recovery_check(raw: &rusqlite::Connection) -> rusqlite::Result<String> {
  injected_open_error(raw)?;
  #[cfg(test)]
  OPEN_INTEGRITY_CHECKS.with(|n| n.set(n.get() + 1));
  raw.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
}
fn corruption_error(error: &IndexError) -> bool {
  matches!(error, IndexError::Engine(e) if matches!(e.sqlite_error_code(),
    Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)))
}
fn confirmed_corruption(checked: rusqlite::Result<String>) -> Result<bool> {
  match checked {
    Ok(s) => Ok(s != "ok"),
    Err(e)
      if matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
      ) =>
    {
      Ok(true)
    }
    Err(e) => Err(e.into()),
  }
}
impl KuntuIndex {
  pub fn open_shared(home: Home, readonly: bool, consumer: &str) -> Result<Self> {
    Self::open_shared_minor(home, readonly, consumer, BUILD_MINOR)
  }
  pub fn open_shared_minor(home: Home, readonly: bool, consumer: &str, minor: i64) -> Result<Self> {
    // Lowering the supported minor is conservative (and useful to probes).
    // A caller cannot claim structures this compiled engine cannot maintain.
    if !(0..=BUILD_MINOR).contains(&minor) {
      return Err(IndexError::SchemaTooNew {
        found: minor,
        supported: BUILD_MINOR,
      });
    }
    let _local = kuntu_store::bootstrap_process_guard()?;
    let _boot = home
      .lock("bootstrap.lock", true)?
      .ok_or(IndexError::LeaseRequired)?;
    if home.exists("quarantine-3.json")? {
      if readonly {
        return Err(IndexError::RecreateRequired);
      }
      let _lease = home
        .lock("indexer.lock", false)?
        .ok_or(IndexError::LeaseRequired)?;
      let _epoch = home
        .lock("epoch-3.lock", false)?
        .ok_or(IndexError::LeaseRequired)?;
      home.resume_quarantine(3, &_lease, &_epoch)?;
    }
    let reader = home.lifetime(3)?;
    kuntu_vfs::register()?;
    let name = home.sqlite_name("index.v3.db")?;
    let flags = if readonly {
      rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
      rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
    };
    let raw = rusqlite::Connection::open_with_flags_and_vfs(name, flags, "kuntu-fd")?;
    raw.busy_timeout(Duration::from_millis(5000))?;
    let opened = opening_check(&raw)
      .map_err(IndexError::Engine)
      .and_then(|_| Self::from_raw(raw, Some(home.clone()), true, readonly, consumer, minor));
    match opened {
      Ok(mut result) => {
        result._reader = Some(reader);
        Ok(result)
      }
      Err(e) if !corruption_error(&e) => Err(e),
      Err(_) => {
        drop(reader);
        if readonly {
          return Err(IndexError::RecreateRequired);
        }
        let writer = home
          .lock("indexer.lock", false)?
          .ok_or(IndexError::LeaseRequired)?;
        let epoch = home
          .lock("epoch-3.lock", false)?
          .ok_or(IndexError::LeaseRequired)?;
        let recheck = rusqlite::Connection::open_with_flags_and_vfs(
          home.sqlite_name("index.v3.db")?,
          rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
          "kuntu-fd",
        )?;
        recheck.busy_timeout(Duration::from_millis(5000))?;
        let corrupt = confirmed_corruption(recovery_check(&recheck))?;
        drop(recheck);
        if corrupt {
          home.resume_quarantine(3, &writer, &epoch)?;
        }
        drop(epoch);
        drop(writer);
        drop(_boot);
        drop(_local);
        Self::open_shared_minor(home, readonly, consumer, minor)
      }
    }
  }
  pub(crate) fn open_private(path: &Path, readonly: bool) -> Result<Self> {
    kuntu_vfs::register()?;
    let flags = if readonly {
      rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
      rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
    };
    if !readonly {
      use std::os::unix::fs::OpenOptionsExt;
      let _ = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix_flags())
        .open(path)?;
    }
    // Read the format before choosing either engine. A v2 source stays v2;
    // the old constructor opens exactly the supplied path through its backend.
    if path.exists() {
      let inspect =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
      inspect.busy_timeout(Duration::from_millis(5000))?;
      let version: i64 = inspect.query_row("PRAGMA user_version", [], |r| r.get(0))?;
      drop(inspect);
      if version == 2 {
        let mut index = KuntuIndex::open_memory()?;
        index.legacy = Some(kuntu_legacy_index::KuntuIndex::open(path).map_err(legacy_error)?);
        index.conn.readonly.set(readonly);
        let parent = std::fs::canonicalize(path.parent().unwrap_or(Path::new(".")))?;
        index.home = Some(Home::open(&parent.join(format!(
          "{}.kuntu",
          path.file_name().unwrap().to_string_lossy()
        )))?);
        return Ok(index);
      }
    }
    let raw = rusqlite::Connection::open_with_flags(path, flags)?;
    let mut index = Self::from_raw(raw, None, false, readonly, "private", 1)?;
    // Private compatibility stores have their own permanent lease directory.
    if !readonly {
      let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
      let parent = std::fs::canonicalize(parent)?;
      let leaf = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("Missing DB leaf."))?
        .to_string_lossy();
      index.home = Some(Home::open(&parent.join(format!("{leaf}.kuntu")))?);
    }
    Ok(index)
  }
  pub(crate) fn from_raw(
    raw: rusqlite::Connection,
    home: Option<Home>,
    shared: bool,
    readonly: bool,
    consumer: &str,
    minor: i64,
  ) -> Result<Self> {
    raw.busy_timeout(Duration::from_millis(5000))?;
    assert!(
      sqlite_runtime_supported(rusqlite::version_number()),
      "SQLite WAL-reset fix is required"
    );
    let conn = engine::Connection {
      raw,
      readonly: Cell::new(readonly),
      minor,
      lease: std::cell::RefCell::new(None),
    };
    let raw = &conn.raw;
    raw.create_scalar_function(
      "kuntu_terms",
      2,
      rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
      |ctx| {
        let Some(encoded) = ctx.get::<Option<Vec<u8>>>(0)? else {
          return Ok("[]".to_string());
        };
        let which = ctx.get::<i64>(1)?;
        let (name, path) =
          decode_terms(&encoded).map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
        serde_json::to_string(if which == 1 { &name } else { &path })
          .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))
      },
    )?;
    let version: i64 = raw.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let tables: i64 = raw.query_row(
      "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
      [],
      |r| r.get(0),
    )?;
    if version > 3 {
      return Err(IndexError::SchemaTooNew {
        found: version,
        supported: 3,
      });
    }
    if version != 3 && tables != 0 {
      return Err(if version == 2 {
        IndexError::RecreateRequired
      } else {
        IndexError::UnrecognizedDatabase {
          path: PathBuf::from(raw.path().unwrap_or(":memory:")),
        }
      });
    }
    if version == 0 {
      if readonly {
        return Err(IndexError::ReadOnly);
      }
      // Bootstrap is serialized before this function by the shared-home lock.
      let tx = conn.begin_writer_txn(true)?;
      if let Err(e) = raw.execute_batch(SCHEMA_DDL) {
        let _ = raw.execute_batch("ROLLBACK");
        return Err(e.into());
      }
      let name = postings_delete_sql(true)
        .replace("?1", "OLD.id")
        .replace("?2", "1");
      let path = postings_delete_sql(false)
        .replace("?1", "OLD.id")
        .replace("?2", "2");
      raw.execute_batch(&format!("CREATE TRIGGER entries_postings_delete BEFORE DELETE ON entries BEGIN {name}; {path}; END;"))?;
      if let Some(h) = &home {
        raw.execute(
          "UPDATE meta SET value=?1 WHERE key='store_id'",
          params![h.id()?],
        )?;
      }
      block_on(tx.commit())?;
    }
    let read_min: i64 = raw.query_row(
      "SELECT value FROM meta WHERE key='min_reader_minor'",
      [],
      |r| r.get(0),
    )?;
    if read_min > minor {
      return Err(IndexError::SchemaTooNew {
        found: read_min,
        supported: minor,
      });
    }
    let epoch: i64 = raw.query_row("SELECT value FROM meta WHERE key='schema_epoch'", [], |r| {
      r.get(0)
    })?;
    if epoch != 3 {
      return Err(IndexError::SchemaTooNew {
        found: epoch,
        supported: 3,
      });
    }
    if let Some(h) = &home {
      let id: String = raw.query_row("SELECT value FROM meta WHERE key='store_id'", [], |r| {
        r.get(0)
      })?;
      if id != h.id()? {
        return Err(IndexError::UnrecognizedDatabase {
          path: h.path.join("index.v3.db"),
        });
      }
    }
    if !shared {
      let id: String = raw.query_row("SELECT value FROM meta WHERE key='store_id'", [], |r| {
        r.get(0)
      })?;
      if id != "private" {
        return Err(IndexError::UnrecognizedDatabase {
          path: PathBuf::from(raw.path().unwrap_or(":memory:")),
        });
      }
    }
    let write_min: i64 = raw.query_row(
      "SELECT value FROM meta WHERE key='min_writer_minor'",
      [],
      |r| r.get(0),
    )?;
    let ro = readonly || write_min > minor;
    if !ro {
      raw.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA wal_autocheckpoint=1000;",
      )?;
    }
    raw.execute_batch("PRAGMA foreign_keys=ON; PRAGMA mmap_size=67108864;")?;
    if ro {
      raw.execute_batch("PRAGMA query_only=ON")?;
    }
    conn.readonly.set(ro);
    Ok(Self {
      legacy: None,
      conn,
      home,
      cancel: Arc::new(AtomicBool::new(false)),
      _reader: None,
      shared,
      consumer: consumer.into(),
      data_version: Cell::new(0),
      search_profile: Cell::new(SearchProfile::default()),
    })
  }
  pub fn import_v2(&mut self, source: &Path) -> Result<RebuildStats> {
    self.ensure_lease()?;
    let home = self.home.as_ref().ok_or(IndexError::LeaseRequired)?;
    let _bootstrap = home
      .lock("import.lock", true)?
      .ok_or(IndexError::LeaseRequired)?;
    let parent = source
      .parent()
      .ok_or_else(|| std::io::Error::other("Missing source parent."))?;
    // Source traversal is descriptor-relative too. It validates the opened source directory; no source engine is ever opened for writing.
    let src = kuntu_store::SourceDir::open(parent)?;
    let leaf = source
      .file_name()
      .and_then(|s| s.to_str())
      .ok_or_else(|| std::io::Error::other("Invalid source path."))?;
    use std::os::unix::fs::MetadataExt;
    // Open every source member first and preserve their identities.
    let mut inputs = Vec::new();
    for ext in ["", "-wal", "-shm"] {
      match src.read_file(&format!("{leaf}{ext}")) {
        Ok(f) => inputs.push((ext, f)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !ext.is_empty() => {}
        Err(e) => return Err(e.into()),
      }
    }
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|e| std::io::Error::other(e.to_string()))?;
    let leaf = format!(
      "import-{}.db",
      random
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>()
    );
    let mut cleanup = ImportSnapshot {
      home: home.clone(),
      created: Vec::new(),
    };
    let mut outputs = Vec::new();
    for (ext, _) in &inputs {
      let name = format!("{leaf}{ext}");
      outputs.push((*ext, home.create_new_file(&name)?));
      cleanup.created.push(name);
    }
    // Check the complete cross-product before copying even one byte.
    for (_, output) in &outputs {
      let destination = output.metadata()?;
      for (_, input) in &inputs {
        let source = input.metadata()?;
        if source.dev() == destination.dev() && source.ino() == destination.ino() {
          return Err(std::io::Error::other("Import source overlaps its snapshot.").into());
        }
      }
    }
    for ((_, input), (_, output)) in inputs.iter_mut().zip(outputs.iter_mut()) {
      std::io::copy(input, output)?;
      output.sync_all()?;
    }
    drop(outputs);
    let copy = rusqlite::Connection::open_with_flags_and_vfs(
      home.sqlite_name(&leaf)?,
      rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
      "kuntu-fd",
    )?;
    copy.busy_timeout(Duration::from_millis(5000))?;
    let version: i64 = copy.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 2 {
      return Err(IndexError::RecreateRequired);
    }
    let paths = {
      let mut q = copy.prepare("SELECT path FROM roots WHERE enabled=1")?;
      let r = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
      r
    };
    drop(copy);
    drop(cleanup);
    let config = SearchConfig {
      roots: paths.into_iter().map(SearchRoot::new).collect(),
    };
    for root in &config.roots {
      self.register_root(root)?;
    }
    self.rebuild(&config)
  }
  pub fn is_shared(&self) -> bool {
    self.shared
  }
  pub fn access(&self) -> Access {
    if self.conn.readonly.get() {
      Access::ReadOnly
    } else {
      Access::ReadWrite
    }
  }
  pub fn acquire_writer(&mut self) -> Result<bool> {
    if self.conn.readonly.get() {
      self.conn.lease.borrow_mut().take();
      return Err(IndexError::ReadOnly);
    }
    if self.conn.lease.borrow().is_none() {
      if let Some(home) = &self.home {
        *self.conn.lease.borrow_mut() = home.lock("indexer.lock", false)?;
        if self.conn.lease.borrow().is_none() {
          return Ok(false);
        }
      }
    }
    // Acquiring a blocking capability never makes cached compatibility true.
    let result = (|| {
      self.recheck_read_access()?;
      if self.legacy.is_some() {
        return Ok(());
      }
      let floor: i64 = self.conn.raw.query_row(
        "SELECT value FROM meta WHERE key='min_writer_minor'",
        [],
        |r| r.get(0),
      )?;
      if floor > self.conn.minor {
        self.conn.readonly.set(true);
        return Err(IndexError::ReadOnly);
      }
      let current: i64 =
        self
          .conn
          .raw
          .query_row("SELECT value FROM meta WHERE key='schema_minor'", [], |r| {
            r.get(0)
          })?;
      if current < 1 && self.conn.minor >= 1 && self.home.is_some() {
        self.migrate(1)?;
      }
      self.reconcile_sensitive_policy()?;
      Ok(())
    })();
    if let Err(error) = result {
      self.conn.lease.borrow_mut().take();
      return Err(error);
    }
    Ok(true)
  }
  pub(crate) fn check_cancelled(&self) -> Result<()> {
    if self.cancel.load(Ordering::Acquire) {
      Err(IndexError::Cancelled)
    } else {
      Ok(())
    }
  }
  pub(crate) fn reconcile_sensitive_policy(&self) -> Result<()> {
    if self.home.is_some() && self.conn.lease.borrow().is_none() {
      return Err(IndexError::LeaseRequired);
    }
    let identity = kuntu_core::sensitive_policy_identity();
    let stored: String =
      self
        .conn
        .raw
        .query_row("SELECT value FROM meta WHERE key='policy_hash'", [], |r| {
          r.get(0)
        })?;
    if stored == identity {
      return Ok(());
    }
    let tx = self.conn.begin_writer_txn(false)?;
    // Re-read inside the transaction after its lock wait.
    let stored: String =
      tx.conn
        .raw
        .query_row("SELECT value FROM meta WHERE key='policy_hash'", [], |r| {
          r.get(0)
        })?;
    if stored != identity {
      tx.conn.raw.execute(
        "UPDATE root_state SET status='dirty',dirty=1,policy_revision=policy_revision+1",
        [],
      )?;
      tx.conn.raw.execute(
        "UPDATE meta SET value=?1 WHERE key='policy_hash'",
        params![identity],
      )?;
    }
    block_on(tx.commit())?;
    fault("policy_invalidated");
    Ok(())
  }
  pub(crate) fn ensure_lease(&mut self) -> Result<()> {
    if self.conn.readonly.get() {
      return Err(IndexError::ReadOnly);
    }
    if !self.shared && self.conn.lease.borrow().is_none() {
      self.acquire_writer()?;
    }
    if self.home.is_some() && self.conn.lease.borrow().is_none() {
      return Err(IndexError::LeaseRequired);
    }
    self.recheck_read_access()?;
    if self.legacy.is_none() {
      let floor: i64 = self.conn.raw.query_row(
        "SELECT value FROM meta WHERE key='min_writer_minor'",
        [],
        |r| r.get(0),
      )?;
      if floor > self.conn.minor {
        self.conn.readonly.set(true);
        self.conn.lease.borrow_mut().take();
        return Err(IndexError::ReadOnly);
      }
    }
    Ok(())
  }
  pub fn register_root(&self, root: &SearchRoot) -> Result<i64> {
    let tx = self.conn.begin_writer_txn(false)?;
    let id = block_on(upsert_root_tx(&tx, root))?;
    let previous:Option<(i32,bool,bool,bool)>=tx.conn.raw.query_row("SELECT priority,include_hidden,include_ignored,enabled FROM root_consumers WHERE root_id=?1 AND consumer=?2",params![id,self.consumer],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).ok();
    tx.conn.raw.execute("INSERT INTO root_consumers VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(root_id,consumer) DO UPDATE SET priority=excluded.priority,include_hidden=excluded.include_hidden,include_ignored=excluded.include_ignored,enabled=excluded.enabled",params![id,self.consumer,root.priority,root.include_hidden,root.include_ignored,root.enabled])?;
    tx.conn.raw.execute(
      "INSERT OR IGNORE INTO root_state(root_id) VALUES(?1)",
      params![id],
    )?;
    if previous
      != Some((
        root.priority,
        root.include_hidden,
        root.include_ignored,
        root.enabled,
      ))
    {
      tx.conn.raw.execute("UPDATE root_state SET policy_revision=policy_revision+1,status='dirty',dirty=1 WHERE root_id=?1",params![id])?;
    }
    tx.conn.raw.execute("UPDATE roots SET enabled=EXISTS(SELECT 1 FROM root_consumers WHERE root_id=?1 AND enabled=1),include_hidden=EXISTS(SELECT 1 FROM root_consumers WHERE root_id=?1 AND enabled=1 AND include_hidden=1),include_ignored=EXISTS(SELECT 1 FROM root_consumers WHERE root_id=?1 AND enabled=1 AND include_ignored=1) WHERE id=?1",params![id])?;
    block_on(tx.commit())?;
    Ok(id)
  }
  pub fn remove_consumer(&self, root_id: i64) -> Result<()> {
    let tx = self.conn.begin_writer_txn(false)?;
    tx.conn.raw.execute(
      "DELETE FROM root_consumers WHERE root_id=?1 AND consumer=?2",
      params![root_id, self.consumer],
    )?;
    tx.conn.raw.execute("UPDATE roots SET enabled=EXISTS(SELECT 1 FROM root_consumers WHERE root_id=?1 AND enabled=1),include_hidden=EXISTS(SELECT 1 FROM root_consumers WHERE root_id=?1 AND enabled=1 AND include_hidden=1),include_ignored=EXISTS(SELECT 1 FROM root_consumers WHERE root_id=?1 AND enabled=1 AND include_ignored=1) WHERE id=?1",params![root_id])?;
    tx.conn.raw.execute("UPDATE root_state SET policy_revision=policy_revision+1,status='dirty',dirty=1 WHERE root_id=?1",params![root_id])?;
    block_on(tx.commit())
  }
  pub fn begin_scan(&self, root_id: i64) -> Result<i64> {
    if self.home.is_some() && self.conn.lease.borrow().is_none() {
      return Err(IndexError::LeaseRequired);
    }
    let tx = self.conn.begin_writer_txn(false)?;
    tx.conn.raw.execute("UPDATE root_state SET active_scan_gen=active_scan_gen+1,status='building',dirty=1,scan_policy_revision=policy_revision WHERE root_id=?1",params![root_id])?;
    let gen = tx.conn.raw.query_row(
      "SELECT active_scan_gen FROM root_state WHERE root_id=?1",
      params![root_id],
      |r| r.get(0),
    )?;
    block_on(tx.commit())?;
    fault("building_committed");
    Ok(gen)
  }
  pub fn check_scan(&self, root_id: i64, gen: i64) -> Result<()> {
    let ok:bool=self.conn.raw.query_row("SELECT status='building' AND active_scan_gen=?2 AND scan_policy_revision=policy_revision AND EXISTS(SELECT 1 FROM roots WHERE id=?1 AND enabled=1) FROM root_state WHERE root_id=?1",params![root_id,gen],|r|r.get(0))?;
    if ok {
      Ok(())
    } else {
      Err(IndexError::StaleScan)
    }
  }
  pub fn complete_scan(&self, root_id: i64, gen: i64) -> Result<()> {
    if self.home.is_some() && self.conn.lease.borrow().is_none() {
      return Err(IndexError::LeaseRequired);
    }
    let mut cursor = 0;
    loop {
      self.check_cancelled()?;
      let batch = self.cleanup_stale_batch(root_id, gen, &mut cursor)?;
      debug_assert!(batch.deleted <= batch.inspected && batch.inspected <= 5000);
      if batch.done {
        break;
      }
    }
    fault("before_ready");
    self.check_cancelled()?;
    let tx = self.conn.begin_writer_txn(false)?;
    self.check_scan(root_id, gen)?;
    tx.conn.raw.execute("UPDATE root_state SET status='ready',dirty=0,generation=generation+1,last_full_scan_at=?3,entry_count=(SELECT count(*) FROM entries WHERE root_id=?1) WHERE root_id=?1 AND active_scan_gen=?2",params![root_id,gen,unix_now()])?;
    block_on(tx.commit())?;
    fault("ready_committed");
    Ok(())
  }
  pub(crate) fn cleanup_stale_batch(
    &self,
    root_id: i64,
    gen: i64,
    cursor: &mut i64,
  ) -> Result<CleanupBatch> {
    if self.home.is_some() && self.conn.lease.borrow().is_none() {
      return Err(IndexError::LeaseRequired);
    }
    self.check_cancelled()?;
    let started = std::time::Instant::now();
    // Limit inspected rows, including other roots and current generations.
    // No root predicate can force SQLite to walk an unbounded gap first.
    let candidates = {
      let mut q = self.conn.raw.prepare(cleanup_seek_sql())?;
      let rows = q
        .query_map(params![*cursor], |r| {
          Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
          ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
      rows
    };
    self.check_cancelled()?;
    let tx = self.conn.begin_writer_txn(false)?;
    self.check_scan(root_id, gen)?;
    let mut inspected = 0;
    let mut deleted = 0;
    let mut next = *cursor;
    for (id, root, stamp) in &candidates {
      self.check_cancelled()?;
      if *root == root_id && *stamp != gen {
        #[cfg(test)]
        m2_tests::cleanup_delay(self.home.as_ref());
        deleted += tx
          .conn
          .raw
          .execute("DELETE FROM entries WHERE id=?1", params![id])?;
      }
      next = *id;
      inspected += 1;
      if inspected >= 5000 || started.elapsed() >= std::time::Duration::from_millis(100) {
        break;
      }
    }
    block_on(tx.commit())?;
    *cursor = next;
    fault("cleanup_committed");
    Ok(CleanupBatch {
      inspected,
      deleted,
      done: candidates.is_empty(),
    })
  }
  pub(crate) fn index_config(&self, id: i64) -> Result<SearchConfig> {
    let r = self.conn.raw.query_row(
      "SELECT path,include_hidden,include_ignored FROM roots WHERE id=?1 AND enabled=1",
      params![id],
      |r| {
        Ok((
          r.get::<_, String>(0)?,
          r.get::<_, bool>(1)?,
          r.get::<_, bool>(2)?,
        ))
      },
    )?;
    let mut root = SearchRoot::new(r.0);
    root.include_hidden = r.1;
    root.include_ignored = r.2;
    Ok(SearchConfig { roots: vec![root] })
  }
  pub(crate) fn snapshot(&self) -> Result<EntrySnapshot> {
    let mut s = self
      .conn
      .raw
      .prepare("SELECT path,kind,size,mtime FROM entries")?;
    let rows = s
      .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
      .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
  }
  pub fn generation_probe(&self) -> Result<i64> {
    self.recheck_read_access()?;
    Ok(
      self
        .conn
        .raw
        .query_row("SELECT value FROM meta WHERE key='generation'", [], |r| {
          r.get(0)
        })?,
    )
  }
  pub fn point_probe(&self) -> Result<i64> {
    self.recheck_read_access()?;
    use rusqlite::OptionalExtension;
    Ok(
      self
        .conn
        .raw
        .query_row("SELECT id FROM entries WHERE id=1", [], |r| r.get(0))
        .optional()?
        .unwrap_or(0),
    )
  }
  pub fn range_probe(&self) -> Result<i64> {
    self.recheck_read_access()?;
    Ok(self.conn.raw.query_row(
      "SELECT count(*) FROM entries WHERE id>=1 AND id<101",
      [],
      |r| r.get(0),
    )?)
  }
  pub fn count_entries(&self) -> Result<i64> {
    self.recheck_read_access()?;
    Ok(
      self
        .conn
        .raw
        .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))?,
    )
  }
  pub fn recheck_read_access(&self) -> Result<()> {
    if self.legacy.is_some() {
      return Ok(());
    }
    let min: i64 = self.conn.raw.query_row(
      "SELECT value FROM meta WHERE key='min_reader_minor'",
      [],
      |r| r.get(0),
    )?;
    if min > self.conn.minor {
      self.conn.readonly.set(true);
      self.conn.lease.borrow_mut().take();
      return Err(IndexError::SchemaTooNew {
        found: min,
        supported: self.conn.minor,
      });
    }
    Ok(())
  }
  pub fn root_state_name(&self, path: &Path) -> Result<String> {
    self.recheck_read_access()?;
    if let Some(legacy) = &self.legacy {
      let s = legacy
        .status()
        .map_err(legacy_error)?
        .into_iter()
        .find(|s| s.path == path)
        .ok_or(IndexError::EmptyScope)?;
      return Ok(if s.dirty { "dirty" } else { "ready" }.into());
    }
    Ok(self.conn.raw.query_row(
      "SELECT s.status FROM root_state s JOIN roots r ON r.id=s.root_id WHERE r.path=?1",
      params![path.to_string_lossy()],
      |r| r.get(0),
    )?)
  }
  pub fn doctor(&self) -> Result<String> {
    self.recheck_read_access()?;
    Ok(
      self
        .conn
        .raw
        .query_row("PRAGMA quick_check", [], |r| r.get(0))?,
    )
  }
  pub fn poll_changes(&self) -> Result<bool> {
    self.recheck_read_access()?;
    let version = self
      .conn
      .raw
      .query_row("PRAGMA data_version", [], |r| r.get(0))?;
    let old = self.data_version.replace(version);
    Ok(old != 0 && old != version)
  }
  pub(crate) fn has_building(&self, ids: &[i64]) -> Result<bool> {
    for id in ids {
      let s: String = self.conn.raw.query_row(
        "SELECT status FROM root_state WHERE root_id=?1",
        params![id],
        |r| r.get(0),
      )?;
      if s == "building" {
        return Ok(true);
      }
    }
    Ok(false)
  }
  pub fn enqueue(&self, kind: &str, config: &SearchConfig) -> Result<i64> {
    if !matches!(kind, "rebuild" | "refresh" | "repair") {
      return Err(IndexError::StaleScan);
    }
    let payload = serde_json::to_string(
      &config
        .roots
        .iter()
        .map(|r| r.path.clone())
        .collect::<Vec<_>>(),
    )
    .map_err(|e| std::io::Error::other(e.to_string()))?;
    let tx = self.conn.begin_writer_txn(false)?;
    tx.conn.raw.execute(
      "INSERT INTO requests(kind,payload,created_by,created_at) VALUES(?1,?2,?3,?4)",
      params![kind, payload, self.consumer, unix_now()],
    )?;
    let id = tx.conn.raw.last_insert_rowid();
    block_on(tx.commit())?;
    Ok(id)
  }
  pub fn recover_pending_roots(&mut self) -> Result<()> {
    self.ensure_lease()?;
    let roots = {
      let mut q=self.conn.raw.prepare("SELECT r.path,c.consumer FROM roots r JOIN root_state s ON s.root_id=r.id JOIN root_consumers c ON c.root_id=r.id WHERE r.enabled=1 AND c.enabled=1 AND s.status IN ('building','dirty','catching_up') ORDER BY r.id,c.priority DESC,c.consumer")?;
      let rows = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
      rows
    };
    let mut seen = HashSet::new();
    for (path, consumer) in roots {
      self.check_cancelled()?;
      if !seen.insert(path.clone()) {
        continue;
      }
      let previous = std::mem::replace(&mut self.consumer, consumer);
      let result = self.rebuild(&SearchConfig {
        roots: vec![SearchRoot::new(path)],
      });
      self.consumer = previous;
      result?;
    }
    Ok(())
  }
  pub fn drain_requests(&mut self) -> Result<()> {
    self.ensure_lease()?;
    let items = {
      let mut s=self.conn.raw.prepare("SELECT id,kind,payload,created_by FROM requests WHERE state='pending' ORDER BY id LIMIT 16")?;
      let rows = s
        .query_map([], |r| {
          Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
          ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
      rows
    };
    for (id, kind, payload, consumer) in items {
      self.check_cancelled()?;
      let paths: Vec<PathBuf> =
        serde_json::from_str(&payload).map_err(|e| std::io::Error::other(e.to_string()))?;
      self.consumer = consumer;
      // Inbox root state is authoritative only through persisted membership;
      // client-supplied enable/allowance values cannot grant admission.
      let mut trusted = Vec::new();
      for path in paths {
        if let Ok(r)=self.conn.raw.query_row("SELECT r.path,c.priority,c.include_hidden,c.include_ignored FROM roots r JOIN root_consumers c ON c.root_id=r.id WHERE r.path=?1 AND c.consumer=?2 AND c.enabled=1",params![path.to_string_lossy(),self.consumer],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i32>(1)?,r.get::<_,bool>(2)?,r.get::<_,bool>(3)?))){let mut root=SearchRoot::new(r.0);root.priority=r.1;root.include_hidden=r.2;root.include_ignored=r.3;trusted.push(root);}
      }
      let config = SearchConfig { roots: trusted };
      let result = match kind.as_str() {
        "rebuild" => self.rebuild(&config).map(|_| ()),
        "refresh" => self.refresh(&config).map(|_| ()),
        "repair" => self.repair(&config).map(|_| ()),
        _ => Err(IndexError::StaleScan),
      };
      if matches!(
        result,
        Err(IndexError::ReadOnly | IndexError::SchemaTooNew { .. } | IndexError::Cancelled)
      ) {
        return result;
      }
      let tx = self.conn.begin_writer_txn(false)?;
      tx.conn.raw.execute(
        "UPDATE requests SET state=?2 WHERE id=?1",
        params![id, if result.is_ok() { "done" } else { "failed" }],
      )?;
      block_on(tx.commit())?;
    }
    Ok(())
  }
  pub fn migrate(&mut self, target: i64) -> Result<()> {
    if target > self.conn.minor {
      return Err(IndexError::ReadOnly);
    }
    self.ensure_lease()?;
    let h = self.home.as_ref().ok_or(IndexError::LeaseRequired)?;
    let _g = h
      .lock("migrate.lock", true)?
      .ok_or(IndexError::LeaseRequired)?;
    let tx = self.conn.begin_writer_txn(true)?;
    let current: i64 =
      tx.conn
        .raw
        .query_row("SELECT value FROM meta WHERE key='schema_minor'", [], |r| {
          r.get(0)
        })?;
    if target == 1 && current == 0 {
      tx.conn
        .raw
        .execute_batch("CREATE TABLE additive_example(id INTEGER PRIMARY KEY, value TEXT);")?;
      fault("migration_ddl");
      tx.conn.raw.execute(
        "UPDATE meta SET value=1 WHERE key IN ('schema_minor','min_writer_minor')",
        [],
      )?;
    } else if current != target {
      return Err(IndexError::StaleScan);
    }
    block_on(tx.commit())
  }
}
fn nix_flags() -> i32 {
  // O_NOFOLLOW is platform specific; use nix's safe constants.
  #[cfg(target_os = "macos")]
  {
    0x100
  }
  #[cfg(target_os = "linux")]
  {
    0x20000
  }
}
pub struct WriterService {
  stop: Arc<AtomicBool>,
  holder: Arc<AtomicBool>,
  started: Arc<AtomicBool>,
  worker: Option<JoinHandle<()>>,
}
impl WriterService {
  pub fn is_started(&self) -> bool {
    self.started.load(Ordering::Acquire)
  }
  pub fn is_holder(&self) -> bool {
    self.holder.load(Ordering::Acquire)
  }
  pub fn start(home: Home) -> Self {
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    let holder = Arc::new(AtomicBool::new(false));
    let acquired = holder.clone();
    let started = Arc::new(AtomicBool::new(false));
    let attempted = started.clone();
    let worker = thread::spawn(move || {
      while !signal.load(Ordering::Acquire) {
        if let Ok(mut index) = KuntuIndex::open_shared(home.clone(), false, "service") {
          index.cancel = signal.clone();
          attempted.store(true, Ordering::Release);
          if index.acquire_writer().unwrap_or(false) {
            acquired.store(true, Ordering::Release);
            while !signal.load(Ordering::Acquire) {
              let result = index
                .drain_requests()
                .and_then(|()| index.recover_pending_roots());
              if matches!(
                result,
                Err(IndexError::ReadOnly | IndexError::SchemaTooNew { .. } | IndexError::Cancelled)
              ) {
                index.conn.lease.borrow_mut().take();
                break;
              }
              for _ in 0..10 {
                if signal.load(Ordering::Acquire) {
                  break;
                }
                thread::sleep(Duration::from_millis(10));
              }
            }
          }
        }
        acquired.store(false, Ordering::Release);
        // Five seconds is the poll interval; shutdown checks do not clamp it.
        for _ in 0..50 {
          if signal.load(Ordering::Acquire) {
            break;
          }
          thread::sleep(Duration::from_millis(100));
        }
      }
    });
    Self {
      stop,
      holder,
      started,
      worker: Some(worker),
    }
  }
}
impl Drop for WriterService {
  fn drop(&mut self) {
    self.stop.store(true, Ordering::Release);
    if let Some(t) = self.worker.take() {
      let _ = t.join();
    }
  }
}

pub(crate) fn sqlite_runtime_supported(number: i32) -> bool {
  number >= 3_051_003
}

struct ImportSnapshot {
  home: Home,
  created: Vec<String>,
}
impl Drop for ImportSnapshot {
  fn drop(&mut self) {
    for name in &self.created {
      let _ = self.home.remove(name);
    }
  }
}
