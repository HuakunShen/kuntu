//! Descriptor-relative store layout. Every shared entry point keeps Home alive.
use fs4::fs_std::FileExt;
use nix::fcntl::{open, openat, OFlag};
use nix::sys::stat::{fchmod, fstat, mkdirat, Mode};
use nix::unistd::{getuid, unlinkat, UnlinkatFlags};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
type DirectoryRegistry =
  std::sync::Mutex<std::collections::HashMap<(u64, u64), std::sync::Weak<File>>>;
/// Selected by the host, independently of the database filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMode {
  Private,
  Shared,
}
pub const EPOCH: u32 = 3;
pub type Result<T> = std::io::Result<T>;
fn err(s: impl std::fmt::Display) -> std::io::Error {
  std::io::Error::other(s.to_string())
}
#[derive(Clone, Debug)]
pub struct Home {
  dir: Arc<File>,
  pub path: PathBuf,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Identity {
  format: u32,
  store_id: String,
  created_by: String,
}
pub fn resolve() -> Result<PathBuf> {
  if let Some(p) = std::env::var_os("KUNTU_HOME") {
    if p.is_empty() {
      return Err(err("Empty KUNTU_HOME."));
    }
    return Ok(p.into());
  }
  Ok(PathBuf::from(std::env::var_os("HOME").ok_or_else(|| err("HOME is absent."))?).join(".kuntu"))
}
fn validate(f: &File, private: bool) -> Result<()> {
  let s = fstat(f).map_err(err)?;
  if (private && (s.st_uid != getuid().as_raw() || s.st_mode & 0o777 != 0o700))
    || (!private && s.st_uid != 0 && s.st_uid != getuid().as_raw())
    || (!private && s.st_mode & 0o002 != 0 && s.st_mode & 0o1000 == 0)
  {
    return Err(err("Untrusted directory ownership or mode."));
  }
  Ok(())
}
fn directory(parent: &File, name: &std::ffi::OsStr, create: bool) -> Result<File> {
  if create {
    match mkdirat(parent, name, Mode::from_bits_truncate(0o700)) {
      Ok(()) | Err(nix::errno::Errno::EEXIST) => {}
      Err(e) => return Err(err(format!("create-directory: {e}"))),
    }
  }
  let f: OwnedFd = openat(
    parent,
    name,
    OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
    Mode::empty(),
  )
  .map_err(|e| err(format!("open-directory: {e}")))?;
  Ok(f.into())
}
// Retry only transient creation failure through the same validated fd.
fn create_lock_file(parent: &File, mut open: impl FnMut() -> nix::Result<OwnedFd>) -> Result<File> {
  let mut attempts = 0;
  loop {
    match open() {
      Ok(fd) => return Ok(fd.into()),
      Err(nix::errno::Errno::ENOENT) if attempts < 100 => {
        attempts += 1;
        std::thread::sleep(std::time::Duration::from_millis(1));
        validate(parent, true)?;
      }
      Err(e) => return Err(err(format!("open-lock-file: {e}"))),
    }
  }
}
pub fn allowed_fs(name: &str) -> bool {
  matches!(name, "apfs" | "hfs")
}
#[cfg(target_os = "macos")]
fn local(f: &File) -> Result<()> {
  let s = nix::sys::statfs::fstatfs(f).map_err(err)?;
  if allowed_fs(s.filesystem_type_name()) {
    Ok(())
  } else {
    Err(err("Unsupported local filesystem."))
  }
}
#[cfg(target_os = "linux")]
fn local(f: &File) -> Result<()> {
  let n = nix::sys::statfs::fstatfs(f)
    .map_err(err)?
    .filesystem_type()
    .0;
  if matches!(n, 0xEF53 | 0x58465342 | 0x9123683E | 0x01021994) {
    Ok(())
  } else {
    Err(err("Unsupported local filesystem."))
  }
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn local(_: &File) -> Result<()> {
  Err(err("Shared stores are unsupported on this platform."))
}
impl Home {
  pub fn open(path: &Path) -> Result<Self> {
    let _local = bootstrap_process_guard()?;
    if !path.is_absolute() {
      return Err(err("Store home must be absolute."));
    }
    let mut dir: File = open(
      "/",
      OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
      Mode::empty(),
    )
    .map_err(err)?
    .into();
    let names: Vec<_> = path
      .components()
      .filter_map(|c| {
        if let Component::Normal(n) = c {
          Some(n)
        } else {
          None
        }
      })
      .collect();
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
      return Err(err("Parent traversal refused."));
    }
    for (i, n) in names.iter().enumerate() {
      validate(&dir, false)?;
      dir = directory(&dir, n, i + 1 == names.len())?;
    }
    validate(&dir, true)?;
    local(&dir)?;
    // SQLite may share a Unix shm node between connections. All Homes of
    // this inode reuse one directory descriptor until the last closes.
    static DIRS: std::sync::OnceLock<DirectoryRegistry> = std::sync::OnceLock::new();
    let stat = fstat(&dir).map_err(err)?;
    let key = (stat.st_dev as u64, stat.st_ino);
    let mut dirs = DIRS
      .get_or_init(Default::default)
      .lock()
      .map_err(|_| err("Directory registry poisoned."))?;
    dirs.retain(|_, v| v.strong_count() > 0);
    let dir = dirs
      .get(&key)
      .and_then(std::sync::Weak::upgrade)
      .unwrap_or_else(|| Arc::new(dir));
    dirs.insert(key, Arc::downgrade(&dir));
    drop(dirs);
    let home = Self {
      dir,
      path: path.into(),
    };
    for n in ["locks", "run", "logs"] {
      let d = directory(&home.dir, std::ffi::OsStr::new(n), true)?;
      validate(&d, true)?;
    }
    let _bootstrap = home
      .lock("bootstrap.lock", true)?
      .ok_or_else(|| err("Bootstrap lock unavailable."))?;
    if match home.read_file("store.json") {
      Ok(_) => false,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
      Err(e) => return Err(e),
    } {
      let mut random = [0u8; 16];
      getrandom::fill(&mut random).map_err(err)?;
      let identity = Identity {
        format: 1,
        store_id: random.iter().map(|b| format!("{b:02x}")).collect(),
        created_by: "kuntu 0.6".into(),
      };
      let mut temp = home.file("store.json.tmp", true)?;
      temp.set_len(0)?;
      serde_json::to_writer(&mut temp, &identity).map_err(err)?;
      temp.sync_all()?;
      fault("identity_synced");
      home.rename("store.json.tmp", "store.json")?;
      home.dir.sync_all()?;
    }
    let _: Identity = serde_json::from_reader(home.file("store.json", false)?).map_err(err)?;
    Ok(home)
  }
  pub fn id(&self) -> Result<String> {
    let i: Identity = serde_json::from_reader(self.file("store.json", false)?).map_err(err)?;
    if i.format != 1 {
      return Err(err("Store format too new."));
    }
    Ok(i.store_id)
  }
  pub fn read_file(&self, name: &str) -> Result<File> {
    if name.contains('/') || name.contains("..") {
      return Err(err("Invalid leaf."));
    }
    let f: File = openat(
      &*self.dir,
      name,
      OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
      Mode::empty(),
    )
    .map_err(std::io::Error::from)?
    .into();
    let s = fstat(&f).map_err(err)?;
    if s.st_mode & 0o170000 != 0o100000 {
      return Err(err("Not a regular file."));
    }
    Ok(f)
  }
  pub fn file(&self, name: &str, create: bool) -> Result<File> {
    if name.contains('/') || name.contains("..") {
      return Err(err("Invalid leaf."));
    }
    let flags = OFlag::O_RDWR
      | OFlag::O_NOFOLLOW
      | OFlag::O_CLOEXEC
      | if create {
        OFlag::O_CREAT
      } else {
        OFlag::empty()
      };
    let f: File = openat(&*self.dir, name, flags, Mode::from_bits_truncate(0o600))
      .map_err(std::io::Error::from)?
      .into();
    let s = fstat(&f).map_err(err)?;
    if s.st_uid != getuid().as_raw() || s.st_mode & 0o170000 != 0o100000 || s.st_nlink != 1 {
      return Err(err("Untrusted file."));
    }
    fchmod(&f, Mode::from_bits_truncate(0o600)).map_err(err)?;
    Ok(f)
  }
  /// Create a new private inode. Existing files are never truncated.
  pub fn create_new_file(&self, name: &str) -> Result<File> {
    if name.contains('/') || name.contains("..") {
      return Err(err("Invalid leaf."));
    }
    let f: File = openat(
      &*self.dir,
      name,
      OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
      Mode::from_bits_truncate(0o600),
    )
    .map_err(std::io::Error::from)?
    .into();
    Ok(f)
  }
  pub fn exists(&self, name: &str) -> Result<bool> {
    match self.read_file(name) {
      Ok(_) => Ok(true),
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
      Err(e) => Err(e),
    }
  }
  pub fn sqlite_name(&self, name: &str) -> Result<String> {
    if name.contains('/') || name.contains("..") {
      return Err(err("Invalid leaf."));
    }
    Ok(format!("/kuntu-fd/{}/{name}", self.dir.as_raw_fd()))
  }
  pub fn lock(&self, name: &str, wait: bool) -> Result<Option<Lease>> {
    let locks = directory(&self.dir, std::ffi::OsStr::new("locks"), false)?;
    validate(&locks, true)?;
    if name.contains('/') || name.contains("..") {
      return Err(err("Invalid lock leaf."));
    }
    let mut fd = create_lock_file(&locks, || {
      openat(
        &locks,
        name,
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
      )
    })?;
    let s = fstat(&fd).map_err(|e| err(format!("lock-file: {e}")))?;
    if s.st_nlink != 1 || s.st_uid != getuid().as_raw() || s.st_mode & 0o777 != 0o600 {
      return Err(err("Invalid permanent lock inode."));
    }
    if wait {
      fd.lock_exclusive()?;
    } else if !fd.try_lock_exclusive()? {
      return Ok(None);
    }
    if name == "indexer.lock" {
      fd.set_len(0)?;
      serde_json::to_writer(&mut fd,&serde_json::json!({"pid":std::process::id(),"started_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| err(format!("lock-file: {e}")))?.as_secs(),"version":"0.6.0","exe":std::env::current_exe()?.to_string_lossy(),"schema_minor":1})).map_err(|e| err(format!("lock-file: {e}")))?;
      fd.sync_data()?;
    }
    Ok(Some(Lease {
      file: fd,
      name: name.into(),
      home: self.clone(),
    }))
  }
  pub fn lifetime(&self, epoch: u32) -> Result<ReaderGuard> {
    let lock = format!("epoch-{epoch}.lock");
    let locks = directory(&self.dir, std::ffi::OsStr::new("locks"), false)?;
    let f = create_lock_file(&locks, || {
      openat(
        &locks,
        lock.as_str(),
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
      )
    })?;
    let stat = fstat(&f).map_err(err)?;
    if stat.st_nlink != 1 || stat.st_uid != getuid().as_raw() || stat.st_mode & 0o777 != 0o600 {
      return Err(err("Invalid epoch lock inode."));
    }
    f.lock_shared()?;
    if epoch < EPOCH && self.exists(&format!("gc-{epoch}.json"))? {
      FileExt::unlock(&f)?;
      let guard = self
        .lock(&format!("epoch-{epoch}.lock"), true)?
        .ok_or_else(|| err("Missing epoch capability."))?;
      if self.exists(&format!("gc-{epoch}.json"))? {
        self.resume_gc(epoch, &guard)?;
      }
      drop(guard);
      // Re-enter to acquire shared access and re-check after the lock wait.
      return self.lifetime(epoch);
    }
    let mut registration = self.file(&format!("lifetime-{epoch}"), true)?;
    registration.write_all(b"1")?;
    registration.sync_data()?;
    match self.remove(&format!("unused-{epoch}")) {
      Ok(()) => {}
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
      Err(e) => return Err(e),
    }
    Ok(ReaderGuard {
      file: f,
      home: self.clone(),
      epoch,
    })
  }
  pub fn rename(&self, a: &str, b: &str) -> Result<()> {
    if [a, b].iter().any(|p| p.contains('/') || p.contains("..")) {
      return Err(err("Invalid rename."));
    }
    nix::fcntl::renameat(&*self.dir, a, &*self.dir, b).map_err(|e| err(format!("rename-file: {e}")))
  }
  pub fn remove(&self, n: &str) -> Result<()> {
    unlinkat(&*self.dir, n, UnlinkatFlags::NoRemoveDir).map_err(std::io::Error::from)
  }
  pub fn quarantine(&self, epoch: u32) -> Result<()> {
    if epoch != EPOCH {
      return Err(err("Only the current epoch can be quarantined."));
    }
    let _lease = self
      .lock("indexer.lock", false)?
      .ok_or_else(|| err("Live writer."))?;
    let _epoch = self
      .lock(&format!("epoch-{epoch}.lock"), false)?
      .ok_or_else(|| err("Live reader."))?;
    self.resume_quarantine(epoch, &_lease, &_epoch)
  }
  pub fn resume_quarantine(&self, epoch: u32, writer: &Lease, lifetime: &Lease) -> Result<()> {
    if epoch != EPOCH
      || !writer.belongs_to(self)
      || !lifetime.belongs_to(self)
      || writer.name != "indexer.lock"
      || lifetime.name != format!("epoch-{epoch}.lock")
    {
      return Err(err(
        "Quarantine requires owned writer and epoch capabilities.",
      ));
    }
    let marker = format!("quarantine-{epoch}.json");
    let suffix = self.operation_marker(&marker)?;
    for (ext, point) in [
      ("", "quarantine_main_moved"),
      ("-wal", "quarantine_wal_moved"),
      ("-shm", "quarantine_shm_moved"),
    ] {
      let src = format!("index.v{epoch}.db{ext}");
      match self.file(&src, false) {
        Ok(_) => self.rename(&src, &format!("quarantine-{suffix}{ext}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
      }
      self.dir.sync_all()?;
      namespace_event(point);
      fault(point);
      fault("quarantine_renamed");
    }
    self.finish_operation(&marker)?;
    Ok(())
  }
  /// Atomic marker publication is durable before the first member operation.
  fn operation_marker(&self, marker: &str) -> Result<String> {
    if !self.exists(marker)? {
      let temporary = format!("{marker}.tmp");
      let mut random = [0; 16];
      getrandom::fill(&mut random).map_err(err)?;
      let suffix = random
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
      let mut f = self.file(&temporary, true)?;
      f.set_len(0)?;
      f.write_all(suffix.as_bytes())?;
      f.sync_all()?;
      namespace_event("marker_file_synced");
      self.rename(&temporary, marker)?;
      self.dir.sync_all()?;
      namespace_event("marker_directory_synced");
      fault("namespace_marker_published");
    }
    let mut suffix = String::new();
    self.read_file(marker)?.read_to_string(&mut suffix)?;
    if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
      return Err(err("Invalid durable operation marker."));
    }
    Ok(suffix)
  }
  fn finish_operation(&self, marker: &str) -> Result<()> {
    self.dir.sync_all()?;
    namespace_event("members_directory_synced");
    self.remove(marker)?;
    self.dir.sync_all()?;
    namespace_event("marker_removal_synced");
    Ok(())
  }
  fn resume_gc(&self, epoch: u32, guard: &Lease) -> Result<()> {
    if epoch >= EPOCH || guard.name != format!("epoch-{epoch}.lock") || !guard.belongs_to(self) {
      return Err(err("GC requires an older epoch capability."));
    }
    let marker = format!("gc-{epoch}.json");
    self.operation_marker(&marker)?;
    for (ext, point) in [
      ("", "gc_main_removed"),
      ("-wal", "gc_wal_removed"),
      ("-shm", "gc_shm_removed"),
    ] {
      match self.remove(&format!("index.v{epoch}.db{ext}")) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
      }
      self.dir.sync_all()?;
      namespace_event(point);
      fault(point);
    }
    match self.remove(&format!("unused-{epoch}")) {
      Ok(()) => {}
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
      Err(e) => return Err(e),
    }
    self.finish_operation(&marker)
  }
  pub fn gc_epoch(&self, epoch: u32, now: u64) -> Result<bool> {
    if epoch >= EPOCH {
      return Ok(false);
    }
    // Private legacy epochs never joined the shared lifetime protocol.
    // Their non-use cannot be established; preserve them fail-closed.
    match self.read_file(&format!("lifetime-{epoch}")) {
      Ok(mut f) => {
        let mut v = String::new();
        f.read_to_string(&mut v)?;
        if v != "1" {
          return Err(err("Unknown epoch lifetime protocol."));
        }
      }
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
      Err(e) => return Err(e),
    }
    let Some(_g) = self.lock(&format!("epoch-{epoch}.lock"), false)? else {
      return Ok(false);
    };
    if self.exists(&format!("gc-{epoch}.json"))? {
      self.resume_gc(epoch, &_g)?;
      return Ok(true);
    }
    let stamp = format!("unused-{epoch}");
    let mut f = self.file(&stamp, true)?;
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    let t = s.parse::<u64>().ok();
    if t.is_none() {
      write!(f, "{now}")?;
      f.sync_all()?;
      return Ok(false);
    }
    if now.saturating_sub(t.unwrap()) < 30 * 86400 {
      return Ok(false);
    }
    self.resume_gc(epoch, &_g)?;
    Ok(true)
  }
}
#[derive(Debug)]
pub struct Lease {
  file: File,
  name: String,
  home: Home,
}
impl Lease {
  pub fn belongs_to(&self, home: &Home) -> bool {
    let a = fstat(&*self.home.dir).ok();
    let b = fstat(&*home.dir).ok();
    a.zip(b)
      .is_some_and(|(a, b)| a.st_ino == b.st_ino && a.st_dev == b.st_dev)
  }
}
impl Drop for Lease {
  fn drop(&mut self) {
    let _ = FileExt::unlock(&self.file);
  }
}
#[derive(Debug)]
pub struct ReaderGuard {
  file: File,
  home: Home,
  epoch: u32,
}
impl Drop for ReaderGuard {
  fn drop(&mut self) {
    let _ = self.home.remove(&format!("unused-{}", self.epoch));
    let _ = FileExt::unlock(&self.file);
  }
}

fn fault(point: &str) {
  if std::env::var("KUNTU_FAULT").ok().as_deref() == Some(point) {
    if let Ok(p) = std::env::var("KUNTU_FAULT_MARKER") {
      std::fs::write(p, point).expect("fault handshake");
    }
    loop {
      std::thread::sleep(std::time::Duration::from_millis(10));
    }
  }
}

/// Same-process initialization is serialized as well as cross-process flock.
pub fn bootstrap_process_guard() -> Result<std::sync::MutexGuard<'static, ()>> {
  static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
  GUARD.lock().map_err(|_| err("Bootstrap mutex poisoned."))
}

#[cfg(test)]
mod tests;

/// Read-only source directory; opening it never creates layout or identity files.
pub struct SourceDir {
  dir: File,
}
impl SourceDir {
  pub fn open(path: &Path) -> Result<Self> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
      return Err(err("Invalid source directory."));
    }
    let mut dir: File = open(
      "/",
      OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
      Mode::empty(),
    )
    .map_err(err)?
    .into();
    for c in path.components() {
      if let Component::Normal(n) = c {
        validate(&dir, false)?;
        dir = directory(&dir, n, false)?;
      }
    }
    validate(&dir, false)?;
    Ok(Self { dir })
  }
  pub fn read_file(&self, name: &str) -> Result<File> {
    if name.contains('/') || name.contains("..") {
      return Err(err("Invalid source leaf."));
    }
    let f: File = openat(
      &self.dir,
      name,
      OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
      Mode::empty(),
    )
    .map_err(std::io::Error::from)?
    .into();
    if fstat(&f).map_err(err)?.st_mode & 0o170000 != 0o100000 {
      return Err(err("Source is not a regular file."));
    }
    Ok(f)
  }
}

fn namespace_event(event: &str) {
  #[cfg(test)]
  tests::record_namespace_event(event);
  #[cfg(not(test))]
  let _ = event;
}
