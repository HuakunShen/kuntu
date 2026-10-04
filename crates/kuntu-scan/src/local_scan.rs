//! Protected metadata-only local scanner. Never falls back to the legacy walker.

use crate::{IgnoredMode, ScanNode, ScanOptions};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[cfg(unix)]
use std::fs;
use std::io;
#[cfg(unix)]
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc, Mutex,
};
use std::time::{Duration, Instant};

const ISSUE_LIMIT: usize = 200;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalScanNode {
  pub name: String,
  pub path: PathBuf,
  pub size: u64,
  pub children: Vec<LocalScanNode>,
  pub depth: u32,
  pub ignored: bool,
  pub collapsed: bool,
  pub logical_size: u64,
  pub is_directory: bool,
  pub scan_state: String,
  pub skip_reason: Option<String>,
}

impl From<LocalScanNode> for ScanNode {
  fn from(node: LocalScanNode) -> Self {
    Self {
      name: node.name,
      path: node.path,
      size: node.size,
      children: node.children.into_iter().map(Self::from).collect(),
      depth: node.depth,
      ignored: node.ignored,
      collapsed: node.collapsed,
    }
  }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanIssue {
  pub path: PathBuf,
  pub reason: String,
  pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanCoverage {
  pub mode: String,
  pub protection: String,
  pub size_metric: String,
  /// Logical lengths per named regular file; hard-link names each contribute.
  /// Allocated bytes, unlike this metric, are deduplicated by (device, inode).
  pub logical_bytes: u64,
  pub files: u64,
  pub directories: u64,
  pub skipped_count: u64,
  pub denied_count: u64,
  pub issue_count: u64,
  pub issues_truncated: bool,
  pub issues: Vec<ScanIssue>,
  pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanVolume {
  pub path: PathBuf,
  pub total_bytes: u64,
  pub available_bytes: u64,
  pub free_bytes: u64,
  pub is_local: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalScanReport {
  pub nodes: Vec<LocalScanNode>,
  pub coverage: ScanCoverage,
  pub volumes: Vec<ScanVolume>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalScanProgress {
  pub current_path: Option<PathBuf>,
  pub bytes_scanned: u64,
  pub entries_scanned: u64,
  pub files: u64,
  pub directories: u64,
  pub skipped_count: u64,
  pub denied_count: u64,
  pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct LocalScanCancellation(Arc<AtomicBool>);
impl LocalScanCancellation {
  pub fn cancel(&self) {
    self.0.store(true, Ordering::Release);
  }
  pub fn is_cancelled(&self) -> bool {
    self.0.load(Ordering::Acquire)
  }
  fn check(&self) -> io::Result<()> {
    if self.is_cancelled() {
      Err(io::Error::new(
        io::ErrorKind::Interrupted,
        "local scan cancelled",
      ))
    } else {
      Ok(())
    }
  }
}

/// Disable materialization on the actual calling thread, before path I/O.
/// Not Send: the previous policy must be restored on the same OS thread.
struct PolicyGuard {
  #[cfg(target_os = "macos")]
  previous: i32,
  active: bool,
  _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(target_os = "macos")]
extern "C" {
  fn getiopolicy_np(kind: i32, scope: i32) -> i32;
  fn setiopolicy_np(kind: i32, scope: i32, policy: i32) -> i32;
  #[cfg_attr(not(target_arch = "aarch64"), link_name = "getmntinfo_r_np$INODE64")]
  fn getmntinfo_r_np(buffer: *mut *mut libc::statfs, flags: i32) -> i32;
}

impl PolicyGuard {
  fn enter() -> io::Result<Self> {
    #[cfg(target_os = "macos")]
    {
      // SDK sys/resource.h: VFS_MATERIALIZE_DATALESS=3, THREAD=1, OFF=1.
      let previous = unsafe { getiopolicy_np(3, 1) };
      if previous < 0 {
        return Err(io::Error::last_os_error());
      }
      if unsafe { setiopolicy_np(3, 1, 1) } != 0 {
        return Err(io::Error::last_os_error());
      }
      let mut guard = Self {
        previous,
        active: true,
        _thread: std::marker::PhantomData,
      };
      if unsafe { getiopolicy_np(3, 1) } != 1 {
        guard.restore()?;
        return Err(io::Error::new(
          io::ErrorKind::PermissionDenied,
          "materialization opt-out was not installed",
        ));
      }
      Ok(guard)
    }
    #[cfg(not(target_os = "macos"))]
    {
      Ok(Self {
        active: true,
        _thread: std::marker::PhantomData,
      })
    }
  }
  fn restore(&mut self) -> io::Result<()> {
    if !self.active {
      return Ok(());
    }
    #[cfg(target_os = "macos")]
    if unsafe { setiopolicy_np(3, 1, self.previous) } != 0 {
      return Err(io::Error::last_os_error());
    }
    self.active = false;
    Ok(())
  }
  fn finish<T>(mut self, result: io::Result<T>) -> io::Result<T> {
    self.restore()?;
    result
  }
}
impl Drop for PolicyGuard {
  fn drop(&mut self) {
    let _ = self.restore();
  }
}

#[derive(Clone, Copy)]
struct Facts {
  device: u64,
  inode: u64,
  allocated: u64,
  logical: u64,
  directory: bool,
  file: bool,
  symlink: bool,
  dataless: bool,
}

#[derive(Clone)]
struct PinnedDirectory {
  #[cfg(unix)]
  fd: Arc<std::os::fd::OwnedFd>,
}
struct OpenedRoot {
  directory: PinnedDirectory,
  facts: Facts,
  volume: ScanVolume,
}
enum Access {
  Root(OpenedRoot),
  Child(PinnedDirectory),
}

trait Backend: Sync {
  fn enter(&self) -> io::Result<PolicyGuard> {
    PolicyGuard::enter()
  }
  fn root(&self, path: &Path, cancellation: &LocalScanCancellation) -> io::Result<OpenedRoot>;
  fn metadata(&self, path: &Path, parent: &PinnedDirectory) -> io::Result<Facts>;
  fn directory(
    &self,
    path: &Path,
    parent: &PinnedDirectory,
    expected: Facts,
  ) -> io::Result<PinnedDirectory>;
  fn entries(
    &self,
    path: &Path,
    directory: &PinnedDirectory,
    cancellation: &LocalScanCancellation,
  ) -> io::Result<Vec<io::Result<PathBuf>>>;
  fn ignore_text(
    &self,
    path: &Path,
    directory: &PinnedDirectory,
    expected: Facts,
  ) -> io::Result<String>;
}

struct NativeBackend {
  #[cfg(target_os = "macos")]
  mounts: Vec<(PathBuf, bool)>,
}
impl NativeBackend {
  fn new() -> io::Result<Self> {
    #[cfg(target_os = "macos")]
    {
      // The NOWAIT cached mount table does not stat/provider-probe user paths.
      // The reentrant API owns its buffer, avoiding getmntinfo's static buffer.
      let mut buffer = std::ptr::null_mut();
      let count = unsafe { getmntinfo_r_np(&mut buffer, libc::MNT_NOWAIT) };
      if count <= 0 || buffer.is_null() {
        return Err(io::Error::last_os_error());
      }
      let rows = unsafe { std::slice::from_raw_parts(buffer, count as usize) };
      let mounts = rows
        .iter()
        .map(|row| {
          let path = unsafe { std::ffi::CStr::from_ptr(row.f_mntonname.as_ptr()) }.to_bytes();
          use std::os::unix::ffi::OsStrExt;
          (
            PathBuf::from(std::ffi::OsStr::from_bytes(path)),
            row.f_flags & libc::MNT_LOCAL as u32 != 0,
          )
        })
        .collect();
      unsafe {
        libc::free(buffer.cast());
      }
      Ok(Self { mounts })
    }
    #[cfg(not(target_os = "macos"))]
    {
      Ok(Self {})
    }
  }
  fn check_local(&self, path: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
      let local = self
        .mounts
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, local)| *local);
      if local != Some(true) {
        return Err(io::Error::new(
          io::ErrorKind::Unsupported,
          "network or unknown filesystem excluded",
        ));
      }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = path;
    Ok(())
  }
}

impl Backend for NativeBackend {
  fn root(&self, path: &Path, cancellation: &LocalScanCancellation) -> io::Result<OpenedRoot> {
    self.check_local(path)?;
    #[cfg(unix)]
    {
      use std::os::fd::{AsRawFd, FromRawFd};
      let slash = std::ffi::CString::new("/").unwrap();
      let raw = unsafe {
        libc::open(
          slash.as_ptr(),
          libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
      };
      if raw < 0 {
        return Err(io::Error::last_os_error());
      }
      let mut directory = PinnedDirectory {
        fd: Arc::new(unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) }),
      };
      volume_fd(Path::new("/"), directory.fd.as_raw_fd())?;
      let mut prefix = PathBuf::from("/");
      for component in path.components() {
        cancellation.check()?;
        let Component::Normal(name) = component else {
          continue;
        };
        prefix.push(name);
        self.check_local(&prefix)?;
        let expected = self.metadata(&prefix, &directory)?;
        if expected.symlink {
          return Err(io::Error::from_raw_os_error(libc::ELOOP));
        }
        if expected.dataless {
          return Err(policy_error(
            "dataless",
            "dataless root component was not opened",
          ));
        }
        directory = self.directory(&prefix, &directory, expected)?;
      }
      let facts = facts_fd(directory.fd.as_raw_fd())?;
      if facts.dataless {
        return Err(policy_error("dataless", "dataless root was not enumerated"));
      }
      let volume = volume_fd(path, directory.fd.as_raw_fd())?;
      Ok(OpenedRoot {
        directory,
        facts,
        volume,
      })
    }
    #[cfg(not(unix))]
    {
      let _ = cancellation;
      Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "pinned local traversal is unavailable on this platform",
      ))
    }
  }
  fn metadata(&self, path: &Path, parent: &PinnedDirectory) -> io::Result<Facts> {
    self.check_local(path)?;
    #[cfg(unix)]
    {
      use std::os::fd::AsRawFd;
      facts_at(parent.fd.as_raw_fd(), &component_name(path)?)
    }
    #[cfg(not(unix))]
    {
      let _ = parent;
      Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "pinned metadata is unavailable",
      ))
    }
  }
  fn directory(
    &self,
    path: &Path,
    parent: &PinnedDirectory,
    expected: Facts,
  ) -> io::Result<PinnedDirectory> {
    self.check_local(path)?;
    #[cfg(unix)]
    {
      use std::os::fd::{AsRawFd, FromRawFd};
      let name = component_name(path)?;
      let raw = unsafe {
        libc::openat(
          parent.fd.as_raw_fd(),
          name.as_ptr(),
          libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
      };
      if raw < 0 {
        return Err(io::Error::last_os_error());
      }
      let directory = PinnedDirectory {
        fd: Arc::new(unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) }),
      };
      let current = facts_fd(directory.fd.as_raw_fd())?;
      if !current.directory || current.device != expected.device || current.inode != expected.inode
      {
        return Err(policy_error(
          "changed-identity",
          "directory changed between metadata and open",
        ));
      }
      if current.dataless {
        return Err(policy_error(
          "dataless",
          "dataless directory was not enumerated",
        ));
      }
      volume_fd(path, directory.fd.as_raw_fd())?;
      Ok(directory)
    }
    #[cfg(not(unix))]
    {
      let _ = (parent, expected);
      Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "pinned traversal is unavailable",
      ))
    }
  }
  fn entries(
    &self,
    path: &Path,
    directory: &PinnedDirectory,
    cancellation: &LocalScanCancellation,
  ) -> io::Result<Vec<io::Result<PathBuf>>> {
    self.check_local(path)?;
    cancellation.check()?;
    #[cfg(unix)]
    {
      use std::os::fd::AsRawFd;
      use std::os::unix::ffi::OsStrExt;
      volume_fd(path, directory.fd.as_raw_fd())?;
      if facts_fd(directory.fd.as_raw_fd())?.dataless {
        return Err(policy_error(
          "dataless",
          "directory became dataless before enumeration",
        ));
      }
      let duplicate = unsafe { libc::fcntl(directory.fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
      if duplicate < 0 {
        return Err(io::Error::last_os_error());
      }
      let stream = unsafe { libc::fdopendir(duplicate) };
      if stream.is_null() {
        let error = io::Error::last_os_error();
        unsafe {
          libc::close(duplicate);
        }
        return Err(error);
      }
      struct Stream(*mut libc::DIR);
      impl Drop for Stream {
        fn drop(&mut self) {
          unsafe {
            libc::closedir(self.0);
          }
        }
      }
      let stream = Stream(stream);
      let mut entries = Vec::new();
      loop {
        cancellation.check()?;
        // The iterator and every next readdir run on this guarded OS thread.
        clear_errno();
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
          let error = io::Error::last_os_error();
          if error.raw_os_error() != Some(0) {
            entries.push(Err(error));
          }
          break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
          entries.push(Ok(path.join(std::ffi::OsStr::from_bytes(name))));
        }
      }
      Ok(entries)
    }
    #[cfg(not(unix))]
    {
      let _ = directory;
      Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "pinned enumeration is unavailable",
      ))
    }
  }
  fn ignore_text(
    &self,
    path: &Path,
    directory: &PinnedDirectory,
    expected: Facts,
  ) -> io::Result<String> {
    self.check_local(path)?;
    #[cfg(unix)]
    {
      use std::os::fd::{AsRawFd, FromRawFd};
      let name = component_name(path)?;
      let raw = unsafe {
        libc::openat(
          directory.fd.as_raw_fd(),
          name.as_ptr(),
          libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
      };
      if raw < 0 {
        return Err(io::Error::last_os_error());
      }
      let file = unsafe { fs::File::from_raw_fd(raw) };
      let current = facts_fd(file.as_raw_fd())?;
      if !current.file || current.device != expected.device || current.inode != expected.inode {
        return Err(policy_error(
          "changed-identity",
          "ignore rules changed identity, kind or filesystem",
        ));
      }
      if current.dataless {
        return Err(policy_error(
          "dataless",
          "dataless ignore rules were not read",
        ));
      }
      volume_fd(path, file.as_raw_fd())?;
      let mut text = String::new();
      file.take(1024 * 1024 + 1).read_to_string(&mut text)?;
      if text.len() > 1024 * 1024 {
        return Err(io::Error::new(
          io::ErrorKind::InvalidData,
          ".gitignore exceeds the protected 1 MiB limit",
        ));
      }
      Ok(text)
    }
    #[cfg(not(unix))]
    {
      let _ = (directory, expected);
      Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "pinned ignore reads are unavailable",
      ))
    }
  }
}

#[derive(Debug)]
struct PolicyFailure {
  reason: &'static str,
  message: String,
}
impl std::fmt::Display for PolicyFailure {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    formatter.write_str(&self.message)
  }
}
impl std::error::Error for PolicyFailure {}
fn policy_error(reason: &'static str, message: &str) -> io::Error {
  io::Error::other(PolicyFailure {
    reason,
    message: message.into(),
  })
}

#[cfg(unix)]
fn component_name(path: &Path) -> io::Result<std::ffi::CString> {
  use std::os::unix::ffi::OsStrExt;
  std::ffi::CString::new(
    path
      .file_name()
      .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing path component"))?
      .as_bytes(),
  )
  .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}
#[cfg(unix)]
fn facts_at(parent: std::os::fd::RawFd, name: &std::ffi::CStr) -> io::Result<Facts> {
  let mut stats = std::mem::MaybeUninit::<libc::stat>::uninit();
  if unsafe {
    libc::fstatat(
      parent,
      name.as_ptr(),
      stats.as_mut_ptr(),
      libc::AT_SYMLINK_NOFOLLOW,
    )
  } != 0
  {
    return Err(io::Error::last_os_error());
  }
  Ok(facts_from_stat(unsafe { &stats.assume_init() }))
}
#[cfg(unix)]
fn facts_fd(fd: std::os::fd::RawFd) -> io::Result<Facts> {
  let mut stats = std::mem::MaybeUninit::<libc::stat>::uninit();
  if unsafe { libc::fstat(fd, stats.as_mut_ptr()) } != 0 {
    return Err(io::Error::last_os_error());
  }
  Ok(facts_from_stat(unsafe { &stats.assume_init() }))
}
#[cfg(unix)]
fn facts_from_stat(stats: &libc::stat) -> Facts {
  #[cfg(target_os = "macos")]
  let dataless = stats.st_flags & 0x40000000 != 0;
  #[cfg(not(target_os = "macos"))]
  let dataless = false;
  #[allow(clippy::unnecessary_cast)]
  let (device, inode) = (stats.st_dev as u64, stats.st_ino as u64);
  Facts {
    device,
    inode,
    allocated: (stats.st_blocks.max(0) as u64).saturating_mul(512),
    logical: stats.st_size.max(0) as u64,
    directory: stats.st_mode & libc::S_IFMT == libc::S_IFDIR,
    file: stats.st_mode & libc::S_IFMT == libc::S_IFREG,
    symlink: stats.st_mode & libc::S_IFMT == libc::S_IFLNK,
    dataless,
  }
}
#[cfg(unix)]
fn clear_errno() {
  #[cfg(target_os = "macos")]
  unsafe {
    *libc::__error() = 0;
  }
  #[cfg(target_os = "linux")]
  unsafe {
    *libc::__errno_location() = 0;
  }
}
#[cfg(unix)]
fn volume_fd(path: &Path, fd: std::os::fd::RawFd) -> io::Result<ScanVolume> {
  #[cfg(any(target_os = "macos", target_os = "linux"))]
  {
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(fd, stats.as_mut_ptr()) } != 0 {
      return Err(io::Error::last_os_error());
    }
    let stats = unsafe { stats.assume_init() };
    #[cfg(target_os = "macos")]
    let is_local = stats.f_flags & libc::MNT_LOCAL as u32 != 0;
    #[cfg(target_os = "linux")]
    let is_local = matches!(
      stats.f_type as u64,
      0xef53
        | 0x58465342
        | 0x9123683e
        | 0x01021994
        | 0x794c7630
        | 0x2fc12fc1
        | 0x858458f6
        | 0x4d44
        | 0x2011bab0
    );
    if !is_local {
      return Err(policy_error(
        "non-local-volume",
        "network or unknown filesystem excluded",
      ));
    }
    #[allow(clippy::unnecessary_cast)]
    let volume = ScanVolume {
      path: path.to_path_buf(),
      total_bytes: (stats.f_blocks as u64).saturating_mul(stats.f_bsize as u64),
      available_bytes: (stats.f_bavail as u64).saturating_mul(stats.f_bsize as u64),
      free_bytes: (stats.f_bfree as u64).saturating_mul(stats.f_bsize as u64),
      is_local,
    };
    Ok(volume)
  }
  #[cfg(not(any(target_os = "macos", target_os = "linux")))]
  {
    let _ = (path, fd);
    Err(io::Error::new(
      io::ErrorKind::Unsupported,
      "local filesystem verification is unavailable",
    ))
  }
}

struct Stats {
  progress: LocalScanProgress,
  issues: Vec<ScanIssue>,
  issue_count: u64,
  last_emit: Option<Instant>,
}
struct WalkContext<'a, B, F> {
  options: &'a ScanOptions,
  backend: &'a B,
  cancellation: &'a LocalScanCancellation,
  progress: &'a F,
  started: Instant,
  stats: Mutex<Stats>,
  seen_files: Mutex<HashSet<(u64, u64)>>,
  seen_dirs: Mutex<HashSet<(u64, u64)>>,
  cloud_roots: Vec<PathBuf>,
  selected_roots: HashSet<PathBuf>,
}
impl<B: Backend, F: Fn(LocalScanProgress) + Sync> WalkContext<'_, B, F> {
  fn issue(&self, path: &Path, reason: &str, message: impl Into<String>) {
    let mut stats = self.stats.lock().unwrap();
    stats.issue_count += 1;
    stats.progress.skipped_count += 1;
    if reason == "permission-denied" {
      stats.progress.denied_count += 1;
    }
    if stats.issues.len() < ISSUE_LIMIT {
      stats.issues.push(ScanIssue {
        path: path.to_path_buf(),
        reason: reason.into(),
        message: message.into(),
      });
    }
  }
  fn emit(&self, path: Option<&Path>, force: bool) {
    let mut stats = self.stats.lock().unwrap();
    stats.progress.elapsed_ms = self.started.elapsed().as_millis() as u64;
    if let Some(path) = path {
      stats.progress.current_path = Some(path.to_path_buf());
    }
    if !force
      && stats
        .last_emit
        .is_some_and(|last| last.elapsed() < PROGRESS_INTERVAL)
    {
      return;
    }
    stats.last_emit = Some(Instant::now());
    let progress = stats.progress.clone();
    drop(stats);
    (self.progress)(progress);
  }
  fn skipped(
    &self,
    path: &Path,
    depth: u32,
    directory: bool,
    reason: &str,
    message: impl Into<String>,
  ) -> LocalScanNode {
    self.issue(path, reason, message);
    self.emit(Some(path), false);
    let mut node = new_node(path, depth, self.options.full_path);
    node.is_directory = directory;
    node.scan_state = "skipped".into();
    node.skip_reason = Some(reason.into());
    node
  }
}

pub fn scan_local_directory(options: ScanOptions) -> io::Result<LocalScanReport> {
  scan_local_directory_with_progress(options, LocalScanCancellation::default(), |_| {})
}

pub fn scan_local_directory_with_progress<F: Fn(LocalScanProgress) + Send + Sync>(
  options: ScanOptions,
  cancellation: LocalScanCancellation,
  progress: F,
) -> io::Result<LocalScanReport> {
  let guard = PolicyGuard::enter()?; // Before mount-table lookup or current_dir.
  let result = (|| {
    cancellation.check()?;
    let backend = NativeBackend::new()?;
    let threads = std::thread::available_parallelism()
      .map_or(1, |threads| threads.get())
      .min(8);
    let pool = rayon::ThreadPoolBuilder::new()
      .num_threads(threads)
      .build()
      .map_err(io::Error::other)?;
    pool.install(|| scan_with_backend(&options, &cancellation, &progress, &backend))
  })();
  guard.finish(result)
}

fn scan_with_backend<B: Backend, F: Fn(LocalScanProgress) + Sync>(
  options: &ScanOptions,
  cancellation: &LocalScanCancellation,
  progress: &F,
  backend: &B,
) -> io::Result<LocalScanReport> {
  let guard = backend.enter()?;
  let result = (|| {
    cancellation.check()?;
    if options.follow_symlinks {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "local-only scans never follow symbolic links",
      ));
    }
    let cwd = std::env::current_dir()?;
    let cloud_roots = std::env::var_os("SPACELENS_CLOUD_ROOTS")
      .map(|value| {
        std::env::split_paths(&value)
          .map(|path| normalize(&path, &cwd))
          .collect()
      })
      .unwrap_or_default();
    let mut unique = HashSet::new();
    let roots: Vec<PathBuf> = options
      .directories
      .iter()
      .map(|path| normalize(path, &cwd))
      .filter(|path| unique.insert(path.clone()))
      .collect();
    let context = WalkContext {
      options,
      backend,
      cancellation,
      progress,
      started: Instant::now(),
      stats: Mutex::new(Stats {
        progress: LocalScanProgress::default(),
        issues: Vec::new(),
        issue_count: 0,
        last_emit: None,
      }),
      seen_files: Mutex::new(HashSet::new()),
      seen_dirs: Mutex::new(HashSet::new()),
      cloud_roots,
      selected_roots: unique,
    };
    context.emit(None, true);
    cancellation.check()?;
    let mut volumes = Vec::new();
    let mut seen_volumes = HashSet::new();
    let mut nodes = Vec::new();
    for root in roots {
      cancellation.check()?;
      if cloud_path(&root, &context.cloud_roots) {
        nodes.push(context.skipped(
          &root,
          0,
          false,
          "cloud-root",
          "known cloud domain excluded before metadata",
        ));
        continue;
      }
      let opened = match backend.root(&root, cancellation) {
        Ok(opened) => opened,
        Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
        Err(error) => {
          nodes.push(context.skipped(&root, 0, false, error_reason(&error), error.to_string()));
          continue;
        }
      };
      let device = opened.facts.device;
      if seen_volumes.insert(device) {
        volumes.push(opened.volume.clone());
      }
      nodes.push(walk(
        &root,
        device,
        0,
        &[],
        false,
        &context,
        Access::Root(opened),
      )?);
    }
    cancellation.check()?;
    context.emit(None, true);
    let stats = context.stats.into_inner().unwrap();
    Ok(LocalScanReport {
      nodes,
      volumes,
      coverage: ScanCoverage {
        mode: "local-only".into(),
        protection: if cfg!(target_os = "macos") {
          "macos-no-materialization"
        } else {
          "metadata-only"
        }
        .into(),
        size_metric: "allocated".into(),
        logical_bytes: 0,
        files: stats.progress.files,
        directories: stats.progress.directories,
        skipped_count: stats.progress.skipped_count,
        denied_count: stats.progress.denied_count,
        issue_count: stats.issue_count,
        issues_truncated: stats.issue_count > ISSUE_LIMIT as u64,
        issues: stats.issues,
        elapsed_ms: stats.progress.elapsed_ms,
      },
    })
  })();
  guard.finish(result).map(|mut report| {
    report.coverage.logical_bytes = report.nodes.iter().map(|node| node.logical_size).sum();
    report
  })
}

fn walk<B: Backend, F: Fn(LocalScanProgress) + Sync>(
  path: &Path,
  device: u64,
  depth: u32,
  stack: &[Arc<Gitignore>],
  inherited_ignored: bool,
  context: &WalkContext<'_, B, F>,
  access: Access,
) -> io::Result<LocalScanNode> {
  let guard = context.backend.enter()?;
  let result = (|| {
    context.cancellation.check()?;
    if cloud_path(path, &context.cloud_roots) {
      return Ok(context.skipped(
        path,
        depth,
        false,
        "cloud-root",
        "known cloud domain excluded before metadata",
      ));
    }
    if depth > 0
      && context.options.ignore_hidden
      && path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
    {
      return Ok(context.skipped(
        path,
        depth,
        false,
        "hidden",
        "hidden entry excluded by scan options",
      ));
    }
    let facts = match &access {
      Access::Root(opened) => opened.facts,
      Access::Child(parent) => match context.backend.metadata(path, parent) {
        Ok(facts) => facts,
        Err(error) => {
          return Ok(context.skipped(path, depth, false, error_reason(&error), error.to_string()))
        }
      },
    };
    if facts.symlink {
      return Ok(context.skipped(
        path,
        depth,
        false,
        "symlink",
        "symbolic links are never followed",
      ));
    }
    if facts.dataless {
      return Ok(context.skipped(
        path,
        depth,
        facts.directory,
        "dataless",
        "dataless item excluded; materialization is disabled",
      ));
    }
    if facts.device != device {
      return Ok(context.skipped(
        path,
        depth,
        facts.directory,
        "mount-boundary",
        "child filesystem requires a separate root scan",
      ));
    }
    if !facts.directory && !facts.file {
      return Ok(context.skipped(
        path,
        depth,
        false,
        "special-file",
        "only regular files and directories are measured",
      ));
    }
    let mut ignored = inherited_ignored;
    for rules in stack {
      let matched = rules.matched(path, facts.directory);
      if matched.is_ignore() {
        ignored = true;
      } else if matched.is_whitelist() && !inherited_ignored {
        ignored = false;
      }
    }
    if ignored && context.options.ignored_mode == IgnoredMode::Exclude {
      let mut node = context.skipped(
        path,
        depth,
        facts.directory,
        "gitignored",
        "gitignored entry excluded by scan options",
      );
      node.ignored = true;
      return Ok(node);
    }
    if facts.directory
      && !context
        .seen_dirs
        .lock()
        .unwrap()
        .insert((facts.device, facts.inode))
    {
      return Ok(context.skipped(
        path,
        depth,
        true,
        "duplicate-directory",
        "directory inode was already visited",
      ));
    }
    let directory = if facts.directory {
      Some(match access {
        Access::Root(opened) => opened.directory,
        Access::Child(parent) => match context.backend.directory(path, &parent, facts) {
          Ok(directory) => directory,
          Err(error) => {
            return Ok(context.skipped(path, depth, true, error_reason(&error), error.to_string()))
          }
        },
      })
    } else {
      None
    };
    let allocated = if facts.file
      && !context
        .seen_files
        .lock()
        .unwrap()
        .insert((facts.device, facts.inode))
    {
      0
    } else {
      facts.allocated
    };
    let mut node = new_node(path, depth, context.options.full_path);
    node.size = allocated;
    node.logical_size = if facts.file { facts.logical } else { 0 };
    node.is_directory = facts.directory;
    node.ignored = ignored;
    {
      let mut stats = context.stats.lock().unwrap();
      stats.progress.entries_scanned += 1;
      if facts.directory {
        stats.progress.directories += 1;
      } else {
        stats.progress.files += 1;
      }
      stats.progress.bytes_scanned = stats.progress.bytes_scanned.saturating_add(allocated);
    }
    context.emit(Some(path), false);
    context.cancellation.check()?;
    if let Some(directory) = directory {
      let mut rules = stack.to_vec();
      if context.options.respect_gitignore && !ignored {
        let ignore_path = path.join(".gitignore");
        match context.backend.metadata(&ignore_path, &directory) {
          Ok(ignore_facts)
            if ignore_facts.file
              && !ignore_facts.symlink
              && !ignore_facts.dataless
              && ignore_facts.device == device =>
          {
            match context
              .backend
              .ignore_text(&ignore_path, &directory, ignore_facts)
            {
              Ok(text) => {
                let mut builder = GitignoreBuilder::new(path);
                for line in text.lines() {
                  if let Err(error) = builder.add_line(Some(ignore_path.clone()), line) {
                    context.issue(&ignore_path, "gitignore-error", error.to_string());
                    node.scan_state = "partial".into();
                  }
                }
                match builder.build() {
                  Ok(ruleset) => rules.push(Arc::new(ruleset)),
                  Err(error) => {
                    context.issue(&ignore_path, "gitignore-error", error.to_string());
                    node.scan_state = "partial".into();
                  }
                }
              }
              Err(error) => {
                context.issue(&ignore_path, error_reason(&error), error.to_string());
                node.scan_state = "partial".into();
              }
            }
          }
          Ok(ignore_facts) if ignore_facts.dataless => {
            context.issue(
              &ignore_path,
              "dataless",
              "dataless ignore rules were not opened",
            );
            node.scan_state = "partial".into();
          }
          Ok(_) => {}
          Err(error) if error.kind() == io::ErrorKind::NotFound => {}
          Err(error) => {
            context.issue(&ignore_path, error_reason(&error), error.to_string());
            node.scan_state = "partial".into();
          }
        }
      }
      match context
        .backend
        .entries(path, &directory, context.cancellation)
      {
        Ok(entries) => {
          let mut paths = Vec::new();
          for entry in entries {
            match entry {
              Ok(child) if context.selected_roots.contains(&child) => {
                context.issue(&child, "separate-root", "explicitly selected root is scanned separately, without a duplicate child placeholder");
                node.scan_state = "partial".into();
              }
              Ok(child) => paths.push(child),
              Err(error) => {
                context.issue(path, error_reason(&error), error.to_string());
                node.scan_state = "partial".into();
                node.skip_reason = Some(error_reason(&error).into());
              }
            }
          }
          node.children = paths
            .into_par_iter()
            .map(|path| {
              walk(
                &path,
                device,
                depth + 1,
                &rules,
                ignored,
                context,
                Access::Child(directory.clone()),
              )
            })
            .collect::<io::Result<Vec<_>>>()?;
          node
            .children
            .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
          node.size = node
            .size
            .saturating_add(node.children.iter().map(|child| child.size).sum::<u64>());
          node.logical_size = node.children.iter().map(|child| child.logical_size).sum();
          if node
            .children
            .iter()
            .any(|child| child.scan_state != "complete")
          {
            node.scan_state = "partial".into();
          }
          // Preserve ignored descendants for safe discovery from this report;
          // summarize measures them without requiring a second filesystem walk.
        }
        Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
        Err(error) => {
          context.issue(path, error_reason(&error), error.to_string());
          node.scan_state = "partial".into();
          node.skip_reason = Some(error_reason(&error).into());
        }
      }
    }
    Ok(node)
  })();
  guard.finish(result)
}

fn new_node(path: &Path, depth: u32, full_path: bool) -> LocalScanNode {
  LocalScanNode {
    name: if full_path {
      path.to_string_lossy().into_owned()
    } else {
      path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
    },
    path: path.to_path_buf(),
    size: 0,
    children: Vec::new(),
    depth,
    ignored: false,
    collapsed: false,
    logical_size: 0,
    is_directory: false,
    scan_state: "complete".into(),
    skip_reason: None,
  }
}

fn normalize(path: &Path, cwd: &Path) -> PathBuf {
  let input = if path.is_absolute() {
    path.to_path_buf()
  } else {
    cwd.join(path)
  };
  let mut result = PathBuf::new();
  for component in input.components() {
    match component {
      Component::CurDir => {}
      Component::ParentDir => {
        result.pop();
      }
      _ => result.push(component.as_os_str()),
    }
  }
  #[cfg(target_os = "macos")]
  for alias in ["/var", "/tmp", "/etc"] {
    if let Ok(suffix) = result.strip_prefix(alias) {
      return PathBuf::from("/private")
        .join(alias.trim_start_matches('/'))
        .join(suffix);
    }
  }
  result
}

fn cloud_path(path: &Path, custom: &[PathBuf]) -> bool {
  if custom.iter().any(|root| path.starts_with(root)) {
    return true;
  }
  let names: Vec<_> = path
    .components()
    .filter_map(|component| match component {
      Component::Normal(name) => Some(name.to_string_lossy().to_ascii_lowercase()),
      _ => None,
    })
    .collect();
  names.windows(2).any(|pair| {
    pair[0] == "library" && matches!(pair[1].as_str(), "cloudstorage" | "mobile documents")
  }) || names.iter().any(|name| {
    name == "dropbox"
      || name == "icloud drive"
      || name == "iclouddrive"
      || name.starts_with("onedrive")
      || name.starts_with("googledrive")
      || name.starts_with("google drive")
  })
}

fn error_reason(error: &io::Error) -> &'static str {
  if let Some(failure) = error
    .get_ref()
    .and_then(|error| error.downcast_ref::<PolicyFailure>())
  {
    return failure.reason;
  }
  #[cfg(unix)]
  if error.raw_os_error() == Some(libc::ELOOP) {
    return "symlink";
  }
  #[cfg(unix)]
  if error.raw_os_error() == Some(libc::EDEADLK) {
    return "materialization-blocked";
  }
  match error.kind() {
    io::ErrorKind::PermissionDenied => "permission-denied",
    io::ErrorKind::Unsupported => "non-local-volume",
    io::ErrorKind::NotFound => "missing",
    _ => "io-error",
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::fs;
  use std::sync::atomic::{AtomicU64, Ordering};

  struct Fixture(PathBuf);
  impl Fixture {
    fn new() -> Self {
      static NEXT: AtomicU64 = AtomicU64::new(0);
      let path = std::env::temp_dir().join(format!(
        "kuntu-local-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
      ));
      fs::create_dir_all(&path).unwrap();
      Self(fs::canonicalize(path).unwrap())
    }
    fn file(&self, name: &str, size: usize) -> PathBuf {
      let path = self.0.join(name);
      fs::create_dir_all(path.parent().unwrap()).unwrap();
      fs::write(&path, vec![b'x'; size]).unwrap();
      path
    }
    fn options(&self) -> ScanOptions {
      ScanOptions {
        directories: vec![self.0.clone()],
        ignore_hidden: false,
        full_path: true,
        respect_gitignore: false,
        ignored_mode: IgnoredMode::Summarize,
        follow_symlinks: false,
      }
    }
  }
  impl Drop for Fixture {
    fn drop(&mut self) {
      let _ = fs::remove_dir_all(&self.0);
    }
  }

  #[cfg(unix)]
  #[test]
  fn local_report_measures_logical_and_unique_allocated_bytes() {
    let fixture = Fixture::new();
    fixture.file("file", 8192);
    let report = scan_local_directory(fixture.options()).unwrap();
    assert_eq!(report.coverage.logical_bytes, 8192);
    assert_eq!(report.coverage.files, 1);
    assert_eq!(report.coverage.mode, "local-only");
    assert_eq!(report.coverage.size_metric, "allocated");
    assert_eq!(report.nodes[0].scan_state, "complete");
    assert_eq!(report.nodes[0].children[0].logical_size, 8192);
    assert!(report.volumes[0].is_local && report.volumes[0].total_bytes > 0);
  }

  #[cfg(unix)]
  #[test]
  fn hardlinks_count_allocated_blocks_once_and_sparse_files_keep_logical_length() {
    use std::os::unix::fs::MetadataExt;
    let fixture = Fixture::new();
    let file = fixture.file("file", 8192);
    fs::hard_link(&file, fixture.0.join("link")).unwrap();
    let sparse = fs::File::create(fixture.0.join("sparse")).unwrap();
    sparse.set_len(16 * 1024 * 1024).unwrap();
    let report = scan_local_directory(fixture.options()).unwrap();
    let expected = fs::metadata(&file).unwrap().blocks() * 512
      + fs::metadata(fixture.0.join("sparse")).unwrap().blocks() * 512
      + fs::metadata(&fixture.0).unwrap().blocks() * 512;
    assert_eq!(report.nodes[0].size, expected);
    assert_eq!(report.coverage.logical_bytes, 16 * 1024 * 1024 + 16384);
    assert!(report.nodes[0].logical_size > report.nodes[0].size);
  }

  #[test]
  fn cancellation_returns_interrupted_instead_of_a_successful_empty_report() {
    let fixture = Fixture::new();
    let cancellation = LocalScanCancellation::default();
    cancellation.cancel();
    assert_eq!(
      scan_local_directory_with_progress(fixture.options(), cancellation, |_| {})
        .unwrap_err()
        .kind(),
      io::ErrorKind::Interrupted
    );
  }

  #[cfg(unix)]
  #[test]
  fn progress_can_cancel_and_issues_are_bounded_without_losing_total_counts() {
    let fixture = Fixture::new();
    for ix in 0..250 {
      fixture.file(&format!("Dropbox/f{ix}"), 1);
    }
    let report = scan_local_directory(fixture.options()).unwrap();
    assert_eq!(report.coverage.skipped_count, 1);
    assert_eq!(
      report.nodes[0].children[0].skip_reason.as_deref(),
      Some("cloud-root")
    );
    let paths = (0..250)
      .map(|ix| fixture.0.join(format!("missing{ix}")))
      .collect();
    let mut options = fixture.options();
    options.directories = paths;
    let report = scan_local_directory(options).unwrap();
    assert!(report.coverage.issue_count >= 250);
    assert_eq!(report.coverage.issues.len(), 200);
    assert!(report.coverage.issues_truncated);
    let cancellation = LocalScanCancellation::default();
    let cancel = cancellation.clone();
    assert_eq!(
      scan_local_directory_with_progress(fixture.options(), cancellation, move |_| cancel.cancel())
        .unwrap_err()
        .kind(),
      io::ErrorKind::Interrupted
    );
  }

  #[cfg(unix)]
  #[test]
  fn symlink_roots_and_children_are_skipped_without_following_targets() {
    let fixture = Fixture::new();
    fixture.file("actual/file", 8192);
    std::os::unix::fs::symlink(fixture.0.join("actual"), fixture.0.join("link")).unwrap();
    let report = scan_local_directory(fixture.options()).unwrap();
    assert_eq!(report.coverage.files, 1);
    assert!(report.nodes[0]
      .children
      .iter()
      .any(|node| node.skip_reason.as_deref() == Some("symlink")));
    let mut options = fixture.options();
    options.directories = vec![fixture.0.join("link/file")];
    let report = scan_local_directory(options).unwrap();
    assert_eq!(report.coverage.files, 0);
    assert_eq!(report.nodes[0].scan_state, "skipped");
  }

  #[cfg(unix)]
  #[test]
  fn protected_gitignore_summarizes_local_ignored_entries_and_exclude_is_explicit() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join(".gitignore"), "ignored/\n").unwrap();
    fixture.file("ignored/file", 8192);
    let mut options = fixture.options();
    options.respect_gitignore = true;
    let report = scan_local_directory(options.clone()).unwrap();
    let ignored = report.nodes[0]
      .children
      .iter()
      .find(|node| node.name.ends_with("ignored"))
      .unwrap();
    assert!(ignored.ignored && !ignored.collapsed && !ignored.children.is_empty());
    assert!(ignored.size >= 8192);
    options.ignored_mode = IgnoredMode::Exclude;
    let report = scan_local_directory(options).unwrap();
    let ignored = report.nodes[0]
      .children
      .iter()
      .find(|node| node.name.ends_with("ignored"))
      .unwrap();
    assert_eq!(ignored.skip_reason.as_deref(), Some("gitignored"));
    assert_eq!(ignored.size, 0);
  }

  #[cfg(unix)]
  #[test]
  fn separately_selected_nested_roots_are_scanned_once_and_have_unique_node_paths() {
    let fixture = Fixture::new();
    fixture.file("outer", 1);
    fixture.file("nested/inner", 8192);
    let mut options = fixture.options();
    options.directories.push(fixture.0.join("nested"));
    let report = scan_local_directory(options).unwrap();
    assert_eq!(report.nodes.len(), 2);
    assert_eq!(report.nodes[1].scan_state, "complete");
    assert!(report.nodes[1]
      .children
      .iter()
      .any(|node| node.path.ends_with("inner")));
    assert!(!report.nodes[0]
      .children
      .iter()
      .any(|node| node.path.ends_with("nested")));
    assert_eq!(report.coverage.files, 2);
    assert!(report
      .coverage
      .issues
      .iter()
      .any(|issue| issue.reason == "separate-root"));
    fn collect(node: &LocalScanNode, paths: &mut HashSet<PathBuf>) {
      assert!(
        paths.insert(node.path.clone()),
        "duplicate {}",
        node.path.display()
      );
      for child in &node.children {
        collect(child, paths);
      }
    }
    let mut paths = HashSet::new();
    for node in &report.nodes {
      collect(node, &mut paths);
    }
  }

  struct ObservedBackend {
    native: NativeBackend,
    threads: Mutex<HashSet<std::thread::ThreadId>>,
    metadata_calls: AtomicU64,
    directory_calls: Mutex<Vec<PathBuf>>,
    fail_guard: bool,
    replace_parent: Option<PathBuf>,
  }
  impl ObservedBackend {
    fn new(fail_guard: bool) -> Self {
      let guard = PolicyGuard::enter().unwrap();
      let native = guard.finish(NativeBackend::new()).unwrap();
      Self {
        native,
        threads: Mutex::new(HashSet::new()),
        metadata_calls: AtomicU64::new(0),
        directory_calls: Mutex::new(Vec::new()),
        fail_guard,
        replace_parent: None,
      }
    }
    fn protected(&self) {
      #[cfg(target_os = "macos")]
      assert_eq!(
        unsafe { getiopolicy_np(3, 1) },
        1,
        "filesystem operation ran without the calling thread's opt-out"
      );
      self
        .threads
        .lock()
        .unwrap()
        .insert(std::thread::current().id());
    }
  }
  impl Backend for ObservedBackend {
    fn enter(&self) -> io::Result<PolicyGuard> {
      if self.fail_guard {
        Err(io::Error::new(
          io::ErrorKind::PermissionDenied,
          "injected guard failure",
        ))
      } else {
        PolicyGuard::enter()
      }
    }
    fn root(&self, path: &Path, cancellation: &LocalScanCancellation) -> io::Result<OpenedRoot> {
      self.protected();
      self.metadata_calls.fetch_add(1, Ordering::SeqCst);
      let mut root = self.native.root(path, cancellation)?;
      if path
        .components()
        .any(|component| component.as_os_str() == "mount")
      {
        root.facts.device += 1;
      }
      Ok(root)
    }
    fn directory(
      &self,
      path: &Path,
      parent: &PinnedDirectory,
      mut expected: Facts,
    ) -> io::Result<PinnedDirectory> {
      self.protected();
      if path
        .components()
        .any(|component| component.as_os_str() == "mount")
      {
        expected.device -= 1;
      }
      self.native.directory(path, parent, expected)
    }
    fn metadata(&self, path: &Path, parent: &PinnedDirectory) -> io::Result<Facts> {
      self.protected();
      self.metadata_calls.fetch_add(1, Ordering::SeqCst);
      if path.file_name().is_some_and(|name| name == "blocked") {
        return Err(io::Error::from_raw_os_error(libc::EDEADLK));
      }
      let mut facts = self.native.metadata(path, parent)?;
      if path.file_name().is_some_and(|name| name == "dataless") {
        facts.dataless = true;
      }
      if path
        .components()
        .any(|component| component.as_os_str() == "mount")
      {
        facts.device += 1;
      }
      Ok(facts)
    }
    fn entries(
      &self,
      path: &Path,
      directory: &PinnedDirectory,
      cancellation: &LocalScanCancellation,
    ) -> io::Result<Vec<io::Result<PathBuf>>> {
      self.protected();
      self
        .directory_calls
        .lock()
        .unwrap()
        .push(path.to_path_buf());
      #[cfg(unix)]
      if path.file_name().is_some_and(|name| name == "race-parent") {
        if let Some(replacement) = &self.replace_parent {
          fs::rename(path, path.parent().unwrap().join("held-parent")).unwrap();
          std::os::unix::fs::symlink(replacement, path).unwrap();
        }
      }
      if path.file_name().is_some_and(|name| name == "denied") {
        return Err(io::Error::new(
          io::ErrorKind::PermissionDenied,
          "injected directory denial",
        ));
      }
      if path.file_name().is_some_and(|name| name == "iteration") {
        return Ok(vec![Err(io::Error::from_raw_os_error(libc::EIO))]);
      }
      self.native.entries(path, directory, cancellation)
    }
    fn ignore_text(
      &self,
      path: &Path,
      directory: &PinnedDirectory,
      expected: Facts,
    ) -> io::Result<String> {
      self.protected();
      self.native.ignore_text(path, directory, expected)
    }
  }

  #[test]
  fn guard_failures_and_known_cloud_aliases_stop_before_path_metadata() {
    let fixture = Fixture::new();
    let backend = ObservedBackend::new(true);
    assert!(scan_with_backend(
      &fixture.options(),
      &LocalScanCancellation::default(),
      &|_| {},
      &backend
    )
    .is_err());
    assert_eq!(backend.metadata_calls.load(Ordering::SeqCst), 0);
    for root in [
      "/Users/example/Library/CloudStorage/domain",
      "/System/Volumes/Data/Users/example/Library/Mobile Documents",
      "/Volumes/GoogleDrive-1/My Drive",
      "/Users/example/Dropbox",
      "/Users/example/iCloud Drive/Documents",
      "/System/Volumes/Data/Users/example/iCloudDrive/Documents",
    ] {
      let backend = ObservedBackend::new(false);
      let mut options = fixture.options();
      options.directories = vec![PathBuf::from(root)];
      let report = scan_with_backend(
        &options,
        &LocalScanCancellation::default(),
        &|_| {},
        &backend,
      )
      .unwrap();
      assert_eq!(report.nodes[0].skip_reason.as_deref(), Some("cloud-root"));
      assert_eq!(backend.metadata_calls.load(Ordering::SeqCst), 0, "{root}");
    }
  }

  #[cfg(unix)]
  #[test]
  fn dataless_mounts_denials_and_iteration_errors_are_visible_without_descent() {
    let fixture = Fixture::new();
    for name in ["dataless", "mount", "denied", "iteration"] {
      fixture.file(&format!("{name}/file"), 8192);
    }
    fixture.file("blocked", 1);
    let backend = ObservedBackend::new(false);
    let report = scan_with_backend(
      &fixture.options(),
      &LocalScanCancellation::default(),
      &|_| {},
      &backend,
    )
    .unwrap();
    assert_eq!(report.coverage.denied_count, 1);
    assert_eq!(report.coverage.issue_count, 5);
    assert_eq!(report.coverage.files, 0);
    assert_eq!(report.nodes[0].scan_state, "partial");
    for reason in [
      "dataless",
      "mount-boundary",
      "permission-denied",
      "io-error",
      "materialization-blocked",
    ] {
      assert!(
        report
          .coverage
          .issues
          .iter()
          .any(|issue| issue.reason == reason),
        "missing {reason}"
      );
    }
    let enumerated = backend.directory_calls.lock().unwrap();
    assert!(!enumerated
      .iter()
      .any(|path| path.ends_with("dataless") || path.ends_with("mount")));
  }

  #[cfg(unix)]
  #[test]
  fn every_actual_rayon_thread_and_directory_iterator_is_protected() {
    let fixture = Fixture::new();
    for ix in 0..256 {
      fixture.file(&format!("dir{ix}/file"), 1);
    }
    let backend = ObservedBackend::new(false);
    let pool = rayon::ThreadPoolBuilder::new()
      .num_threads(4)
      .build()
      .unwrap();
    let report = pool
      .install(|| {
        scan_with_backend(
          &fixture.options(),
          &LocalScanCancellation::default(),
          &|_| {},
          &backend,
        )
      })
      .unwrap();
    assert_eq!(report.coverage.files, 256);
    assert!(backend.threads.lock().unwrap().len() >= 2);
  }

  #[cfg(unix)]
  #[test]
  fn explicitly_selected_child_mount_is_omitted_from_parent_and_scanned_as_its_own_root() {
    let fixture = Fixture::new();
    fixture.file("outer", 1);
    fixture.file("mount/inner", 8192);
    let mut options = fixture.options();
    options.directories.push(fixture.0.join("mount"));
    let backend = ObservedBackend::new(false);
    let report = scan_with_backend(
      &options,
      &LocalScanCancellation::default(),
      &|_| {},
      &backend,
    )
    .unwrap();
    assert_eq!(report.coverage.files, 2);
    assert_eq!(report.nodes[1].scan_state, "complete");
    assert!(report.nodes[1]
      .children
      .iter()
      .any(|node| node.path.ends_with("inner")));
    assert!(!report.nodes[0]
      .children
      .iter()
      .any(|node| node.path.ends_with("mount")));
  }

  #[cfg(unix)]
  #[test]
  fn progress_is_throttled_and_terminal_counts_match_measured_nodes() {
    let fixture = Fixture::new();
    for ix in 0..500 {
      fixture.file(&format!("file{ix}"), 1);
    }
    let frames = Mutex::new(Vec::new());
    let report = scan_local_directory_with_progress(
      fixture.options(),
      LocalScanCancellation::default(),
      |progress| frames.lock().unwrap().push(progress),
    )
    .unwrap();
    let frames = frames.lock().unwrap();
    assert!(frames.len() as u64 <= report.coverage.elapsed_ms / 250 + 2);
    assert_eq!(frames.last().unwrap().files, 500);
    assert_eq!(frames.last().unwrap().entries_scanned, 501);
    assert_eq!(frames.last().unwrap().bytes_scanned, report.nodes[0].size);
  }

  #[cfg(target_os = "macos")]
  #[cfg(target_os = "macos")]
  #[test]
  fn materialization_policy_is_restored_on_the_same_thread_even_when_scanning_fails() {
    let before = unsafe { getiopolicy_np(3, 1) };
    let guard = PolicyGuard::enter().unwrap();
    assert_eq!(unsafe { getiopolicy_np(3, 1) }, 1);
    let result: io::Result<()> =
      guard.finish(Err(io::Error::new(io::ErrorKind::Interrupted, "cancel")));
    assert!(result.is_err());
    assert_eq!(unsafe { getiopolicy_np(3, 1) }, before);
  }

  #[cfg(unix)]
  #[test]
  fn protected_ignore_read_does_not_follow_a_final_symlink_replaced_after_metadata() {
    let fixture = Fixture::new();
    let ignore_path = fixture.file(".gitignore", 1);
    let target = fixture.file("target", 1);
    let guard = PolicyGuard::enter().unwrap();
    let native = NativeBackend::new().unwrap();
    let root = native
      .root(&fixture.0, &LocalScanCancellation::default())
      .unwrap();
    let expected = native.metadata(&ignore_path, &root.directory).unwrap();
    fs::remove_file(&ignore_path).unwrap();
    std::os::unix::fs::symlink(target, &ignore_path).unwrap();
    assert!(native
      .ignore_text(&ignore_path, &root.directory, expected)
      .is_err());
    guard.finish(Ok(())).unwrap();
  }

  #[cfg(unix)]
  #[test]
  fn parent_directory_rename_to_symlink_cannot_redirect_enumeration_or_child_metadata() {
    let fixture = Fixture::new();
    fixture.file("race-parent/original", 8192);
    let replacement = Fixture::new();
    replacement.file("foreign", 16384);
    let mut backend = ObservedBackend::new(false);
    backend.replace_parent = Some(replacement.0.clone());
    let report = scan_with_backend(
      &fixture.options(),
      &LocalScanCancellation::default(),
      &|_| {},
      &backend,
    )
    .unwrap();
    assert_eq!(report.coverage.logical_bytes, 8192);
    let parent = report.nodes[0]
      .children
      .iter()
      .find(|node| node.path.ends_with("race-parent"))
      .unwrap();
    assert!(parent
      .children
      .iter()
      .any(|node| node.path.ends_with("original")));
    assert!(!parent
      .children
      .iter()
      .any(|node| node.path.ends_with("foreign")));
  }

  #[cfg(unix)]
  #[test]
  fn pinned_ignore_reads_keep_the_original_namespace_after_parent_is_replaced() {
    let fixture = Fixture::new();
    fixture.file("parent/.gitignore", 0);
    fs::write(fixture.0.join("parent/.gitignore"), "original-rule\n").unwrap();
    let replacement = Fixture::new();
    fs::write(replacement.0.join(".gitignore"), "foreign-rule\n").unwrap();
    let guard = PolicyGuard::enter().unwrap();
    let native = NativeBackend::new().unwrap();
    let path = fixture.0.join("parent");
    let opened = native
      .root(&path, &LocalScanCancellation::default())
      .unwrap();
    let ignore_path = path.join(".gitignore");
    let expected = native.metadata(&ignore_path, &opened.directory).unwrap();
    fs::rename(&path, fixture.0.join("held-parent")).unwrap();
    std::os::unix::fs::symlink(&replacement.0, &path).unwrap();
    assert_eq!(
      native
        .ignore_text(&ignore_path, &opened.directory, expected)
        .unwrap(),
      "original-rule\n"
    );
    assert!(native
      .root(&path, &LocalScanCancellation::default())
      .is_err());
    guard.finish(Ok(())).unwrap();
  }
}
