//! Foundation regression tests, executable before the index port.
use super::*;
fn home() -> Home {
  let base =
    PathBuf::from(std::env::var_os("KUNTU_TEST_DIR").expect("scratch KUNTU_TEST_DIR required"));
  std::fs::create_dir_all(&base).unwrap();
  Home::open(&base.join(format!(
      "store-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
    )))
  .unwrap()
}
#[test]
fn descriptor_open_rejects_symlink_and_private_modes_are_exact() {
  use std::os::unix::fs::PermissionsExt;
  let h = home();
  assert_eq!(
    std::fs::metadata(&h.path).unwrap().permissions().mode() & 0o777,
    0o700
  );
  assert_eq!(
    h.read_file("store.json")
      .unwrap()
      .metadata()
      .unwrap()
      .permissions()
      .mode()
      & 0o777,
    0o600
  );
  std::os::unix::fs::symlink("store.json", h.path.join("alias")).unwrap();
  assert!(h.file("alias", false).is_err());
}
#[test]
fn permanent_lease_inode_is_exclusive_and_reusable() {
  use std::os::unix::fs::MetadataExt;
  let h = home();
  let first = h.lock("indexer.lock", false).unwrap().unwrap();
  assert!(h.lock("indexer.lock", false).unwrap().is_none());
  let inode = std::fs::metadata(h.path.join("locks/indexer.lock"))
    .unwrap()
    .ino();
  drop(first);
  assert!(h.lock("indexer.lock", false).unwrap().is_some());
  assert_eq!(
    std::fs::metadata(h.path.join("locks/indexer.lock"))
      .unwrap()
      .ino(),
    inode
  );
}
#[test]
fn atomic_identity_is_128_bits_and_existing_document_is_preserved() {
  let h = home();
  let before = std::fs::read(h.path.join("store.json")).unwrap();
  assert_eq!(h.id().unwrap().len(), 32);
  assert_eq!(h.id().unwrap(), Home::open(&h.path).unwrap().id().unwrap());
  assert_eq!(before, std::fs::read(h.path.join("store.json")).unwrap());
}

#[test]
fn transient_lock_creation_retries_only_not_found_under_the_same_directory_fd() {
  let home = home();
  let parent = directory(&home.dir, std::ffi::OsStr::new("locks"), false).unwrap();
  let mut first = true;
  let file = create_lock_file(&parent, || {
    if first {
      first = false;
      return Err(nix::errno::Errno::ENOENT);
    }
    openat(
      &parent,
      "retry.lock",
      OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
      Mode::from_bits_truncate(0o600),
    )
  })
  .unwrap();
  assert_eq!(fstat(&file).unwrap().st_mode & 0o777, 0o600);
  let mut calls = 0;
  assert!(create_lock_file(&parent, || {
    calls += 1;
    Err(nix::errno::Errno::EACCES)
  })
  .is_err());
  assert_eq!(calls, 1);
  assert!(home.lock("../escape", false).is_err());
}

#[test]
fn executed_child_does_not_inherit_the_permanent_writer_lease() {
  let h = home();
  let lease = h.lock("indexer.lock", false).unwrap().unwrap();
  let flags = nix::fcntl::fcntl(&lease.file, nix::fcntl::FcntlArg::F_GETFD).unwrap();
  let close_on_exec =
    nix::fcntl::FdFlag::from_bits_truncate(flags).contains(nix::fcntl::FdFlag::FD_CLOEXEC);
  let mut child = std::process::Command::new("/bin/sleep")
    .arg("1")
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .spawn()
    .unwrap();
  drop(lease);
  let reacquired = h.lock("indexer.lock", false).unwrap().is_some();
  assert!(child.wait().unwrap().success());
  assert!(close_on_exec, "the writer capability lacks FD_CLOEXEC");
  assert!(
    reacquired,
    "an executed child inherited the live writer capability"
  );
}

thread_local! {static NAMESPACE_EVENTS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };}
pub(super) fn record_namespace_event(event: &str) {
  NAMESPACE_EVENTS.with(|events| events.borrow_mut().push(event.into()));
}
#[test]
fn durable_markers_precede_every_namespace_member_and_only_enoent_is_absent() {
  let h = home();
  for ext in ["", "-wal", "-shm"] {
    h.file(&format!("index.v3.db{ext}"), true)
      .unwrap()
      .write_all(b"member")
      .unwrap();
  }
  NAMESPACE_EVENTS.with(|events| events.borrow_mut().clear());
  h.quarantine(3).unwrap();
  NAMESPACE_EVENTS.with(|events| {
    let events = events.borrow();
    assert_eq!(
      &events[..2],
      ["marker_file_synced", "marker_directory_synced"]
    );
    assert_eq!(
      &events[2..5],
      [
        "quarantine_main_moved",
        "quarantine_wal_moved",
        "quarantine_shm_moved"
      ]
    );
    assert_eq!(events.last().unwrap(), "marker_removal_synced");
  });
  // ELOOP is a refusal, never a missing member; leave the marker for recovery.
  std::os::unix::fs::symlink("store.json", h.path.join("index.v3.db")).unwrap();
  assert!(h.quarantine(3).is_err());
  assert!(h.exists("quarantine-3.json").unwrap());
  h.remove("index.v3.db").unwrap();
  h.quarantine(3).unwrap();
  assert!(!h.exists("quarantine-3.json").unwrap());
  let reader = h.lifetime(2).unwrap();
  drop(reader);
  for ext in ["", "-wal", "-shm"] {
    h.file(&format!("index.v2.db{ext}"), true).unwrap();
  }
  assert!(!h.gc_epoch(2, 1).unwrap());
  NAMESPACE_EVENTS.with(|events| events.borrow_mut().clear());
  assert!(h.gc_epoch(2, 31 * 86400).unwrap());
  NAMESPACE_EVENTS.with(|events| {
    let events = events.borrow();
    assert_eq!(
      &events[..2],
      ["marker_file_synced", "marker_directory_synced"]
    );
    assert_eq!(
      &events[2..5],
      ["gc_main_removed", "gc_wal_removed", "gc_shm_removed"]
    );
  });
}
#[test]
fn pending_gc_is_completed_before_an_old_epoch_can_be_reused() {
  let h = home();
  let reader = h.lifetime(2).unwrap();
  drop(reader);
  for ext in ["", "-wal", "-shm"] {
    h.file(&format!("index.v2.db{ext}"), true)
      .unwrap()
      .write_all(b"old")
      .unwrap();
  }
  let _ = h.operation_marker("gc-2.json").unwrap();
  h.remove("index.v2.db").unwrap();
  let _reuser = h.lifetime(2).unwrap();
  for ext in ["", "-wal", "-shm"] {
    assert!(!h.exists(&format!("index.v2.db{ext}")).unwrap());
  }
  assert!(!h.exists("gc-2.json").unwrap());
  assert!(!h.gc_epoch(3, u64::MAX).unwrap());
  assert!(!h.gc_epoch(4, u64::MAX).unwrap());
}
