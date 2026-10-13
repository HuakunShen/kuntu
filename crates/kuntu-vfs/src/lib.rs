//! Unix VFS registration. This crate is the only Rust FFI exemption.
use std::sync::OnceLock;
unsafe extern "C" {
  fn kuntu_vfs_register() -> i32;
  fn kuntu_thread_cpu_ns() -> u64;
  fn kuntu_probe_start();
  fn kuntu_probe_stop(out: *mut ProbeMetrics);
}
pub fn register() -> rusqlite::Result<()> {
  static RESULT: OnceLock<i32> = OnceLock::new();
  // SAFETY: registration runs once, after SQLite initialization. C retains
  // only static storage; it copies the Unix VFS and never retains Rust data.
  let code = *RESULT.get_or_init(|| unsafe { kuntu_vfs_register() });
  if code == 0 {
    Ok(())
  } else {
    Err(rusqlite::Error::InvalidQuery)
  }
}

/// Elapsed SQLite sleep/read/stat syscall time on the calling thread.
/// I/O wall time includes scheduling; it does not isolate device latency.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ProbeMetrics {
  pub retries: u64,
  pub sleep_ns: u64,
  pub io_ns: u64,
  pub io_calls: u64,
}
pub fn thread_cpu_ns() -> u64 {
  // SAFETY: C reads the calling thread's OS counters and retains no pointers.
  unsafe { kuntu_thread_cpu_ns() }
}
pub fn begin_probe() {
  // SAFETY: C resets only thread-local counters.
  unsafe { kuntu_probe_start() }
}
pub fn end_probe() -> ProbeMetrics {
  let mut result = ProbeMetrics::default();
  // SAFETY: repr(C) matches the four u64 fields; C writes synchronously.
  unsafe { kuntu_probe_stop(&mut result) };
  result
}
#[cfg(test)]
mod probe_tests {
  use super::*;
  #[test]
  fn probe_counters_measure_thread_cpu_and_sqlite_file_reads() {
    register().unwrap();
    let path = std::env::temp_dir().join(format!("vfs-probe-{}.sqlite", std::process::id()));
    {
      let c = rusqlite::Connection::open(&path).unwrap();
      c.execute_batch("CREATE TABLE IF NOT EXISTS t(x); INSERT INTO t VALUES(1)")
        .unwrap();
    }
    let cpu = thread_cpu_ns();
    begin_probe();
    let c = rusqlite::Connection::open(&path).unwrap();
    let _: i64 = c
      .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
      .unwrap();
    let metrics = end_probe();
    assert!(thread_cpu_ns() > cpu, "thread CPU counter did not advance");
    assert!(metrics.io_calls > 0 && metrics.io_ns > 0, "{metrics:?}");
    drop(c);
    std::fs::remove_file(path).unwrap();
  }
}

#[cfg(all(test, feature = "test-hooks"))]
mod tests {
  use super::*;
  use std::os::fd::AsRawFd;
  unsafe extern "C" {
    fn kuntu_vfs_test_delete(path: *const std::ffi::c_char) -> i32;
  }
  #[test]
  fn journal_and_wal_sync_the_pinned_directory_with_cloexec() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("vfs-sync-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.join("sync.log");
    if std::env::var_os("KUNTU_VFS_SYNC_CHILD").is_none() {
      let library = dir.join("fsync-hook.dylib");
      let mut cc = std::process::Command::new("cc");
      if cfg!(target_os = "macos") {
        cc.arg("-dynamiclib");
      } else {
        cc.args(["-shared", "-fPIC"]);
      }
      assert!(cc
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/src/test-fsync.c"))
        .arg("-o")
        .arg(&library)
        .status()
        .unwrap()
        .success());
      let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
          "journal_and_wal_sync_the_pinned_directory_with_cloexec",
          "--nocapture",
        ])
        .env("KUNTU_VFS_SYNC_CHILD", "1")
        .env("KUNTU_VFS_SYNC_DIR", &dir)
        .env("KUNTU_VFS_SYNC_LOG", &log)
        .env(
          if cfg!(target_os = "macos") {
            "DYLD_INSERT_LIBRARIES"
          } else {
            "LD_PRELOAD"
          },
          library,
        )
        .status()
        .unwrap();
      assert!(
        status.success(),
        "instrumented SQLite directory-sync regression failed"
      );
      std::fs::remove_dir_all(dir).unwrap();
      return;
    }
    let dir = std::path::PathBuf::from(std::env::var_os("KUNTU_VFS_SYNC_DIR").unwrap());
    let log = dir.join("sync.log");
    let syncs = || {
      std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .count()
    };
    register().unwrap();
    let pinned = std::fs::File::open(&dir).unwrap();
    let name = format!("/kuntu-fd/{}/sync.db", pinned.as_raw_fd());
    let c = rusqlite::Connection::open_with_flags_and_vfs(
      &name,
      rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE,
      "kuntu-fd",
    )
    .unwrap();
    c.execute_batch("PRAGMA synchronous=EXTRA; CREATE TABLE t(x)")
      .unwrap();
    // EXTRA requests both journal creation and journal deletion directory sync.
    assert!(
      syncs() >= 2,
      "journal create/delete bypassed directory fsync"
    );
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
      .unwrap();
    let before = syncs();
    c.execute_batch("INSERT INTO t VALUES(1)").unwrap();
    assert!(syncs() > before, "WAL creation bypassed directory fsync");
    // Preserve WAL on close, then exercise SQLite's sync-directory xDelete.
    let mut persist = 1_i32;
    unsafe {
      rusqlite::ffi::sqlite3_file_control(
        c.handle(),
        std::ptr::null(),
        rusqlite::ffi::SQLITE_FCNTL_PERSIST_WAL,
        (&mut persist as *mut i32).cast(),
      )
    };
    c.close().unwrap();
    assert!(dir.join("sync.db-wal").exists());
    let before = syncs();
    let wal = std::ffi::CString::new(format!("{name}-wal")).unwrap();
    assert_eq!(unsafe { kuntu_vfs_test_delete(wal.as_ptr()) }, 0);
    assert!(syncs() > before, "WAL deletion bypassed directory fsync");
    assert!(std::fs::read_to_string(log)
      .unwrap()
      .lines()
      .all(|line| line == "1"));
  }
}
