//! Regression contracts for the shared-store review; no sleep-only ordering.
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier};
fn home(name: &str) -> Home {
  let p = PathBuf::from(
    std::env::var_os("KUNTU_TEST_DIR").expect("KUNTU_TEST_DIR under scratch required"),
  );
  std::fs::create_dir_all(&p).unwrap();
  let p = p.join(format!(
    "{name}-{}-{}",
    std::process::id(),
    SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos()
  ));
  Home::open(&p).unwrap()
}
fn fixture() -> (Home, SearchConfig, KuntuIndex) {
  let h = home("fixture");
  let root = h.path.join("root");
  std::fs::create_dir(&root).unwrap();
  std::fs::write(root.join("visible-file.md"), "v").unwrap();
  std::fs::write(root.join(".hidden-file.md"), "h").unwrap();
  let config = SearchConfig {
    roots: vec![SearchRoot::new(root)],
  };
  let mut i = KuntuIndex::open_shared(h.clone(), false, "strict").unwrap();
  i.register_root(&config.roots[0]).unwrap();
  assert!(i.acquire_writer().unwrap());
  i.rebuild(&config).unwrap();
  (h, config, i)
}
#[test]
fn two_concurrent_first_opens_publish_one_random_identity() {
  let h = home("first");
  let p = h.path.join("new-home");
  let b = Arc::new(Barrier::new(2));
  let tasks: Vec<_> = (0..2)
    .map(|_| {
      let b = b.clone();
      let p = p.clone();
      std::thread::spawn(move || {
        b.wait();
        let h = Home::open(&p).unwrap();
        let i = KuntuIndex::open_shared(h.clone(), false, "a").unwrap();
        assert_eq!(i.doctor().unwrap(), "ok");
        h.id().unwrap()
      })
    })
    .collect();
  let ids: Vec<_> = tasks.into_iter().map(|t| t.join().unwrap()).collect();
  assert_eq!(ids[0], ids[1]);
  assert_eq!(ids[0].len(), 32);
}
#[test]
fn lease_capability_blocks_second_writer_and_payload_is_not_authority() {
  let (h, c, i) = fixture();
  let mut second = KuntuIndex::open_shared(h.clone(), false, "strict").unwrap();
  assert!(!second.acquire_writer().unwrap());
  assert!(matches!(second.rebuild(&c), Err(IndexError::LeaseRequired)));
  let mut payload = h.read_file("store.json").unwrap();
  use std::io::Read;
  let mut text = String::new();
  payload.read_to_string(&mut text).unwrap();
  assert!(text.contains("store_id"));
  drop(i);
  assert!(second.acquire_writer().unwrap());
  second.rebuild(&c).unwrap();
}
#[test]
fn actual_opens_enforce_skew_before_wal_and_old_writer_steps_down() {
  let h = home("skew");
  let mut old = KuntuIndex::open_shared_minor(h.clone(), false, "old", 0).unwrap();
  assert!(old.acquire_writer().unwrap());
  let reader = KuntuIndex::open_shared_minor(h.clone(), true, "reader", 0).unwrap();
  drop(old);
  let mut new = KuntuIndex::open_shared(h.clone(), false, "new").unwrap();
  assert!(new.acquire_writer().unwrap());
  assert_eq!(reader.doctor().unwrap(), "ok");
  let old = KuntuIndex::open_shared_minor(h.clone(), false, "old", 0).unwrap();
  assert_eq!(old.access(), Access::ReadOnly);
  assert!(matches!(
    old.enqueue("rebuild", &SearchConfig::default()),
    Err(IndexError::ReadOnly)
  ));
  new
    .conn
    .raw
    .execute("UPDATE meta SET value=2 WHERE key='min_reader_minor'", [])
    .unwrap();
  drop(old);
  drop(reader);
  drop(new);
  let before = std::fs::read(h.path.join("index.v3.db")).unwrap();
  assert!(matches!(
    KuntuIndex::open_shared(h.clone(), true, "old"),
    Err(IndexError::SchemaTooNew { .. })
  ));
  assert_eq!(before, std::fs::read(h.path.join("index.v3.db")).unwrap());
}
#[test]
fn every_write_transaction_rechecks_minor_and_rolls_back() {
  let (h, c, i) = fixture();
  let old = KuntuIndex::open_shared_minor(h.clone(), false, "old", 1).unwrap();
  i.conn
    .raw
    .execute("UPDATE meta SET value=2 WHERE key='min_writer_minor'", [])
    .unwrap();
  assert!(matches!(
    old.register_root(&c.roots[0]),
    Err(IndexError::ReadOnly)
  ));
  assert_eq!(old.access(), Access::ReadOnly);
  assert!(old.conn.raw.is_autocommit());
}
#[test]
fn strict_consumer_never_inherits_union_or_client_flags() {
  let (h, c, mut i) = fixture();
  let permissive = KuntuIndex::open_shared(h.clone(), false, "permissive").unwrap();
  let mut root = c.roots[0].clone();
  root.include_hidden = true;
  root.include_ignored = true;
  permissive.register_root(&root).unwrap();
  i.rebuild(&c).unwrap();
  let mut q = SearchQuery::new("file");
  q.include_hidden = true;
  q.include_ignored = true;
  let s = i.search_with_metrics(&c, &q).unwrap();
  assert!(s.results.iter().all(|r| !r
    .path
    .file_name()
    .unwrap()
    .to_string_lossy()
    .starts_with('.')));
  let p = permissive.search_with_metrics(&c, &q).unwrap();
  assert!(p.results.iter().any(|r| r
    .path
    .file_name()
    .unwrap()
    .to_string_lossy()
    .starts_with('.')));
  assert!(matches!(
    i.search_with_metrics(&SearchConfig::default(), &SearchQuery::new("")),
    Err(IndexError::EmptyScope)
  ));
  let foreign = SearchConfig {
    roots: vec![SearchRoot::new(h.path.join("foreign"))],
  };
  assert!(matches!(
    i.search_with_metrics(&foreign, &q),
    Err(IndexError::EmptyScope)
  ));
}
#[test]
fn consumer_removal_and_disable_invalidate_inflight_generation() {
  let (h, c, i) = fixture();
  let id = i
    .conn
    .raw
    .query_row("SELECT id FROM roots", [], |r| r.get(0))
    .unwrap();
  let gen = i.begin_scan(id).unwrap();
  let second = KuntuIndex::open_shared(h, false, "strict").unwrap();
  second.remove_consumer(id).unwrap();
  assert!(matches!(
    i.complete_scan(id, gen),
    Err(IndexError::StaleScan)
  ));
  let mut disabled = c.roots[0].clone();
  disabled.enabled = false;
  second.register_root(&disabled).unwrap();
  assert!(matches!(i.check_scan(id, gen), Err(IndexError::StaleScan)));
}
#[test]
fn restart_uses_fresh_generation_stale_completion_is_rejected_and_building_is_reported() {
  let (_h, c, i) = fixture();
  let id = i
    .conn
    .raw
    .query_row("SELECT id FROM roots", [], |r| r.get(0))
    .unwrap();
  let gen = i.begin_scan(id).unwrap();
  assert_eq!(gen + 1, i.begin_scan(id).unwrap());
  assert!(matches!(
    i.complete_scan(id, gen - 1),
    Err(IndexError::StaleScan)
  ));
  assert_eq!(
    i.search_with_metrics(&c, &SearchQuery::new("file"))
      .unwrap()
      .coverage,
    CandidateCoverage::Building
  );
  assert_eq!(
    i.search_with_metrics(&c, &SearchQuery::new("zzzz"))
      .unwrap()
      .coverage,
    CandidateCoverage::Building
  );
}
#[test]
fn batch_generation_is_atomic_and_readers_observe_data_version() {
  let (h, c, i) = fixture();
  let r = KuntuIndex::open_shared(h, true, "strict").unwrap();
  r.poll_changes().unwrap();
  let before = r.generation_probe().unwrap();
  i.register_root(&c.roots[0]).unwrap();
  assert!(r.generation_probe().unwrap() > before);
  assert!(r.poll_changes().unwrap());
  assert!(!r.poll_changes().unwrap());
}
#[test]
fn production_builders_pin_both_postings_reverse_and_rowid() {
  let i = KuntuIndex::open_memory().unwrap();
  for name in [true, false] {
    let sql = postings_sql(name);
    let mut q = i
      .conn
      .raw
      .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
      .unwrap();
    let p = q
      .query_map(params!["a", "b", 10], |r| r.get::<_, String>(3))
      .unwrap()
      .collect::<rusqlite::Result<Vec<_>>>()
      .unwrap()
      .join("|");
    assert!(p.contains("PRIMARY KEY"), "{p}");
    assert!(!p.contains("SCAN"), "{p}");
  }
  let mut q = i
    .conn
    .raw
    .prepare(&format!("EXPLAIN QUERY PLAN {}", entry_fetch_sql(2)))
    .unwrap();
  let p = q
    .query_map(params![1, 2], |r| r.get::<_, String>(3))
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
    .join("|");
  assert!(p.contains("rowid=?"));
  let mut q = i
    .conn
    .raw
    .prepare(&format!("EXPLAIN QUERY PLAN {}", entry_terms_sql()))
    .unwrap();
  let p = q
    .query_map(params![1], |r| r.get::<_, String>(3))
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
    .join("|");
  assert!(p.contains("rowid=?"), "{p}");
  for name in [true, false] {
    let mut s = i
      .conn
      .raw
      .prepare(&format!("EXPLAIN QUERY PLAN {}", postings_delete_sql(name)))
      .unwrap();
    let p = s
      .query_map(params![1, if name { 1 } else { 2 }], |r| {
        r.get::<_, String>(3)
      })
      .unwrap()
      .collect::<rusqlite::Result<Vec<_>>>()
      .unwrap()
      .join("|");
    assert!(p.contains("PRIMARY KEY") && p.contains("rowid=?"), "{p}");
  }
}
#[test]
fn opened_directory_survives_path_swap_and_sidecars_are_private() {
  let (h, _c, i) = fixture();
  let original = h.path.clone();
  let moved = original.with_extension("moved");
  std::fs::rename(&original, &moved).unwrap();
  std::os::unix::fs::symlink(&moved, &original).unwrap();
  assert!(Home::open(&original).is_err());
  let r = KuntuIndex::open_shared(h, true, "strict").unwrap();
  assert_eq!(r.doctor().unwrap(), "ok");
  for name in ["index.v3.db", "index.v3.db-wal", "index.v3.db-shm"] {
    assert_eq!(
      std::fs::metadata(moved.join(name))
        .unwrap()
        .permissions()
        .mode()
        & 0o777,
      0o600
    );
  }
  drop(i);
}
#[test]
fn live_readers_prevent_quarantine_and_gc_never_touches_future_epochs() {
  let (h, _c, i) = fixture();
  assert!(h.quarantine(3).is_err());
  drop(i);
  let reader = h.lifetime(2).unwrap();
  assert!(!h.gc_epoch(2, 40 * 86400).unwrap());
  assert!(!h.gc_epoch(3, u64::MAX).unwrap());
  assert!(!h.gc_epoch(4, u64::MAX).unwrap());
  assert!(!h.path.join("locks/epoch-4.lock").exists());
  assert!(!h.path.join("unused-4").exists());
  drop(reader);
  assert!(!h.gc_epoch(2, 1).unwrap());
  assert!(h.gc_epoch(2, 31 * 86400).unwrap());
}
#[test]
fn local_fs_allow_list_is_fail_closed() {
  assert!(kuntu_store::allowed_fs("apfs"));
  for n in ["nfs", "smbfs", "fuse", "unknown", ""] {
    assert!(!kuntu_store::allowed_fs(n));
  }
}
#[test]
fn bundled_runtime_includes_wal_reset_fix() {
  assert!(shared::sqlite_runtime_supported(3_051_003));
  assert!(!shared::sqlite_runtime_supported(3_051_002));
  assert!(shared::sqlite_runtime_supported(rusqlite::version_number()));
  eprintln!("SQLITE runtime={}", rusqlite::version());
}
#[test]
fn real_turso_v2_import_rebuilds_and_preserves_source_sidecars() {
  let h = home("import");
  let source = Home::open(&h.path.join("source")).unwrap();
  let root = h.path.join("files");
  std::fs::create_dir(&root).unwrap();
  std::fs::write(root.join("import-file.md"), "x").unwrap();
  let path = source.path.join("v2.db");
  let db = block_on(turso::Builder::new_local(path.to_str().unwrap()).build()).unwrap();
  let conn = db.connect().unwrap();
  block_on(conn.execute_batch(
    "CREATE TABLE roots(id INTEGER PRIMARY KEY,path TEXT,enabled INTEGER);PRAGMA user_version=2;",
  ))
  .unwrap();
  block_on(conn.execute(
    "INSERT INTO roots VALUES(1,?1,1)",
    turso::params![root.to_string_lossy()],
  ))
  .unwrap();
  drop(conn);
  drop(db);
  let before: Vec<_> = ["", "-wal", "-shm"]
    .into_iter()
    .map(|e| std::fs::read(source.path.join(format!("v2.db{e}"))).ok())
    .collect();
  let mut i = KuntuIndex::open_shared(h, false, "importer").unwrap();
  assert!(i.acquire_writer().unwrap());
  let s = i.import_v2(&path).unwrap();
  assert!(s.entries > 0);
  let c = SearchConfig {
    roots: vec![SearchRoot::new(root)],
  };
  assert!(i
    .search(&c, &SearchQuery::new("import file"))
    .unwrap()
    .iter()
    .any(|r| r.path.ends_with("import-file.md")));
  let after: Vec<_> = ["", "-wal", "-shm"]
    .into_iter()
    .map(|e| std::fs::read(source.path.join(format!("v2.db{e}"))).ok())
    .collect();
  assert_eq!(before, after);
}

#[test]
fn joined_takeover_service_releases_lease_only_after_worker_stops() {
  let h = home("joined-service");
  let _ = KuntuIndex::open_shared(h.clone(), false, "service").unwrap();
  let service = WriterService::start(h.clone());
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
  while !service.is_holder() {
    assert!(std::time::Instant::now() < deadline);
    std::thread::yield_now();
  }
  assert!(h.lock("indexer.lock", false).unwrap().is_none());
  drop(service);
  assert!(h.lock("indexer.lock", false).unwrap().is_some());
}

thread_local! {static BATCH_ROWS:std::cell::RefCell<Vec<usize>>=const{std::cell::RefCell::new(Vec::new())};}
pub(super) fn record_batch(rows: usize) {
  BATCH_ROWS.with(|r| r.borrow_mut().push(rows));
}
#[test]
fn slow_scan_rows_force_the_100_ms_batch_boundary() {
  let h = home("slow-batch");
  let root = h.path.join("files");
  std::fs::create_dir(&root).unwrap();
  for n in 0..100 {
    std::fs::write(root.join(format!("file-{n}.md")), "x").unwrap();
  }
  let c = SearchConfig {
    roots: vec![SearchRoot::new(root)],
  };
  let mut i = KuntuIndex::open_shared(h, false, "slow").unwrap();
  i.register_root(&c.roots[0]).unwrap();
  assert!(i.acquire_writer().unwrap());
  BATCH_ROWS.with(|r| r.borrow_mut().clear());
  struct Delay;
  impl Drop for Delay {
    fn drop(&mut self) {
      std::env::remove_var("KUNTU_TEST_ROW_DELAY_MS");
    }
  }
  std::env::set_var("KUNTU_TEST_ROW_DELAY_MS", "20");
  let _delay = Delay;
  i.rebuild(&c).unwrap();
  BATCH_ROWS.with(|r| {
    let r = r.borrow();
    assert!(r.len() > 1);
    assert!(r.iter().all(|n| *n <= 6), "rows per slow batch: {r:?}");
  });
}

#[test]
fn requested_scope_cannot_widen_config_even_on_an_early_return() {
  let (h, c, i) = fixture();
  let foreign = SearchRoot::new(h.path.join("foreign"));
  let id = i.register_root(&foreign).unwrap();
  let scope = kuntu_core::SearchScope::new(vec![id]).unwrap();
  let mut q = SearchQuery::new("");
  q.include_files = false;
  q.include_directories = false;
  assert!(matches!(
    i.search_scoped(&c, &scope, &q, SearchOptions::default()),
    Err(IndexError::EmptyScope)
  ));
}
#[test]
fn restart_recovery_waits_for_live_holder_then_resumes_persisted_generation() {
  let (h, c, i) = fixture();
  let id = i
    .conn
    .raw
    .query_row("SELECT id FROM roots", [], |r| r.get(0))
    .unwrap();
  let gen = i.begin_scan(id).unwrap();
  let before = i.generation_probe().unwrap();
  let service = WriterService::start(h.clone());
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
  while !service.is_started() {
    assert!(std::time::Instant::now() < deadline);
    std::thread::yield_now();
  }
  assert!(!service.is_holder());
  assert_eq!(i.generation_probe().unwrap(), before);
  assert_eq!(i.root_state_name(&c.roots[0].path).unwrap(), "building");
  drop(service);
  drop(i);
  let service = WriterService::start(h.clone());
  let reader = KuntuIndex::open_shared(h, true, "strict").unwrap();
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
  while reader.root_state_name(&c.roots[0].path).unwrap() != "ready" {
    assert!(std::time::Instant::now() < deadline);
    std::thread::yield_now();
  }
  assert_eq!(
    reader
      .conn
      .raw
      .query_row("SELECT active_scan_gen FROM root_state", [], |r| r
        .get::<_, i64>(0))
      .unwrap(),
    gen + 1
  );
  drop(service);
}

#[test]
fn private_open_cannot_bypass_a_shared_store_lease() {
  let (h, c, _i) = fixture();
  assert!(matches!(
    KuntuIndex::open(h.path.join("index.v3.db")),
    Err(IndexError::UnrecognizedDatabase { .. })
  ));
  let mut other = KuntuIndex::open_shared(h, false, "strict").unwrap();
  assert!(!other.acquire_writer().unwrap());
  assert!(matches!(other.rebuild(&c), Err(IndexError::LeaseRequired)));
}

#[test]
#[ignore = "1M storage and production reverse-delete evidence"]
fn realistic_fixture_reverse_delete_cost_and_size_gate() {
  let rows = std::env::var("KUNTU_BENCH_ROWS")
    .unwrap()
    .parse::<usize>()
    .unwrap();
  let f = fixture::open_or_build(rows);
  f.index
    .conn
    .raw
    .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
    .unwrap();
  let base = PathBuf::from(std::env::var_os("KUNTU_BENCH_DIR").unwrap());
  let source = base.join(format!("realistic-v3-rev2-compact-{rows}.sqlite"));
  let copy = base.join("reverse-delete-measurement.sqlite");
  drop(f);
  for suffix in ["", "-wal", "-shm"] {
    let _ = std::fs::remove_file(format!("{}{suffix}", copy.display()));
  }
  let bytes = std::fs::metadata(&source).unwrap().len();
  std::fs::copy(&source, &copy).unwrap();
  let mut i = KuntuIndex::open(&copy).unwrap();
  assert!(i.acquire_writer().unwrap());
  let root: i64 = i
    .conn
    .raw
    .query_row("SELECT id FROM roots", [], |r| r.get(0))
    .unwrap();
  let tx = i.conn.begin_writer_txn(false).unwrap();
  tx.conn
    .raw
    .execute("UPDATE entries SET scan_gen=1 WHERE id<=5000", [])
    .unwrap();
  tx.conn.raw.execute("UPDATE root_state SET status='building',active_scan_gen=0,scan_policy_revision=policy_revision WHERE root_id=?1",params![root]).unwrap();
  block_on(tx.commit()).unwrap();
  let started = std::time::Instant::now();
  let mut deleted = 0;
  let mut batches = 0;
  let mut max_ms = 0.0_f64;
  let mut cursor = 0;
  let mut inspected = 0;
  loop {
    let t = std::time::Instant::now();
    let batch = i.cleanup_stale_batch(root, 0, &mut cursor).unwrap();
    inspected += batch.inspected;
    if batch.done {
      break;
    }
    deleted += batch.deleted;
    batches += 1;
    max_ms = max_ms.max(t.elapsed().as_secs_f64() * 1000.0);
  }
  let ms = started.elapsed().as_secs_f64() * 1000.0;
  assert_eq!(deleted, 5000);
  for table in ["name_postings", "path_postings", "reverse_postings"] {
    let left: i64 = i
      .conn
      .raw
      .query_row(
        &format!("SELECT count(*) FROM {table} WHERE entry_id<=5000"),
        [],
        |r| r.get(0),
      )
      .unwrap();
    assert_eq!(left, 0);
  }
  assert_eq!(i.doctor().unwrap(), "ok");
  eprintln!(
    "REVERSE_DELETE rows={rows} database_bytes={bytes} deleted={deleted} elapsed_ms={ms:.3} batches={batches} inspected={inspected} max_call_ms={max_ms:.3}"
  );
  assert!(bytes <= 550_000_000, "550 MB size gate: {bytes} bytes");
}

#[test]
fn fd_vfs_preserves_the_unix_open_abi_for_private_connections() {
  kuntu_vfs::register().unwrap();
  let h = home("private-vfs-abi");
  let path = h.path.join("ordinary.sqlite");
  let conn = rusqlite::Connection::open(&path).unwrap();
  conn
    .execute_batch("CREATE TABLE retained(value TEXT);INSERT INTO retained VALUES('readable')")
    .unwrap();
  drop(conn);
  assert_eq!(
    std::fs::metadata(&path).unwrap().permissions().mode() & 0o600,
    0o600
  );
  assert!(!std::fs::read(path).unwrap().is_empty());
}

#[test]
fn slow_delete_triggers_force_the_100_ms_cleanup_boundary() {
  let (_, c, i) = fixture();
  let id: i64 = i
    .conn
    .raw
    .query_row("SELECT id FROM roots", [], |r| r.get(0))
    .unwrap();
  let gen = i.begin_scan(id).unwrap();
  let tx = i.conn.begin_writer_txn(false).unwrap();
  for n in 0..100 {
    tx.conn
      .raw
      .execute(
        "INSERT INTO entries(root_id,path,name,kind,scan_gen) VALUES(?1,?2,'stale',1,?3)",
        params![id, format!("/stale/{n}"), gen - 1],
      )
      .unwrap();
  }
  block_on(tx.commit()).unwrap();
  i.conn
    .raw
    .create_scalar_function(
      "slow_delete",
      0,
      rusqlite::functions::FunctionFlags::SQLITE_UTF8,
      |_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        Ok(1)
      },
    )
    .unwrap();
  let tx = i.conn.begin_writer_txn(false).unwrap();
  tx.conn.raw.execute_batch("CREATE TEMP TRIGGER slow_delete_trigger BEFORE DELETE ON entries BEGIN SELECT slow_delete(); END;").unwrap();
  block_on(tx.commit()).unwrap();
  let n = i.cleanup_stale_batch(id, gen, &mut 0).unwrap().deleted;
  assert!((1..=6).contains(&n), "one call deleted {n} slow rows");
  assert_eq!(i.root_state_name(&c.roots[0].path).unwrap(), "building");
  i.complete_scan(id, gen).unwrap();
  assert_eq!(i.root_state_name(&c.roots[0].path).unwrap(), "ready");
}

#[test]
fn caller_cannot_claim_a_future_build_minor_to_bypass_writer_floor() {
  let (h, _, i) = fixture();
  let tx = i.conn.begin_writer_txn(false).unwrap();
  tx.conn
    .raw
    .execute(
      "UPDATE meta SET value=2 WHERE key IN ('schema_minor','min_writer_minor')",
      [],
    )
    .unwrap();
  block_on(tx.commit()).unwrap();
  drop(i);
  let ordinary = KuntuIndex::open_shared(h.clone(), false, "ordinary").unwrap();
  assert_eq!(ordinary.access(), Access::ReadOnly);
  drop(ordinary);
  assert!(
    matches!(
      KuntuIndex::open_shared_minor(h, false, "claimed-future", 2),
      Err(IndexError::SchemaTooNew { .. })
    ),
    "caller-supplied build version bypassed the compiled minor floor"
  );
}

#[test]
fn restarted_walk_removes_deleted_and_renamed_committed_files() {
  let (h, c, mut index) = fixture();
  let root: i64 = index
    .conn
    .raw
    .query_row("SELECT id FROM roots", [], |r| r.get(0))
    .unwrap();
  let old = index.begin_scan(root).unwrap();
  let tx = index.conn.begin_writer_txn(false).unwrap();
  tx.conn
    .raw
    .execute("UPDATE entries SET scan_gen=?1", params![old])
    .unwrap();
  block_on(tx.commit()).unwrap();
  std::fs::remove_file(c.roots[0].path.join("visible-file.md")).unwrap();
  std::fs::rename(
    c.roots[0].path.join(".hidden-file.md"),
    c.roots[0].path.join("renamed-file.md"),
  )
  .unwrap();
  drop(index);
  index = KuntuIndex::open_shared(h, false, "strict").unwrap();
  assert!(index.acquire_writer().unwrap());
  index.rebuild(&c).unwrap();
  assert!(index
    .search(&c, &SearchQuery::new("visible file"))
    .unwrap()
    .is_empty());
  assert_eq!(
    index
      .search(&c, &SearchQuery::new("renamed file"))
      .unwrap()
      .len(),
    1
  );
  let mut fresh = KuntuIndex::open_memory().unwrap();
  fresh.rebuild(&c).unwrap();
  assert_eq!(index.snapshot().unwrap(), fresh.snapshot().unwrap());
}
#[test]
fn preopened_old_writer_rechecks_after_migration_and_releases_capability() {
  let h = home("preopened-skew");
  let old = KuntuIndex::open_shared_minor(h.clone(), false, "old", 0).unwrap();
  let preopened = Arc::new(Barrier::new(2));
  let migrated = Arc::new(Barrier::new(2));
  let a = preopened.clone();
  let b = migrated.clone();
  let thread = std::thread::spawn(move || {
    let mut old = old;
    a.wait();
    b.wait();
    assert!(matches!(old.acquire_writer(), Err(IndexError::ReadOnly)));
    assert!(
      old.conn.lease.borrow().is_none(),
      "stale opener retained capability"
    );
    old
  });
  preopened.wait();
  let mut current = KuntuIndex::open_shared(h.clone(), false, "new").unwrap();
  assert!(current.acquire_writer().unwrap());
  drop(current);
  migrated.wait();
  let old = thread.join().unwrap();
  let mut supported = KuntuIndex::open_shared(h, false, "supported").unwrap();
  assert!(supported.acquire_writer().unwrap());
  drop(old);
  // A floor raised while already holding also drops the capability at BEGIN.
  supported
    .conn
    .raw
    .execute("UPDATE meta SET value=2 WHERE key='min_writer_minor'", [])
    .unwrap();
  assert!(matches!(
    supported.conn.begin_writer_txn(false),
    Err(IndexError::ReadOnly)
  ));
  assert!(supported.conn.lease.borrow().is_none());
}
#[test]
fn self_snapshot_import_uses_unique_inodes_and_preserves_all_source_hashes() {
  let h = home("self-snapshot");
  let root = h.path.join("files");
  std::fs::create_dir(&root).unwrap();
  std::fs::write(root.join("retained.md"), "x").unwrap();
  let source = h.path.join("import-v2.db");
  let raw = rusqlite::Connection::open(&source).unwrap();
  raw
    .execute_batch("CREATE TABLE roots(path TEXT,enabled INTEGER);PRAGMA user_version=2;")
    .unwrap();
  raw
    .execute(
      "INSERT INTO roots VALUES(?1,1)",
      params![root.to_string_lossy()],
    )
    .unwrap();
  drop(raw);
  for ext in ["-wal", "-shm"] {
    std::fs::write(h.path.join(format!("import-v2.db{ext}")), []).unwrap();
  }
  let before: Vec<_> = ["", "-wal", "-shm"]
    .iter()
    .map(|e| std::fs::read(h.path.join(format!("import-v2.db{e}"))).unwrap())
    .collect();
  let mut index = KuntuIndex::open_shared(h.clone(), false, "importer").unwrap();
  assert!(index.acquire_writer().unwrap());
  assert_eq!(index.import_v2(&source).unwrap().entries, 2);
  let after: Vec<_> = ["", "-wal", "-shm"]
    .iter()
    .map(|e| std::fs::read(h.path.join(format!("import-v2.db{e}"))).unwrap())
    .collect();
  assert_eq!(before, after, "import modified its own retained source");
  assert_eq!(
    std::fs::read_dir(&h.path)
      .unwrap()
      .filter_map(|e| e.ok())
      .filter(|e| e.file_name().to_string_lossy().starts_with("import-"))
      .count(),
    3
  );
}
#[test]
fn current_sensitive_policy_denies_historical_rows_during_interrupted_reconciliation() {
  let (h, c, index) = fixture();
  let root: i64 = index
    .conn
    .raw
    .query_row("SELECT id FROM roots", [], |r| r.get(0))
    .unwrap();
  let tx = index.conn.begin_writer_txn(false).unwrap();
  let path = c.roots[0].path.join("credentials-public.md");
  tx.conn.raw.execute("INSERT INTO entries(root_id,path,name,kind,flags,scan_gen) VALUES(?1,?2,'credentials-public.md',1,0,0)",params![root,path.to_string_lossy()]).unwrap();
  block_on(insert_terms_tx(&tx, tx.conn.raw.last_insert_rowid(), &path)).unwrap();
  tx.conn
    .raw
    .execute(
      "UPDATE meta SET value='historical' WHERE key='policy_hash'",
      [],
    )
    .unwrap();
  block_on(tx.commit()).unwrap();
  let reader = KuntuIndex::open_shared(h.clone(), true, "strict").unwrap();
  assert!(
    reader
      .search(&c, &SearchQuery::new("credentials public"))
      .unwrap()
      .is_empty(),
    "trusted historical flags"
  );
  drop(index);
  let mut holder = KuntuIndex::open_shared(h.clone(), false, "strict").unwrap();
  assert!(holder.acquire_writer().unwrap());
  assert_eq!(holder.root_state_name(&c.roots[0].path).unwrap(), "dirty");
  assert_eq!(
    holder
      .conn
      .raw
      .query_row("SELECT value FROM meta WHERE key='policy_hash'", [], |r| {
        r.get::<_, String>(0)
      })
      .unwrap(),
    kuntu_core::sensitive_policy_identity()
  );
  let interrupted = holder.begin_scan(root).unwrap();
  drop(holder);
  assert!(reader
    .search(&c, &SearchQuery::new("credentials public"))
    .unwrap()
    .is_empty());
  let mut holder = KuntuIndex::open_shared(h, false, "strict").unwrap();
  assert!(holder.acquire_writer().unwrap());
  holder.recover_pending_roots().unwrap();
  assert!(matches!(
    holder.complete_scan(root, interrupted),
    Err(IndexError::StaleScan)
  ));
  assert!(!holder
    .snapshot()
    .unwrap()
    .contains_key(path.to_str().unwrap()));
  assert_eq!(holder.root_state_name(&c.roots[0].path).unwrap(), "ready");
}
#[test]
fn long_lived_rust_readers_recheck_floor_for_search_status_and_probes() {
  let (h, c, index) = fixture();
  let reader = KuntuIndex::open_shared(h, true, "strict").unwrap();
  let tx = index.conn.begin_writer_txn(false).unwrap();
  tx.conn
    .raw
    .execute("UPDATE meta SET value=2 WHERE key='min_reader_minor'", [])
    .unwrap();
  block_on(tx.commit()).unwrap();
  assert!(matches!(
    reader.search(&c, &SearchQuery::new("file")),
    Err(IndexError::SchemaTooNew { .. })
  ));
  assert!(matches!(
    reader.status(),
    Err(IndexError::SchemaTooNew { .. })
  ));
  assert!(matches!(
    reader.root_state_name(&c.roots[0].path),
    Err(IndexError::SchemaTooNew { .. })
  ));
  assert!(matches!(
    reader.poll_changes(),
    Err(IndexError::SchemaTooNew { .. })
  ));
  assert!(matches!(
    reader.point_probe(),
    Err(IndexError::SchemaTooNew { .. })
  ));
}
#[test]
fn cleanup_seek_limits_inspection_even_across_large_current_generation_gaps() {
  let (_, _, index) = fixture();
  let root: i64 = index
    .conn
    .raw
    .query_row("SELECT id FROM roots", [], |r| r.get(0))
    .unwrap();
  let gen = index.begin_scan(root).unwrap();
  let tx = index.conn.begin_writer_txn(false).unwrap();
  for n in 0..15000 {
    tx.conn
      .raw
      .execute(
        "INSERT INTO entries(root_id,path,name,kind,scan_gen) VALUES(?1,?2,'current',1,?3)",
        params![root, format!("/current/{n}"), gen],
      )
      .unwrap();
  }
  tx.conn
    .raw
    .execute(
      "INSERT INTO entries(root_id,path,name,kind,scan_gen) VALUES(?1,'/stale','stale',1,0)",
      params![root],
    )
    .unwrap();
  block_on(tx.commit()).unwrap();
  let mut cursor = 0;
  let first = index.cleanup_stale_batch(root, gen, &mut cursor).unwrap();
  assert!(first.inspected <= 5000);
  assert!(cursor <= 5000, "seek scanned past inspection budget");
  let mut q = index
    .conn
    .raw
    .prepare(&format!(
      "EXPLAIN QUERY PLAN {}",
      shared::cleanup_seek_sql()
    ))
    .unwrap();
  let plan = q
    .query_map(params![0], |r| r.get::<_, String>(3))
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
    .join("|");
  assert!(plan.contains("rowid>?") && !plan.contains("SCAN"), "{plan}");
  index.complete_scan(root, gen).unwrap();
  assert!(!index.snapshot().unwrap().contains_key("/stale"));
}

static CLEANUP_DELAY: std::sync::Mutex<Option<(PathBuf, Arc<std::sync::atomic::AtomicBool>)>> =
  std::sync::Mutex::new(None);
pub(super) fn cleanup_delay(home: Option<&Home>) {
  let hook = CLEANUP_DELAY.lock().unwrap().clone();
  if let Some((path, seen)) = hook {
    if home.is_some_and(|h| h.path == path) {
      seen.store(true, std::sync::atomic::Ordering::Release);
      std::thread::sleep(std::time::Duration::from_millis(20));
    }
  }
}
#[test]
fn shutdown_during_cleanup_is_bounded_and_readers_continue_before_takeover() {
  let (h, c, index) = fixture();
  let root: i64 = index
    .conn
    .raw
    .query_row("SELECT id FROM roots", [], |r| r.get(0))
    .unwrap();
  let tx = index.conn.begin_writer_txn(false).unwrap();
  for n in 0..100 {
    tx.conn
      .raw
      .execute(
        "INSERT INTO entries(root_id,path,name,kind,scan_gen) VALUES(?1,?2,'stale',1,0)",
        params![root, format!("/stale/{n}")],
      )
      .unwrap();
  }
  tx.conn
    .raw
    .execute("UPDATE root_state SET status='dirty',dirty=1", [])
    .unwrap();
  block_on(tx.commit()).unwrap();
  drop(index);
  let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
  *CLEANUP_DELAY.lock().unwrap() = Some((h.path.clone(), seen.clone()));
  struct Hook;
  impl Drop for Hook {
    fn drop(&mut self) {
      *CLEANUP_DELAY.lock().unwrap() = None;
    }
  }
  let _hook = Hook;
  let reader = KuntuIndex::open_shared(h.clone(), true, "strict").unwrap();
  let service = WriterService::start(h.clone());
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
  let mut probes = 0;
  let mut max = std::time::Duration::ZERO;
  while !seen.load(std::sync::atomic::Ordering::Acquire) {
    assert!(
      std::time::Instant::now() < deadline,
      "worker did not enter cleanup"
    );
    let t = std::time::Instant::now();
    reader.point_probe().unwrap();
    max = max.max(t.elapsed());
    probes += 1;
  }
  let start = std::time::Instant::now();
  drop(service);
  let shutdown = start.elapsed();
  assert!(
    shutdown < std::time::Duration::from_millis(500),
    "cleanup shutdown took {shutdown:?}"
  );
  assert!(
    probes > 0 && max < std::time::Duration::from_millis(100),
    "readers: {probes} max={max:?}"
  );
  assert_eq!(
    reader.root_state_name(&c.roots[0].path).unwrap(),
    "building"
  );
  let mut next = KuntuIndex::open_shared(h, false, "strict").unwrap();
  assert!(next.acquire_writer().unwrap());
  eprintln!(
    "CLEANUP_HANDOFF shutdown_ms={:.3} reader_probes={probes} reader_max_ms={:.3}",
    shutdown.as_secs_f64() * 1000.0,
    max.as_secs_f64() * 1000.0
  );
  drop(_hook);
  next.recover_pending_roots().unwrap();
  assert_eq!(next.root_state_name(&c.roots[0].path).unwrap(), "ready");
}
#[test]
fn holder_service_drops_unusable_capability_after_writer_floor_changes() {
  let (h, c, index) = fixture();
  drop(index);
  let control = KuntuIndex::open_shared(h.clone(), false, "strict").unwrap();
  let service = WriterService::start(h.clone());
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
  while !service.is_holder() {
    assert!(std::time::Instant::now() < deadline);
    std::thread::yield_now();
  }
  let tx = control.conn.begin_writer_txn(false).unwrap();
  tx.conn
    .raw
    .execute("UPDATE meta SET value=2 WHERE key='min_writer_minor'", [])
    .unwrap();
  block_on(tx.commit()).unwrap();
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
  while service.is_holder() {
    assert!(
      std::time::Instant::now() < deadline,
      "service retained stale lease"
    );
    std::thread::yield_now();
  }
  assert!(h.lock("indexer.lock", false).unwrap().is_some());
  control
    .conn
    .raw
    .execute("UPDATE meta SET value=1 WHERE key='min_writer_minor'", [])
    .unwrap();
  let mut next = KuntuIndex::open_shared(h, false, "strict").unwrap();
  assert!(next.acquire_writer().unwrap());
  next.rebuild(&c).unwrap();
  drop(service);
}
