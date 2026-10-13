# Kuntu M2: shared store, revision 2

Implementation hand-off from `e164d67` (M1 merged). Sources: `/Volumes/Portable2TB/tmp/kuntu/PLAN.md` §5, §§8–9; `RESEARCH.md` §§4, 6, 10; round-1 review `kuntum2-plan-review.md` (2026-10-10). This revision replaces the incomplete round-1 snippets with complete files and executable fault tests. Replay evidence below distinguishes completed checks from acceptance targets.

## Frozen owner decisions

D1–D14 stay frozen. D1 SQLite WAL with bundled rusqlite; D2 M1 ships separately; D3 signed `kuntud` with embedded fallback; D4 epoch in filename and additive minors; D5 Xross keeps export mappings in its own DB; D6 5,000 candidate budget, 50,000 cap; D7 native macOS watcher in M3; D8 opt-in local text/code content in M5; D9 locale-dependent pinyin in M4; D10 submodule for development, npm for release; D11 shared file with explicit non-empty scope; D12 macOS first; D13 GC after 30 days of recorded non-use; D14 lease-held additive migrations with older writers stepping down.

| Decision | Recommended default adopted in rev 2 |
|---|---|
| O1 | Pin rusqlite `=0.40.2`, bundled SQLite 3.53.2; assert runtime ≥3.51.3. The [rusqlite release history](https://github.com/rusqlite/rusqlite/releases) records the WAL fix in 0.39.0 and SQLite 3.53.2 in 0.40.1. |
| O2 | fs4 0.13.1, permanent lock inodes, CLOEXEC. |
| O3 | 5,000 ms busy timeout on every connection. |
| O4 | At most 5,000 entries or 100 ms per scan batch. |
| O5 | Persistent 5 s takeover poll; joined shutdown before lease release. |
| O6 | Strictly older epochs only, 30 days of recorded non-use, exclusive epoch lifetime lock. |
| O7 | 64 MiB mmap cap on readers and writers; this is not an RSS bound. |
| O8 | Explicit task access and kind; old string constructor remains private. |
| O9 | Rebuild-only import, source bytes and sidecars preserved. |
| O10 | 128 random bits, stable bootstrap lock, synced private temporary document, atomic publication. |
| O11 | Fail-closed platform allow-list from `fstatfs` on the opened directory: macOS APFS/HFS; Linux ext-family/XFS/Btrfs/tmpfs. Unknown, FUSE and remote filesystems fail. |
| O12 | Compatibility before WAL/DDL, guarded write transactions, additive migrations under `migrate.lock`. |
| D15 (new, finding 7) | Hidden/ignored admission and ranking priority belong to each consumer. The indexing union never grants query admission. Default allowances are false; a stricter consumer cannot override its registration with query flags. |

## Execution contract

Only this document is committed on `docs/m2-plan`. Implementation workers start from `e164d67`; apply complete-file blocks in task order. A `file` block replaces the entire named file, including existing files; it is not a unified diff. Generated bindings are included in the write set. Never edit `kuntu-crawler`, `kuntu-watcher`, `kuntu-scan`, or `.github`. Keep all generated fixture data, logs and fault markers under `/Volumes/Portable2TB/tmp/kuntu-m2/`. Run commands in the foreground. Never push, merge or tag.

A narrowly scoped `kuntu-vfs` crate is the FFI exemption for fd-relative SQLite Unix I/O. Existing Rust crates continue to forbid unsafe code. SQLite's Unix VFS retains the DB/WAL/SHM locking and recovery implementation; the boundary substitutes descriptor-relative filesystem calls for synthetic store filenames. This is required on macOS, where `/dev/fd/<directory-fd>/child` does not work.


## Exact write set

The complete write set is the following list, including regenerated dependencies and bindings. Every file has a complete payload below; there are no instructions to improvise a port. `Cargo.lock` may be regenerated during intermediate tasks, then the final payload is installed at M2.9. No `--locked` check is promised before the complete manifests have landed. The protected crates and `.github` remain byte-for-byte unchanged.

- `Cargo.toml`
- `crates/kuntu-store/Cargo.toml`
- `crates/kuntu-store/src/lib.rs`
- `crates/kuntu-store/src/tests.rs`
- `crates/kuntu-vfs/Cargo.toml`
- `crates/kuntu-vfs/build.rs`
- `crates/kuntu-vfs/src/lib.rs`
- `crates/kuntu-vfs/src/vfs.c`
- `crates/kuntu-legacy-index/Cargo.toml`
- `crates/kuntu-legacy-index/src/lib.rs`
- `crates/kuntu-core/src/model.rs`
- `crates/kuntu-core/src/lib.rs`
- `crates/kuntu-index/Cargo.toml`
- `crates/kuntu-index/src/engine.rs`
- `crates/kuntu-index/src/shared.rs`
- `crates/kuntu-index/src/lib.rs`
- `crates/kuntu-index/src/fixture.rs`
- `crates/kuntu-index/src/m2_tests.rs`
- `crates/kuntu-index/examples/m2_probe.rs`
- `crates/kuntu-napi/Cargo.toml`
- `crates/kuntu-napi/src/lib.rs`
- `crates/kuntu-napi/index.js`
- `crates/kuntu-napi/index.d.ts`
- `crates/kuntu-napi/test/index.test.cjs`
- `crates/kuntu-cli/Cargo.toml`
- `crates/kuntu-cli/src/args.rs`
- `crates/kuntu-cli/src/main.rs`
- `crates/kuntu-daemon/Cargo.toml`
- `crates/kuntu-daemon/src/lib.rs`
- `Cargo.lock`
- `scripts/m2-previous-tag.ts`
- `scripts/m2-evidence.ts`
- `scripts/m2-red.ts`
- `scripts/m2-replay.ts`

## State and lifetime rules

Shared homes are absolute paths resolved with no-follow `openat` directory traversal. Ancestors must belong to root or the user; a user-owned mounted volume may have group-write permissions because each traversed object is opened and pinned. World-write without sticky protection is refused. The final home and `locks/run/logs` require user ownership and exact 0700. Files are 0600 at creation; existing symlinks and hard links are refused. No process-global umask changes or temporary fallback occur. One directory descriptor is reused per inode in a process, so SQLite's shared Unix shm nodes cannot outlive the descriptor encoded in their filenames.

Transient ENOENT during permanent-lock creation is retried at most 100 times through the same validated directory fd; permission/symlink failures fail immediately. Bootstrap serialization covers identity publication and first schema creation. The bootstrap, migration, indexer and epoch lock files are permanent; JSON holder payloads are informational. `KuntuIndex` owns its lease capability. Shared bulk methods refuse a missing capability; private v3 and v2 methods acquire their private lease first. `WriterService::start` is the persistent embedding entry point, used by the shared NAPI factory and the daemon. A service destructor joins its worker. The daemon uses four scoped workers and an eight-connection queue; every return joins them. Read endpoints open read-only connections; bulk requests enter the guarded inbox. The CLI retains `--db` and otherwise resolves the shared home.

The compiled `BUILD_MINOR` bounds every shared opener; the skew helper may lower it to 0 but cannot accept a caller claim above the compiled version. Compatibility is validated before WAL configuration or DDL, including read-only opens, `schema_epoch`, store identity and reader/writer floors. Initial schema creation, migrations, inbox/consumer updates, batches, cleanup and completion use `begin_writer_txn`; RAII rollback applies on error. The empty-store bootstrap exception is available only to an exclusive transaction with no existing user tables. A minor-1 holder migrates under `migrate.lock` plus an exclusive transaction; minor-0 readers keep working and minor-0 writers step down. The private v2 compatibility backend retains the old engine and its original file format. It cannot open a shared store; its guarded transactions re-check v2 user_version instead of v3 metadata.

Each scan registers `building` before crawling without holding a SQL transaction. Restart of an interrupted scan with unchanged consumer policy re-walks using the same generation and idempotent upserts; a changed policy starts a new generation. After the walk, every batch, every cleanup transaction and final completion checks the persisted generation and policy revision again. Removed/disabled consumers cannot be revived by an inbox payload. Entries, postings, progress and `meta.generation` commit together, so there is no crash interval between a batch commit and a separate generation bump. Ready is a checked final transaction after bounded stale cleanup. Readers hold no SQL transaction across calls; shared NAPI instances pool one locked reader and re-check reader floors after acquiring that lock. They use `data_version` to detect other connections' commits. Empty queries and misses on a building root report `Building`.

Name/path postings keep the M1 term-first, covering range scan; fetches keep the bare rowid `IN` projection with `id`. The reverse path is a compressed `reverse_postings` record keyed by entry ID. The lossy-path verifier reads that rowid record. Replacement and delete triggers decode its terms and delete by the term-first primary keys, avoiding an `entry_id` scan of postings. Tests explain the production builders, including the trigger's builder before `OLD.id` substitution. This replaces the space-heavy reverse indexes considered in the pilot.

Every open holds the epoch lifetime lock. Quarantine requires both the indexer lease and exclusive epoch access, publishes a synced recovery marker, moves the DB/WAL/SHM group under that marker, and resumes that sequence before reuse. GC only considers epochs strictly below 3; a live epoch holder blocks it and resets the non-use clock. Future epochs are never deleted. Unregistered old epochs are preserved fail-closed. Old binaries that do not participate in epoch lifetime coordination require a product rollout before automatic retention can be enabled; the minor matrix below simulates cooperating versions within v3. The actual previous-tag controller builds v0.4.1 from archive commit `a5df3559671b09f2af75fdd28ed5a4a06ab7dc56` and checks epoch2/epoch3 isolation and unchanged bytes on refusal. No released v3 minor-0 binary exists yet.

Import snapshots the source main file and sidecars through read-only opened directory handles into the private destination, verifies v2 on that copy, registers its enabled roots, then rebuilds from the filesystem under the destination lease. Source main/WAL/SHM bytes remain unchanged. A source must be quiescent while its three files are copied; importing a live, changing v2 writer is not an atomic snapshot protocol.

## RED/GREEN method and foreground runner

M2.1–M2.10 each have a named regression (plus deterministic lock-creation and slow-delete boundary regressions) and an exact unique-anchor defect replacement in `scripts/m2-red.ts`. The runner first installs the complete compilation prerequisites, then the task's test. RED is a **sensitivity run against the specified defect**, not a claim that a missing workspace member or an unregistered test failed. It requires a non-zero result containing the named test and an assertion failure; it rejects compile failures and zero-test passes. The original payload is restored in `finally`, then the same named test must be GREEN. These replacements are ordinary exact string replacements, never unified diffs. This ordering avoids round-1's invalid compile states while proving each test observes the missing fix.

The final replay installs every complete file from the committed plan in task order, runs all ten tasks with fourteen RED/GREEN pairs, fmt, clippy, locked workspace tests, generated NAPI checks, the 1M M1 gate, real process crash tests, 60 s stress, and the skew matrix. The controller awaits all processes and kills test children on failure; it launches no detached worker. All data, targets and logs use the scratch directory. `scripts/m2-replay.ts` is the exact entry point; extract its complete payload first, then pass a file obtained with `git show HEAD:docs/plans/M2-shared-store.md`.


## Task M2.1: descriptor-safe foundation and bootstrap

Install the workspace, store and VFS boundary, plus the private v2 compatibility backend. RED/GREEN: `descriptor_open_rejects_symlink_and_private_modes_are_exact`.

<!-- apply {"task": "M2.1", "file": "Cargo.toml", "mode": "file"} -->
````toml
# Kuntu — a local-first file & storage engine. Embeddable; no transport, no product.
[workspace]
resolver = "2"
members = [
  "crates/kuntu-core",
  "crates/kuntu-crawler",
  "crates/kuntu-index",
  "crates/kuntu-daemon",
  "crates/kuntu-watcher",
  "crates/kuntu-provider-spotlight",
  "crates/kuntu-napi",
  "crates/kuntu-cli",
  "crates/kuntu-scan",
  "crates/kuntu-store",
  "crates/kuntu-vfs",
  "crates/kuntu-legacy-index",
]

[workspace.package]
edition = "2021"
license = "MIT"
repository = "https://github.com/HuakunShen/kuntu"

[workspace.lints.rust]
unsafe_code = "forbid"

# The shipped artifact of this workspace is the kuntu-napi cdylib; without
# these the addon carries every engine symbol (22MB vs 15MB on
# darwin-arm64). Only affects this workspace's own release builds —
# consumers building these crates inside another workspace keep their
# own profile.
[profile.release]
strip = "symbols"
lto = "thin"
codegen-units = 1

[workspace.dependencies]
rusqlite = { version = "=0.40.2", features = ["bundled", "functions"] }
fs4 = "=0.13.1"
nix = { version = "=0.30.1", features = ["fs", "user"] }
getrandom = "=0.3.4"
cc = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
futures = "0.3"
turso = { version = "=0.7.2", default-features = false }
kuntu-legacy-index = { path = "crates/kuntu-legacy-index" }
kuntu-store = { path = "crates/kuntu-store" }
kuntu-vfs = { path = "crates/kuntu-vfs" }

[workspace.dependencies.zstd]
version = "=0.13.3"
````

<!-- apply {"task": "M2.1", "file": "crates/kuntu-store/Cargo.toml", "mode": "file"} -->
````toml
# Opened store directories, stable OS locks, lifetime coordination.
[package]
name = "kuntu-store"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true
[dependencies]
fs4.workspace = true
nix.workspace = true
getrandom.workspace = true
serde.workspace = true
serde_json.workspace = true
[lints]
workspace = true
````

<!-- apply {"task": "M2.1", "file": "crates/kuntu-store/src/lib.rs", "mode": "file"} -->
````rust
//! Descriptor-relative store layout. Every shared entry point keeps Home alive.
use fs4::fs_std::FileExt;
use nix::fcntl::{open, openat, OFlag};
use nix::sys::stat::{fchmod, fstat, mkdirat, Mode};
use nix::unistd::{getuid, unlinkat, UnlinkatFlags};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
type DirectoryRegistry =
  std::sync::Mutex<std::collections::HashMap<(u64, u64), std::sync::Weak<File>>>;
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
    let mut f = self.file(&marker, true)?;
    if f.metadata()?.len() == 0 {
      let mut b = [0; 16];
      getrandom::fill(&mut b).map_err(err)?;
      write!(
        f,
        "{}",
        b.iter().map(|v| format!("{v:02x}")).collect::<String>()
      )?;
      f.sync_all()?;
    }
    f.rewind()?;
    let mut suffix = String::new();
    f.read_to_string(&mut suffix)?;
    for ext in ["", "-wal", "-shm"] {
      let src = format!("index.v{epoch}.db{ext}");
      if self.file(&src, false).is_ok() {
        self.rename(&src, &format!("quarantine-{suffix}{ext}"))?;
        fault("quarantine_renamed");
      }
    }
    self.dir.sync_all()?;
    self.remove(&marker)?;
    self.dir.sync_all()?;
    Ok(())
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
    for ext in ["", "-wal", "-shm"] {
      let n = format!("index.v{epoch}.db{ext}");
      match self.remove(&n) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(2) => {}
        Err(e) => return Err(e),
      }
    }
    self.remove(&stamp)?;
    self.dir.sync_all()?;
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
````

<!-- apply {"task": "M2.1", "file": "crates/kuntu-store/src/tests.rs", "mode": "file"} -->
````rust
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
````

<!-- apply {"task": "M2.1", "file": "crates/kuntu-vfs/Cargo.toml", "mode": "file"} -->
````toml
# The single audited FFI boundary for descriptor-relative SQLite Unix I/O.
[package]
name = "kuntu-vfs"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true
[dependencies]
rusqlite.workspace = true
[build-dependencies]
cc.workspace = true
````

<!-- apply {"task": "M2.1", "file": "crates/kuntu-vfs/build.rs", "mode": "file"} -->
````rust
fn main() {
  cc::Build::new().file("src/vfs.c").compile("kuntu_vfs");
  println!("cargo:rerun-if-changed=src/vfs.c");
}
````

<!-- apply {"task": "M2.1", "file": "crates/kuntu-vfs/src/lib.rs", "mode": "file"} -->
````rust
//! Unix VFS registration. This crate is the only Rust FFI exemption.
use std::sync::OnceLock;
unsafe extern "C" {
  fn kuntu_vfs_register() -> i32;
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
````

<!-- apply {"task": "M2.1", "file": "crates/kuntu-vfs/src/vfs.c", "mode": "file"} -->
````c
/* Descriptor-relative I/O for bundled SQLite's Unix VFS. Unknown paths use
 * the original syscalls. Our synthetic paths never leave the opened home.
 * SQLite owns byte-range DB/WAL/SHM locking and recovery, unchanged. */
#include <sys/types.h>
#include <sys/stat.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
typedef struct sqlite3_vfs sqlite3_vfs;
typedef void (*syscall_ptr)(void);
struct sqlite3_vfs {
 int iVersion, szOsFile, mxPathname; sqlite3_vfs *pNext; const char *zName;
 void *pAppData;
 int (*xOpen)(sqlite3_vfs*, const char*, void*, int, int*);
 int (*xDelete)(sqlite3_vfs*,const char*,int);
 int (*xAccess)(sqlite3_vfs*,const char*,int,int*);
 int (*xFullPathname)(sqlite3_vfs*,const char*,int,char*);
 void *(*xDlOpen)(sqlite3_vfs*,const char*);
 void (*xDlError)(sqlite3_vfs*,int,char*);
 void (*(*xDlSym)(sqlite3_vfs*,void*,const char*))(void);
 void (*xDlClose)(sqlite3_vfs*,void*);
 int (*xRandomness)(sqlite3_vfs*,int,char*);
 int (*xSleep)(sqlite3_vfs*,int);
 int (*xCurrentTime)(sqlite3_vfs*,double*);
 int (*xGetLastError)(sqlite3_vfs*,int,char*);
 int (*xCurrentTimeInt64)(sqlite3_vfs*,long long*);
 int (*xSetSystemCall)(sqlite3_vfs*,const char*,syscall_ptr);
 syscall_ptr (*xGetSystemCall)(sqlite3_vfs*,const char*);
 const char *(*xNextSystemCall)(sqlite3_vfs*,const char*);
};
extern sqlite3_vfs *sqlite3_vfs_find(const char*);
extern int sqlite3_vfs_register(sqlite3_vfs*, int);
extern int sqlite3_initialize(void);
static sqlite3_vfs kv;
static int parse(const char *p, int *fd, const char **leaf) {
 if (strncmp(p,"/kuntu-fd/",10)) return 0;
 char *end; long n=strtol(p+10,&end,10);
 if (n<0 || n>2147483647 || *end!='/' || !end[1] || strchr(end+1,'/') || strstr(end+1,"..")) { errno=EINVAL; return -1; }
 *fd=(int)n; *leaf=end+1; return 1;
}
static int ko(const char *p,int flags,int mode) {
 int fd; const char *leaf; int r=parse(p,&fd,&leaf);
 if(r<0)return -1;
 if(r==0) return open(p,flags,mode);
 int f=openat(fd,leaf,flags|O_NOFOLLOW|O_CLOEXEC,0600);
 if(f>=0) { struct stat s; if(fstat(f,&s) || !S_ISREG(s.st_mode) || s.st_uid!=getuid() || s.st_nlink!=1 || (s.st_mode&0777)!=0600) {close(f);errno=EACCES;return -1;} }
 return f;
}
static int ks(const char *p,struct stat *s) {int fd;const char*l;int r=parse(p,&fd,&l);if(r<0)return -1;return r?fstatat(fd,l,s,AT_SYMLINK_NOFOLLOW):stat(p,s);}
static int kl(const char *p,struct stat *s) {int fd;const char*l;int r=parse(p,&fd,&l);if(r<0)return -1;return r?fstatat(fd,l,s,AT_SYMLINK_NOFOLLOW):lstat(p,s);}
static int ka(const char *p,int m) {int fd;const char*l;int r=parse(p,&fd,&l);if(r<0)return -1;return r?faccessat(fd,l,m,AT_SYMLINK_NOFOLLOW):access(p,m);}
static int ku(const char *p) {int fd;const char*l;int r=parse(p,&fd,&l);if(r<0)return -1;return r?unlinkat(fd,l,0):unlink(p);}
static ssize_t kr(const char *p,char *b,size_t n) {int fd;const char*l;int r=parse(p,&fd,&l);if(r<0)return -1;return r?readlinkat(fd,l,b,n):readlink(p,b,n);}
static int full(sqlite3_vfs*v,const char*p,int n,char*b) { (void)v;int fd;const char*l;if(parse(p,&fd,&l)!=1 || (int)strlen(p)>=n)return 14;strcpy(b,p);return 0; }
int kuntu_vfs_register(void) {
 if(sqlite3_initialize())return 1;
 sqlite3_vfs*u=sqlite3_vfs_find("unix");if(!u || u->iVersion<3)return 1;
 kv=*u;kv.zName="kuntu-fd";kv.pNext=0;kv.xFullPathname=full;
 if(u->xSetSystemCall(u,"open",(syscall_ptr)ko) || u->xSetSystemCall(u,"stat",(syscall_ptr)ks) || u->xSetSystemCall(u,"lstat",(syscall_ptr)kl) || u->xSetSystemCall(u,"access",(syscall_ptr)ka) || u->xSetSystemCall(u,"unlink",(syscall_ptr)ku) || u->xSetSystemCall(u,"readlink",(syscall_ptr)kr))return 1;
 return sqlite3_vfs_register(&kv,0);
}
````

<!-- apply {"task": "M2.1", "file": "crates/kuntu-legacy-index/Cargo.toml", "mode": "file"} -->
````toml
# Private v2 compatibility only. Never imported by the shared SQLite backend.
[package]
name = "kuntu-legacy-index"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true
[dependencies]
kuntu-core = { path = "../kuntu-core" }
kuntu-crawler = { path = "../kuntu-crawler" }
turso.workspace = true
futures.workspace = true
[lints]
workspace = true
````

<!-- apply {"task": "M2.1", "file": "crates/kuntu-legacy-index/src/lib.rs", "mode": "file"} -->
````rust
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
````

## Task M2.2: owned permanent lease

RED/GREEN: `permanent_lease_inode_is_exclusive_and_reusable`. Process death and CLOEXEC evidence are in M2.10. The implementation is included in the foundation file, so no unregistered module can produce a false GREEN.

## Task M2.3: complete SQLite v3 engine and M1 fixture port

Install these files together before compiling; this is the complete connection, CRUD, status, postings, fixture and core-export port. RED/GREEN: `production_builders_pin_both_postings_reverse_and_rowid`.

<!-- apply {"task": "M2.3", "file": "crates/kuntu-core/src/model.rs", "mode": "file"} -->
````rust
//! Shared data model for provider-independent file search.
//!
//! These types are intentionally plain Rust structs so CLI, daemon, desktop,
//! and future native bindings can all adapt them without pulling UI concerns
//! into the search core.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRoot {
  pub path: PathBuf,
  pub enabled: bool,
  pub priority: i32,
  pub include_hidden: bool,
  pub include_ignored: bool,
}

impl SearchRoot {
  pub fn new(path: impl Into<PathBuf>) -> Self {
    Self {
      path: normalize_root_path(path.into()),
      enabled: true,
      priority: 0,
      include_hidden: false,
      include_ignored: false,
    }
  }

  pub fn disabled(mut self) -> Self {
    self.enabled = false;
    self
  }

  pub fn with_priority(mut self, priority: i32) -> Self {
    self.priority = priority;
    self
  }

  pub fn include_hidden(mut self, include_hidden: bool) -> Self {
    self.include_hidden = include_hidden;
    self
  }

  pub fn include_ignored(mut self, include_ignored: bool) -> Self {
    self.include_ignored = include_ignored;
    self
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchConfig {
  pub roots: Vec<SearchRoot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
  pub query: String,
  pub limit: usize,
  pub include_hidden: bool,
  pub include_ignored: bool,
  /// Xross search scopes results to files or directories before ranking.
  /// Both default to true, which preserves the unfiltered behaviour; when
  /// exactly one is set, entries of the other kind are excluded at the SQL
  /// candidate stage and never crowd out the requested kind.
  pub include_files: bool,
  pub include_directories: bool,
  pub extensions: Vec<String>,
}

impl SearchQuery {
  pub fn new(query: impl Into<String>) -> Self {
    Self {
      query: query.into(),
      limit: 20,
      include_hidden: false,
      include_ignored: false,
      include_files: true,
      include_directories: true,
      extensions: Vec::new(),
    }
  }

  pub fn with_limit(mut self, limit: usize) -> Self {
    self.limit = limit;
    self
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
  File,
  Directory,
  Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCandidate {
  pub path: PathBuf,
  pub kind: EntryKind,
  pub provider: String,
}

impl SearchCandidate {
  pub fn new(path: impl Into<PathBuf>) -> Self {
    Self {
      path: path.into(),
      kind: EntryKind::Other,
      provider: "test".to_string(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchKind {
  Empty,
  ExactBasename,
  BasenamePrefix,
  BasenameToken,
  PathComponent,
  Substring,
  /// Query characters matched the filename in order after separators were removed.
  Fuzzy,
  Extension,
  RootPriority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
  pub path: PathBuf,
  pub score: i32,
  pub provider: String,
  pub matches: Vec<MatchKind>,
  /// Kind as the provider observed it. The in-memory engine knows it from the
  /// walk; index rows carry their crawled kind.
  pub kind: EntryKind,
  /// Crawled size in bytes, when the provider had it.
  pub byte_size: Option<u64>,
  /// Crawled modification time, when the provider had it.
  pub modified_unix_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecision {
  pub path: PathBuf,
  pub allowed: bool,
  pub root: Option<PathBuf>,
  pub root_priority: i32,
  pub hidden: bool,
  pub ignored: bool,
  pub sensitive: bool,
  pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplainResult {
  pub path: PathBuf,
  pub allowed: bool,
  pub root: Option<PathBuf>,
  pub hidden: bool,
  pub ignored: bool,
  pub sensitive: bool,
  pub reasons: Vec<String>,
  pub score: Option<i32>,
  pub matches: Vec<MatchKind>,
}

pub fn expand_tilde(path: PathBuf) -> PathBuf {
  let Some(text) = path.to_str() else {
    return path;
  };
  if text == "~" {
    if let Some(home) = std::env::var_os("HOME") {
      return PathBuf::from(home);
    }
  }
  if let Some(rest) = text.strip_prefix("~/") {
    if let Some(home) = std::env::var_os("HOME") {
      return PathBuf::from(home).join(rest);
    }
  }
  path
}

pub fn normalize_root_path(path: PathBuf) -> PathBuf {
  let expanded = expand_tilde(path);
  let absolute = if expanded.is_absolute() {
    expanded
  } else if let Ok(current_dir) = std::env::current_dir() {
    current_dir.join(expanded)
  } else {
    expanded
  };
  let canonical = std::fs::canonicalize(&absolute).unwrap_or(absolute);
  strip_windows_verbatim_prefix(canonical)
}

/// Windows `canonicalize` returns extended-length verbatim paths
/// (`\\?\C:\...`); the prefix is rustc-internal noise that leaks into
/// statuses, results and every string comparison against a caller path.
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
  let Some(text) = path.to_str() else {
    return path;
  };
  if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
    PathBuf::from(format!(r"\\{unc}"))
  } else if let Some(drive) = text.strip_prefix(r"\\?\") {
    PathBuf::from(drive)
  } else {
    path
  }
}

pub(crate) fn lowercase_extension(path: &std::path::Path) -> Option<String> {
  path
    .extension()
    .and_then(|value| value.to_str())
    .map(|value| value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn search_root_normalizes_relative_paths_to_absolute_paths() {
    let root = SearchRoot::new("relative-project");

    assert!(root.path.is_absolute());
    assert!(root.path.ends_with("relative-project"));
  }
}

/// Non-empty explicit root identities; hosts resolve paths before constructing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchScope(Vec<i64>);
impl SearchScope {
  pub fn new(mut ids: Vec<i64>) -> Result<Self, crate::BackendError> {
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() || ids.iter().any(|id| *id <= 0) {
      return Err(crate::BackendError::new("Search scope must be non-empty."));
    }
    Ok(Self(ids))
  }
  pub fn ids(&self) -> &[i64] {
    &self.0
  }
}
````

<!-- apply {"task": "M2.3", "file": "crates/kuntu-core/src/lib.rs", "mode": "file"} -->
````rust
//! Provider-independent file search core for Kunkun.
//!
//! This crate owns the domain model, path policy, ranking, and explanation
//! behavior so platform providers and future desktop adapters stay thin.

pub mod backend;
pub mod engine;
pub mod model;
pub mod policy;
pub mod ranking;

pub use backend::{
  BackendError, CandidateProvider, IndexRebuildStats, IndexRefreshStats, IndexRepairStats,
  IndexRootStatus, MetadataIndex,
};
pub use engine::SearchEngineCore;
pub use model::{
  normalize_root_path, EntryKind, ExplainResult, MatchKind, PolicyDecision, SearchCandidate,
  SearchConfig, SearchQuery, SearchResult, SearchRoot,
};
pub use policy::PathPolicy;

pub use model::SearchScope;
````

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/Cargo.toml", "mode": "file"} -->
````toml
# SQLite metadata index crate for the file search prototype.
[package]
name = "kuntu-index"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
kuntu-core = { path = "../kuntu-core" }
kuntu-crawler = { path = "../kuntu-crawler" }
kuntu-legacy-index.workspace = true
rusqlite.workspace = true
kuntu-store.workspace = true
kuntu-vfs.workspace = true
futures.workspace = true
serde_json.workspace = true
zstd.workspace = true
nix.workspace = true

[lib]
path = "src/lib.rs"

[lints]
workspace = true

[dev-dependencies]
turso.workspace = true
````

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/engine.rs", "mode": "file"} -->
````rust
//! Owned-row adapter: synchronous SQLite behind the existing awaitable API.
use super::{IndexError, Result};
use rusqlite::{
  types::{FromSql, Value},
  Params,
};
use std::cell::Cell;
#[derive(Debug)]
pub struct Connection {
  pub raw: rusqlite::Connection,
  pub readonly: Cell<bool>,
  pub minor: i64,
}
pub struct Rows {
  rows: std::vec::IntoIter<Row>,
}
pub struct Row {
  values: Vec<Value>,
}
impl Row {
  pub fn get<T: FromSql>(&self, n: usize) -> rusqlite::Result<T> {
    T::column_result(rusqlite::types::ValueRef::from(&self.values[n])).map_err(|e| {
      rusqlite::Error::FromSqlConversionFailure(n, self.values[n].data_type(), Box::new(e))
    })
  }
}
impl Rows {
  pub async fn next(&mut self) -> rusqlite::Result<Option<Row>> {
    Ok(self.rows.next())
  }
}
impl Connection {
  pub async fn query(&self, sql: &str, p: impl Params) -> rusqlite::Result<Rows> {
    let mut s = self.raw.prepare_cached(sql)?;
    let n = s.column_count();
    let rows = s
      .query_map(p, |r| {
        let mut v = Vec::with_capacity(n);
        for i in 0..n {
          v.push(r.get(i)?);
        }
        Ok(Row { values: v })
      })?
      .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Rows {
      rows: rows.into_iter(),
    })
  }
  pub async fn execute(&self, sql: &str, p: impl Params) -> rusqlite::Result<usize> {
    self.raw.prepare_cached(sql)?.execute(p)
  }
  pub async fn transaction(&self) -> Result<Transaction<'_>> {
    self.begin_writer_txn(false)
  }
  pub fn begin_writer_txn(&self, exclusive: bool) -> Result<Transaction<'_>> {
    if self.readonly.get() {
      return Err(IndexError::ReadOnly);
    }
    self.raw.execute_batch(if exclusive {
      "BEGIN EXCLUSIVE"
    } else {
      "BEGIN IMMEDIATE"
    })?;
    let tx = Transaction {
      conn: self,
      done: false,
    };
    let min = self.raw.query_row(
      "SELECT value FROM meta WHERE key='min_writer_minor'",
      [],
      |r| r.get::<_, i64>(0),
    );
    match min {
      Ok(m) if m > self.minor => {
        self.readonly.set(true);
        Err(IndexError::ReadOnly)
      }
      Ok(_) => Ok(tx),
      Err(e) => {
        let tables: i64 = self.raw.query_row(
          "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
          [],
          |r| r.get(0),
        )?;
        if tables == 0 && exclusive {
          Ok(tx)
        } else {
          Err(e.into())
        }
      }
    }
  }
}
pub struct Transaction<'a> {
  pub conn: &'a Connection,
  done: bool,
}
impl std::ops::Deref for Transaction<'_> {
  type Target = Connection;
  fn deref(&self) -> &Connection {
    self.conn
  }
}
impl Transaction<'_> {
  pub async fn commit(mut self) -> Result<()> {
    self.conn.raw.execute(
      "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='generation'",
      [],
    )?;
    self.conn.raw.execute_batch("COMMIT")?;
    self.done = true;
    Ok(())
  }
}
impl Drop for Transaction<'_> {
  fn drop(&mut self) {
    if !self.done {
      let _ = self.conn.raw.execute_batch("ROLLBACK");
    }
  }
}
````

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/shared.rs", "mode": "file"} -->
````rust
//! Shared access, lease capabilities, scan state, additive migration and hosts.
use super::*;
type EntrySnapshot = HashMap<String, (i64, Option<i64>, Option<i64>)>;
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
    if home.file("quarantine-3.json", false).is_ok() {
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
    let checked = raw.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0));
    if matches!(&checked,Ok(s)if s!="ok") || checked.is_err() {
      drop(raw);
      drop(reader);
      if readonly {
        return Err(IndexError::RecreateRequired);
      }
      let _writer = home
        .lock("indexer.lock", false)?
        .ok_or(IndexError::LeaseRequired)?;
      let _epoch = home
        .lock("epoch-3.lock", false)?
        .ok_or(IndexError::LeaseRequired)?;
      home.resume_quarantine(3, &_writer, &_epoch)?;
      drop(_epoch);
      drop(_writer);
      drop(_boot);
      drop(_local);
      return Self::open_shared_minor(home, readonly, consumer, minor);
    }
    let mut result = Self::from_raw(raw, Some(home), true, readonly, consumer, minor)?;
    result._reader = Some(reader);
    Ok(result)
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
    assert!(
      sqlite_runtime_supported(rusqlite::version_number()),
      "SQLite WAL-reset fix is required"
    );
    let conn = engine::Connection {
      raw,
      readonly: Cell::new(readonly),
      minor,
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
    raw.busy_timeout(Duration::from_millis(5000))?;
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
      lease: None,
      _reader: None,
      shared,
      consumer: consumer.into(),
      data_version: Cell::new(0),
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
    for ext in ["", "-wal", "-shm"] {
      match src.read_file(&format!("{leaf}{ext}")) {
        Ok(mut f) => {
          let mut out = home.file(&format!("import-v2.db{ext}"), true)?;
          out.set_len(0)?;
          std::io::copy(&mut f, &mut out)?;
          out.sync_all()?;
        }
        Err(e) if e.raw_os_error() == Some(2) => {
          let _ = home.remove(&format!("import-v2.db{ext}"));
        }
        Err(e) => return Err(e.into()),
      }
    }
    let copy = rusqlite::Connection::open_with_flags_and_vfs(
      home.sqlite_name("import-v2.db")?,
      rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
      "kuntu-fd",
    )?;
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
      return Err(IndexError::ReadOnly);
    }
    if self.lease.is_some() {
      return Ok(true);
    }
    let Some(home) = &self.home else {
      return Ok(true);
    };
    self.lease = home.lock("indexer.lock", false)?;
    if self.lease.is_some() && self.conn.minor >= 1 {
      let current: i64 =
        self
          .conn
          .raw
          .query_row("SELECT value FROM meta WHERE key='schema_minor'", [], |r| {
            r.get(0)
          })?;
      if current < 1 {
        self.migrate(1)?;
      }
    }
    Ok(self.lease.is_some())
  }
  pub(crate) fn ensure_lease(&mut self) -> Result<()> {
    if self.conn.readonly.get() {
      return Err(IndexError::ReadOnly);
    }
    if !self.shared {
      self.acquire_writer()?;
    }
    if self.home.is_some() && self.lease.is_none() {
      return Err(IndexError::LeaseRequired);
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
    if self.home.is_some() && self.lease.is_none() {
      return Err(IndexError::LeaseRequired);
    }
    let tx = self.conn.begin_writer_txn(false)?;
    tx.conn.raw.execute("UPDATE root_state SET active_scan_gen=CASE WHEN status='building' AND scan_policy_revision=policy_revision THEN active_scan_gen ELSE active_scan_gen+1 END,status='building',dirty=1,scan_policy_revision=policy_revision WHERE root_id=?1",params![root_id])?;
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
    if self.home.is_some() && self.lease.is_none() {
      return Err(IndexError::LeaseRequired);
    }
    while self.cleanup_stale_batch(root_id, gen)? != 0 {}
    fault("before_ready");
    let tx = self.conn.begin_writer_txn(false)?;
    self.check_scan(root_id, gen)?;
    tx.conn.raw.execute("UPDATE root_state SET status='ready',dirty=0,generation=generation+1,last_full_scan_at=?3,entry_count=(SELECT count(*) FROM entries WHERE root_id=?1) WHERE root_id=?1 AND active_scan_gen=?2",params![root_id,gen,unix_now()])?;
    block_on(tx.commit())?;
    fault("ready_committed");
    Ok(())
  }
  pub(crate) fn cleanup_stale_batch(&self, root_id: i64, gen: i64) -> Result<usize> {
    if self.home.is_some() && self.lease.is_none() {
      return Err(IndexError::LeaseRequired);
    }
    let ids: Vec<i64> = {
      let mut query = self
        .conn
        .raw
        .prepare("SELECT id FROM entries WHERE root_id=?1 AND scan_gen<>?2 LIMIT 5000")?;
      let ids = query
        .query_map(params![root_id, gen], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
      ids
    };
    if ids.is_empty() {
      self.check_scan(root_id, gen)?;
      return Ok(0);
    }
    let tx = self.conn.begin_writer_txn(false)?;
    self.check_scan(root_id, gen)?;
    let started = std::time::Instant::now();
    let mut rows = 0;
    for id in ids {
      rows += tx
        .conn
        .raw
        .execute("DELETE FROM entries WHERE id=?1", params![id])?;
      if rows >= 5000 || started.elapsed() >= std::time::Duration::from_millis(100) {
        break;
      }
    }
    block_on(tx.commit())?;
    Ok(rows)
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
    Ok(self.conn.raw.query_row(
      "SELECT count(*) FROM entries WHERE id>=1 AND id<101",
      [],
      |r| r.get(0),
    )?)
  }
  pub fn count_entries(&self) -> Result<i64> {
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
      return Err(IndexError::SchemaTooNew {
        found: min,
        supported: self.conn.minor,
      });
    }
    Ok(())
  }
  pub fn root_state_name(&self, path: &Path) -> Result<String> {
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
    Ok(
      self
        .conn
        .raw
        .query_row("PRAGMA quick_check", [], |r| r.get(0))?,
    )
  }
  pub fn poll_changes(&self) -> Result<bool> {
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
          attempted.store(true, Ordering::Release);
          if index.acquire_writer().unwrap_or(false) {
            acquired.store(true, Ordering::Release);
            while !signal.load(Ordering::Acquire) {
              let _ = index.drain_requests();
              let _ = index.recover_pending_roots();
              for _ in 0..10 {
                if signal.load(Ordering::Acquire) {
                  break;
                }
                thread::sleep(Duration::from_millis(10));
              }
            }
          }
        }
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
````

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/lib.rs", "mode": "file"} -->
````rust
//! M2 SQLite WAL metadata index; query shape retained from M1.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kuntu_core::{
  ranking::{score_candidate, tokenize},
  BackendError, EntryKind, IndexRebuildStats, IndexRefreshStats, IndexRepairStats, IndexRootStatus,
  MatchKind, MetadataIndex, SearchCandidate, SearchConfig, SearchQuery, SearchResult, SearchRoot,
};
use kuntu_crawler::{crawl, CrawlOptions};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter};
mod engine;
mod shared;
use kuntu_store::{Home, Lease, ReaderGuard};
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
  RecreateFailed(Box<IndexError>),
  UnrecognizedDatabase { path: PathBuf },
}

impl std::fmt::Display for IndexError {
  fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      IndexError::ReadOnly => write!(formatter,"Store is read-only."),
      IndexError::LeaseRequired => write!(formatter,"An owned writer lease is required."),
      IndexError::EmptyScope => write!(formatter,"Search scope must be non-empty."),
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
  lease: Option<Lease>,
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
      let (entries, stats) = crawl(&CrawlOptions::new(crawl_config));
      skipped += stats.skipped;
      if !stats.errors.is_empty() {
        errors.extend(stats.errors);
        self.mark_root_dirty(root)?;
        continue;
      }
      self.check_scan(root_id, gen)?;
      let mut offset = 0;
      while offset < entries.len() {
        let started = std::time::Instant::now();
        let tx = self.conn.begin_writer_txn(false)?;
        self.check_scan(root_id, gen)?;
        let mut rows = 0;
        while offset < entries.len()
          && rows < 5_000
          && (rows == 0 || started.elapsed() < std::time::Duration::from_millis(100))
        {
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
mod m2_tests;

fn legacy_error(e: kuntu_legacy_index::IndexError) -> IndexError {
  IndexError::Io(std::io::Error::other(e.to_string()))
}
````

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/fixture.rs", "mode": "file"} -->
````rust
//! Test-only fixtures for the release-mode latency gate and the recall tests.
//!
//! `open_or_build` generates (and caches on disk) a *realistic* store: Zipf
//! distributed vocabulary, a pool of nested directories, one to four name
//! tokens joined by mixed separators, a skewed extension mix, and the root
//! path components that every real row carries. Rows are written with
//! guarded v3 batches through the same SQLite connection and posting writer,
//! with the same term rules as `insert_terms_tx` (name tokens = field 1,
//! extension = field 2, every path component token = field 3).
//!
//! The single-term fixture in `run_bounded_fixture` is pathological (one
//! posting list of N rows); this one is what a real home directory looks like.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use kuntu_core::ranking::tokenize;
use kuntu_core::{SearchConfig, SearchRoot};
use rusqlite::params;

use super::{block_on, remove_database_files, KuntuIndex};

/// xorshift64: deterministic across platforms and runs.
pub struct Rng(u64);

impl Rng {
  pub fn new(seed: u64) -> Self {
    Self(seed)
  }

  pub fn next_u64(&mut self) -> u64 {
    self.0 ^= self.0 << 13;
    self.0 ^= self.0 >> 7;
    self.0 ^= self.0 << 17;
    self.0
  }

  fn unit(&mut self) -> f64 {
    (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
  }

  /// Log-uniform rank in `0..n`, i.e. a Zipf(1) distribution.
  pub fn zipf(&mut self, n: usize) -> usize {
    (((n as f64).ln() * self.unit()).exp() as usize)
      .saturating_sub(1)
      .min(n - 1)
  }
}

const SYLLABLES: [&str; 40] = [
  "ba", "ko", "ri", "tu", "men", "sa", "lo", "vi", "dar", "pe", "qua", "non", "fi", "gra", "hu",
  "jo", "ka", "lem", "mo", "ne", "or", "pla", "ro", "sti", "tra", "un", "ve", "wi", "xa", "yo",
  "zen", "con", "dis", "ex", "inf", "pro", "re", "sub", "tel", "ult",
];

const EXTENSIONS: [&str; 30] = [
  "ts", "js", "json", "md", "rs", "png", "jpg", "pdf", "txt", "log", "toml", "yaml", "html", "css",
  "svelte", "py", "go", "swift", "mp4", "zip", "csv", "docx", "xlsx", "heic", "dat", "lock", "map",
  "sh", "c", "h",
];

fn word(index: usize) -> String {
  let mut rest = index + 1;
  let mut word = String::new();
  let mut syllables = 0;
  while rest > 0 || syllables < 2 {
    word.push_str(SYLLABLES[rest % SYLLABLES.len()]);
    rest /= SYLLABLES.len();
    syllables += 1;
  }
  word
}

/// One row to plant: an absolute path and whether it is a directory.
#[derive(Debug, Clone)]
pub struct RowSpec {
  pub path: String,
  pub is_dir: bool,
}

/// Query texts chosen from the generated data, with ground truth.
#[derive(Debug, Clone)]
pub struct FixtureQueries {
  /// A token on 3 to 10 rows.
  pub rare: String,
  /// A token on about rows/3300 .. rows/1000 rows.
  pub mid: String,
  /// A token on about rows/50 .. rows/12 rows.
  pub broad: String,
  pub prefix3: String,
  pub prefix1: String,
  /// A token present on every row (a component of the fixture root).
  pub all_rows: String,
  /// `"<mid> ts"`: a selective token plus the most common extension.
  pub mixed: String,
  /// Rows whose path has a token starting with `mid` and one starting with `ts`.
  pub mixed_truth: usize,
}

pub struct Fixture {
  pub index: KuntuIndex,
  pub config: SearchConfig,
  pub rows: usize,
  pub queries: FixtureQueries,
  /// `Some` when this call built the fixture, `None` when it was reused.
  pub build_secs: Option<f64>,
  pub db_bytes: u64,
}

pub fn generate_rows(root: &str, rows: usize) -> Vec<RowSpec> {
  let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
  let vocabulary = 40_000;
  let directory_target = (rows / 12).max(1);
  let mut directories: Vec<(String, u32)> = vec![(root.to_string(), 0)];
  let mut seen: HashSet<String> = HashSet::new();
  let mut specs: Vec<RowSpec> = Vec::with_capacity(rows);
  while directories.len() < directory_target {
    let parent = (rng.next_u64() as usize) % directories.len();
    if directories[parent].1 >= 8 {
      continue;
    }
    let depth = directories[parent].1 + 1;
    let path = format!("{}/{}", directories[parent].0, word(rng.zipf(vocabulary)));
    if seen.insert(path.clone()) {
      specs.push(RowSpec {
        path: path.clone(),
        is_dir: true,
      });
      directories.push((path, depth));
    }
  }
  while specs.len() < rows {
    let directory = (rng.next_u64() as usize) % directories.len();
    let tokens = 1 + (rng.next_u64() % 4) as usize;
    let separator = ["-", "_", ".", " "][(rng.next_u64() % 4) as usize];
    let mut name = (0..tokens)
      .map(|_| word(rng.zipf(vocabulary)))
      .collect::<Vec<_>>()
      .join(separator);
    if rng.next_u64().is_multiple_of(5) {
      name.push_str(&(rng.next_u64() % 1000).to_string());
    }
    let extension = EXTENSIONS[rng.zipf(EXTENSIONS.len())];
    let path = format!("{}/{name}.{extension}", directories[directory].0);
    if seen.insert(path.clone()) {
      specs.push(RowSpec {
        path,
        is_dir: false,
      });
    }
  }
  specs
}

/// Plant the realistic fixture through v3's actual entry/posting writer.
pub fn insert_rows(index: &mut KuntuIndex, root_id: i64, rows: &[RowSpec], first_id: i64) {
  use super::{block_on, insert_terms_tx};
  let mut next = first_id;
  for batch in rows.chunks(5000) {
    let tx = index.conn.begin_writer_txn(false).unwrap();
    for row in batch {
      let path = Path::new(&row.path);
      tx.conn.raw.prepare_cached("INSERT INTO entries(id,root_id,path,name,extension,kind,size) VALUES(?1,?2,?3,?4,?5,?6,8)").unwrap().execute(params![next,root_id,row.path,path.file_name().and_then(|v|v.to_str()).unwrap_or_default(),path.extension().and_then(|v|v.to_str()),if row.is_dir{2}else{1}]).unwrap();
      block_on(insert_terms_tx(&tx, next, path)).unwrap();
      next += 1;
    }
    block_on(tx.commit()).unwrap();
  }
}

fn pick(counts: &HashMap<String, u32>, low: u32, high: u32) -> String {
  let mut candidates: Vec<(&String, &u32)> = counts
    .iter()
    .filter(|(token, count)| {
      **count >= low
        && **count <= high
        && token.len() >= 4
        && token.bytes().all(|byte| byte.is_ascii_lowercase())
    })
    .collect();
  candidates.sort_by(|left, right| left.1.cmp(right.1).then_with(|| left.0.cmp(right.0)));
  candidates
    .get(candidates.len() / 2)
    .map(|(token, _)| (*token).clone())
    .expect("the fixture has a token in the requested frequency band")
}

fn choose_queries(specs: &[RowSpec], rows: usize) -> FixtureQueries {
  let mut counts: HashMap<String, u32> = HashMap::new();
  let mut row_tokens: Vec<Vec<String>> = Vec::with_capacity(specs.len());
  for spec in specs {
    let mut tokens = tokenize(&spec.path);
    tokens.sort();
    tokens.dedup();
    for token in &tokens {
      *counts.entry(token.clone()).or_insert(0) += 1;
    }
    row_tokens.push(tokens);
  }
  let rows_u32 = rows as u32;
  let rare = pick(&counts, 3, 10);
  let mid = pick(
    &counts,
    (rows_u32 / 3300).max(20),
    (rows_u32 / 1000).max(60),
  );
  let broad = pick(&counts, rows_u32 / 50, rows_u32 / 12);
  let has_prefix =
    |tokens: &[String], prefix: &str| tokens.iter().any(|token| token.starts_with(prefix));
  let mixed_truth = row_tokens
    .iter()
    .filter(|tokens| has_prefix(tokens, &mid) && has_prefix(tokens, "ts"))
    .count();
  FixtureQueries {
    prefix3: broad[..3].to_string(),
    prefix1: broad[..1].to_string(),
    mixed: format!("{mid} ts"),
    all_rows: "kuntu".to_string(),
    mixed_truth,
    rare,
    mid,
    broad,
  }
}

fn write_queries(path: &Path, rows: usize, queries: &FixtureQueries) {
  let text = format!(
    "rows={rows}\nrare={}\nmid={}\nbroad={}\nprefix3={}\nprefix1={}\nall_rows={}\nmixed={}\nmixed_truth={}\n",
    queries.rare,
    queries.mid,
    queries.broad,
    queries.prefix3,
    queries.prefix1,
    queries.all_rows,
    queries.mixed,
    queries.mixed_truth
  );
  fs::write(path, text).unwrap();
}

fn read_queries(path: &Path, rows: usize) -> Option<FixtureQueries> {
  let text = fs::read_to_string(path).ok()?;
  let values: HashMap<&str, &str> = text
    .lines()
    .filter_map(|line| line.split_once('='))
    .collect();
  if values.get("rows")?.parse::<usize>().ok()? != rows {
    return None;
  }
  Some(FixtureQueries {
    rare: values.get("rare")?.to_string(),
    mid: values.get("mid")?.to_string(),
    broad: values.get("broad")?.to_string(),
    prefix3: values.get("prefix3")?.to_string(),
    prefix1: values.get("prefix1")?.to_string(),
    all_rows: values.get("all_rows")?.to_string(),
    mixed: values.get("mixed")?.to_string(),
    mixed_truth: values.get("mixed_truth")?.parse().ok()?,
  })
}

/// Open the cached fixture for `rows`, building it first when it is missing.
/// The cache lives in `$KUNTU_BENCH_DIR` (default: a `kuntu-bench` directory
/// under the system temp dir) and is reused across runs; delete the directory
/// to force a rebuild. The queries file is written last, so an interrupted
/// build is rebuilt.
pub fn open_or_build(rows: usize) -> Fixture {
  assert!(
    rows >= 20_000,
    "the frequency bands need at least 20,000 rows"
  );
  let base = std::env::var_os("KUNTU_BENCH_DIR")
    .map(PathBuf::from)
    .unwrap_or_else(|| std::env::temp_dir().join("kuntu-bench"));
  fs::create_dir_all(&base).unwrap();
  let root = base.join(format!("kuntu-root-{rows}"));
  fs::create_dir_all(&root).unwrap();
  let root = PathBuf::from("/Users/kuntu/fixture");
  let db_path = base.join(format!("realistic-v3-rev2-compact-{rows}.sqlite"));
  let queries_path = base.join(format!("realistic-v3-rev2-compact-{rows}.queries"));
  let config = SearchConfig {
    roots: vec![SearchRoot::new(&root)],
  };

  if db_path.exists() {
    if let Some(queries) = read_queries(&queries_path, rows) {
      let index = KuntuIndex::open(&db_path).unwrap();
      let db_bytes = fs::metadata(&db_path).map(|meta| meta.len()).unwrap_or(0);
      return Fixture {
        index,
        config,
        rows,
        queries,
        build_secs: None,
        db_bytes,
      };
    }
  }

  remove_database_files(&db_path);
  let _ = fs::remove_file(&queries_path);
  let started = Instant::now();
  let specs = generate_rows(&root.to_string_lossy(), rows);
  let queries = choose_queries(&specs, rows);
  {
    let mut index = KuntuIndex::open(&db_path).unwrap();
    let root_id = index.register_root(&config.roots[0]).unwrap();
    let tx = index.conn.begin_writer_txn(false).unwrap();
    tx.conn
      .raw
      .execute(
        "UPDATE root_state SET status='ready',dirty=0 WHERE root_id=?1",
        params![root_id],
      )
      .unwrap();
    tx.conn.raw.execute("DELETE FROM entries", []).unwrap();
    block_on(tx.commit()).unwrap();
    let root_id: i64 = block_on(async {
      let mut found = index
        .conn
        .query(
          "SELECT id FROM roots WHERE path = ?1",
          params![root.to_string_lossy()],
        )
        .await
        .unwrap();
      found.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    });
    insert_rows(&mut index, root_id, &specs, 1);
  }
  write_queries(&queries_path, rows, &queries);
  let build_secs = started.elapsed().as_secs_f64();
  let index = KuntuIndex::open(&db_path).unwrap();
  let db_bytes = fs::metadata(&db_path).map(|meta| meta.len()).unwrap_or(0);
  eprintln!("FIXTURE built rows={rows} in {build_secs:.1} s, {db_bytes} bytes at {db_path:?}");
  Fixture {
    index,
    config,
    rows,
    queries,
    build_secs: Some(build_secs),
    db_bytes,
  }
}

/// Measured runs of one gate class, after the untimed warm-up.
pub const TIMED_RUNS: usize = 3;

/// Timing policy of the latency gate: one untimed warm-up run, then the best
/// of [`TIMED_RUNS`] measured runs. The warm-up absorbs the cold page cache
/// and the freshly opened database; no measured run is abandoned early,
/// because an early break would let the first cold run decide the gate
/// instead of the warm runs the policy is about (review P2).
pub fn measure_warm_best(mut run: impl FnMut() -> std::time::Duration) -> std::time::Duration {
  run();
  let mut best = std::time::Duration::MAX;
  for _ in 0..TIMED_RUNS {
    best = best.min(run());
  }
  best
}
````

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/m2_tests.rs", "mode": "file"} -->
````rust
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
fn resume_keeps_generation_stale_completion_is_rejected_and_building_is_reported() {
  let (_h, c, i) = fixture();
  let id = i
    .conn
    .raw
    .query_row("SELECT id FROM roots", [], |r| r.get(0))
    .unwrap();
  let gen = i.begin_scan(id).unwrap();
  assert_eq!(gen, i.begin_scan(id).unwrap());
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
    gen
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
  loop {
    let t = std::time::Instant::now();
    let n = i.cleanup_stale_batch(root, 0).unwrap();
    if n == 0 {
      break;
    }
    deleted += n;
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
    "REVERSE_DELETE rows={rows} database_bytes={bytes} deleted={deleted} elapsed_ms={ms:.3} batches={batches} max_call_ms={max_ms:.3}"
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
  let n = i.cleanup_stale_batch(id, gen).unwrap();
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
````

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/examples/m2_probe.rs", "mode": "file"} -->
````rust
//! Foreground process probe for real crash, skew, lease and stress evidence.
use kuntu_core::{SearchConfig, SearchQuery, SearchRoot};
use kuntu_index::{KuntuIndex, WriterService};
use kuntu_store::Home;
use std::path::Path;
use std::time::{Duration, Instant};
fn mark(text: &str) -> std::io::Result<()> {
  if let Ok(p) = std::env::var("KUNTU_FAULT_MARKER") {
    std::fs::write(p, text)?;
  }
  Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
  let a: Vec<_> = std::env::args().collect();
  let home = Home::open(Path::new(&a[2]))?;
  if a[1] == "create-v2" {
    let path = home.path.join("source.v2.db");
    let db =
      futures::executor::block_on(turso::Builder::new_local(path.to_str().unwrap()).build())?;
    let conn = db.connect()?;
    futures::executor::block_on(conn.execute_batch(kuntu_legacy_index::SCHEMA_DDL))?;
    futures::executor::block_on(conn.execute_batch("PRAGMA user_version=2;"))?;
    futures::executor::block_on(conn.execute(
      "INSERT INTO roots(path,enabled,updated_at) VALUES(?1,1,0)",
      turso::params![a[3].clone()],
    ))?;
    println!("{}", path.display());
    return Ok(());
  }
  if a[1] == "identity" {
    let index = KuntuIndex::open_shared(home.clone(), false, "probe")?;
    drop(index);
    println!("{}", home.id()?);
    return Ok(());
  }
  if a[1] == "quarantine" {
    home.quarantine(3)?;
    return Ok(());
  }
  if a[1] == "poll" {
    let service = WriterService::start(home);
    let start = Instant::now();
    while !service.is_holder() {
      if start.elapsed() > Duration::from_secs(20) {
        return Err("Takeover timeout.".into());
      }
      std::thread::sleep(Duration::from_millis(10));
    }
    println!("takeover_ms={}", start.elapsed().as_millis());
    mark("holder")?;
    return Ok(());
  }
  let minor = a.get(4).and_then(|v| v.parse::<i64>().ok()).unwrap_or(1);
  let readonly = matches!(a[1].as_str(), "reader" | "hold-reader" | "skew-reader");
  let mut index = KuntuIndex::open_shared_minor(home.clone(), readonly, "probe", minor)?;
  let config = SearchConfig {
    roots: vec![SearchRoot::new(a.get(3).ok_or("Missing root.")?)],
  };
  match a[1].as_str() {
    "init-permissive" => {
      let mut root = config.roots[0].clone();
      root.include_hidden = true;
      root.include_ignored = true;
      index.register_root(&root)?;
    }
    "init" => {
      index.register_root(&config.roots[0])?;
    }
    "scan" => {
      if !index.acquire_writer()? {
        return Err("Lease busy.".into());
      }
      let s = index.rebuild(&config)?;
      println!(
        "entries={} generation={} check={}",
        s.entries,
        index.generation_probe()?,
        index.doctor()?
      );
    }
    "inspect" => {
      println!(
        "entries={} generation={} check={} access={:?}",
        index.count_entries()?,
        index.generation_probe()?,
        index.doctor()?,
        index.access()
      );
    }
    "hold" | "hold-reader" => {
      if a[1] == "hold" && !index.acquire_writer()? {
        return Err("Lease busy.".into());
      }
      mark("held")?;
      std::thread::sleep(Duration::from_secs(120));
    }
    "exec-holder" => {
      if !index.acquire_writer()? {
        return Err("Lease busy.".into());
      }
      mark("held")?;
      let child = std::process::Command::new("/bin/sleep")
        .arg("20")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
      println!("child_pid={}", child.id());
      // Returning drops the lease while the exec child is still alive.
    }
    "skew-reader" => {
      println!("access={:?} check={}", index.access(), index.doctor()?);
    }
    "skew-writer" => {
      let acquired = index.acquire_writer()?;
      println!("acquired={acquired} access={:?}", index.access());
    }
    "read" => {
      println!(
        "{:?}",
        index
          .search_with_metrics(&config, &SearchQuery::new("file"))?
          .coverage
      );
    }
    "reader" => {
      let start = Instant::now();
      let mut samples = Vec::new();
      let mut errors = 0;
      while start.elapsed() < Duration::from_secs(60) {
        let t = Instant::now();
        if (if samples.len().is_multiple_of(2) {
          index.point_probe()
        } else {
          index.range_probe()
        })
        .is_err()
        {
          errors += 1;
        }
        samples.push(t.elapsed().as_micros());
        std::thread::sleep(Duration::from_micros(500));
      }
      samples.sort_unstable();
      let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
      let max = *samples.last().unwrap();
      println!(
        "{{\"queries\":{},\"errors\":{errors},\"p99_us\":{p99},\"max_us\":{max}}}",
        samples.len()
      );
    }
    "writer" => {
      let start = Instant::now();
      let mut commits = 0;
      let mut blocked = 0;
      while start.elapsed() < Duration::from_secs(60) {
        if index.acquire_writer()? {
          index.rebuild(&config)?;
          commits += 1;
        } else {
          index.enqueue("refresh", &config)?;
          blocked += 1;
          std::thread::sleep(Duration::from_millis(10));
        }
      }
      println!("{{\"scans\":{commits},\"inbox_writes\":{blocked}}}");
    }
    _ => return Err("Unknown probe operation.".into()),
  }
  Ok(())
}
````

## Task M2.4: access, atomic migrations and quarantine

RED/GREEN: `actual_opens_enforce_skew_before_wal_and_old_writer_steps_down`. Also run `caller_cannot_claim_a_future_build_minor_to_bypass_writer_floor`, `every_write_transaction_rechecks_minor_and_rolls_back`, `private_open_cannot_bypass_a_shared_store_lease`, and the process `migration_ddl` / `quarantine_renamed` fault cases. The engine file above contains the complete runner, compatibility path and marker recovery, not unused helpers.

## Task M2.5: guarded batches and reader notification

RED/GREEN: `batch_generation_is_atomic_and_readers_observe_data_version`. `slow_delete_triggers_force_the_100_ms_cleanup_boundary` makes a SQLite trigger spend 20 ms per deleted row and requires one production cleanup call to return after at most six rows. `slow_scan_rows_force_the_100_ms_batch_boundary` injects slow row work and asserts that the production scanner commits before consuming all 100 rows. The 30%/70% process tests assert that overshoot is less than 5,000 rows. `meta.generation` is part of the transaction commit; `data_version` is checked on production reads. The 100 ms deadline is checked between rows; a single row, commit, busy wait or scheduler pause can exceed it, so the stress measurements—not an absolute real-time guarantee—establish the latency evidence.

## Task M2.6: restart/resume state machine

RED/GREEN: `resume_keeps_generation_stale_completion_is_rejected_and_building_is_reported`. Also run `consumer_removal_and_disable_invalidate_inflight_generation` and `restart_recovery_waits_for_live_holder_then_resumes_persisted_generation`. Resume re-walks with the same generation and retains unchanged postings; changed policy uses a new generation. The service reconstructs recovery work from persisted memberships, never client state, and only after obtaining its capability.

## Task M2.7: persistent takeover and joined shutdown

RED/GREEN: `joined_takeover_service_releases_lease_only_after_worker_stops`. `WriterService::start` is public and wired into the hosts below. The process probe covers a real holder death, five-second polling, child exec and capability transfer. The worker joins before its service can release the lease. Read-only hosts are configured to use a separate signed helper; `openReadOnly` never starts an indexer in the read-only application.

## Task M2.8: NAPI access propagation and exact old constructor

Install complete adapter, generated JS/declarations and Node tests. The string constructor remains private and opens exactly its path; a real v2 fixture exercises that backend. The object constructor, `openShared`, `withHome`, `openReadOnly`, `openClient`, `requestRebuild`, `mode`, `registerRoots` and `close` are additive. Every task carries access, kind and consumer. Shared instances pool a read connection and re-check its floor after the mutex wait. `openClient` registers and submits requests without an indexer; `openShared` reports effective compatibility access and starts a service only when writable. Read-only rebuild/refresh/repair/registration fail; shared bulk requests enter the inbox. Scope and phase are exposed in the DTOs. RED/GREEN is the Node test `openReadOnly enforces access on rebuild refresh repair and registration`; also run `old_constructor_opens_exact_v2_path_and_refuses_unrelated_database` and `strict consumer ignores permissive indexing union and supplied query flags`.

<!-- apply {"task": "M2.8", "file": "crates/kuntu-napi/Cargo.toml", "mode": "file"} -->
````toml
# Local NAPI binding crate for consuming Kunkun file search from Node/Electron.
[package]
name = "kuntu-napi"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[lib]
crate-type = ["cdylib"]
path = "src/lib.rs"

[dependencies]
kuntu-core = { path = "../kuntu-core" }
kuntu-index = { path = "../kuntu-index" }
kuntu-store.workspace = true
napi = "3"
napi-derive = "3"

[build-dependencies]
napi-build = "2"

[dev-dependencies]
kuntu-legacy-index.workspace = true
rusqlite.workspace = true
````

<!-- apply {"task": "M2.8", "file": "crates/kuntu-napi/src/lib.rs", "mode": "file"} -->
````rust
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
type ReadPool = Arc<Mutex<KuntuIndex>>;

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
      _reader: Some(Arc::new(Mutex::new(read))),
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
      _reader: Some(Arc::new(Mutex::new(reader))),
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
      _reader: Some(Arc::new(Mutex::new(read))),
    })
  }
  #[napi]
  pub fn close(&mut self) {
    self._reader.take();
    self._service.take();
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
    let index = pool.lock().map_err(napi_error)?;
    index.recheck_read_access().map_err(napi_error)?;
    read(&index)
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
````

<!-- apply {"task": "M2.8", "file": "crates/kuntu-napi/index.js", "mode": "file"} -->
````javascript
// prettier-ignore
/* eslint-disable */
// @ts-nocheck
/* auto-generated by NAPI-RS */

const { readFileSync } = require('node:fs')
let nativeBinding = null
const loadErrors = []

const isMusl = () => {
  let musl = false
  if (process.platform === 'linux') {
    musl = isMuslFromFilesystem()
    if (musl === null) {
      musl = isMuslFromReport()
    }
    if (musl === null) {
      musl = isMuslFromChildProcess()
    }
  }
  return musl
}

const isFileMusl = (f) => f.includes('libc.musl-') || f.includes('ld-musl-')

const isMuslFromFilesystem = () => {
  try {
    return readFileSync('/usr/bin/ldd', 'utf-8').includes('musl')
  } catch {
    return null
  }
}

const isMuslFromReport = () => {
  let report = null
  if (typeof process.report?.getReport === 'function') {
    process.report.excludeNetwork = true
    report = process.report.getReport()
  }
  if (!report) {
    return null
  }
  if (report.header && report.header.glibcVersionRuntime) {
    return false
  }
  if (Array.isArray(report.sharedObjects)) {
    if (report.sharedObjects.some(isFileMusl)) {
      return true
    }
  }
  return false
}

const isMuslFromChildProcess = () => {
  try {
    return require('child_process').execSync('ldd --version', { encoding: 'utf8' }).includes('musl')
  } catch (e) {
    // If we reach this case, we don't know if the system is musl or not, so is better to just fallback to false
    return false
  }
}

function requireNative() {
  if (process.env.NAPI_RS_NATIVE_LIBRARY_PATH) {
    try {
      return require(process.env.NAPI_RS_NATIVE_LIBRARY_PATH);
    } catch (err) {
      loadErrors.push(err)
    }
  } else if (process.platform === 'android') {
    if (process.arch === 'arm64') {
      try {
        return require('./kfs-native.android-arm64.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-android-arm64')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-android-arm64/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else if (process.arch === 'arm') {
      try {
        return require('./kfs-native.android-arm-eabi.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-android-arm-eabi')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-android-arm-eabi/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else {
      loadErrors.push(new Error(`Unsupported architecture on Android ${process.arch}`))
    }
  } else if (process.platform === 'win32') {
    if (process.arch === 'x64') {
      if (process.config?.variables?.shlib_suffix === 'dll.a' || process.config?.variables?.node_target_type === 'shared_library') {
        try {
        return require('./kfs-native.win32-x64-gnu.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-win32-x64-gnu')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-win32-x64-gnu/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
      } else {
        try {
        return require('./kfs-native.win32-x64-msvc.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-win32-x64-msvc')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-win32-x64-msvc/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
      }
    } else if (process.arch === 'ia32') {
      try {
        return require('./kfs-native.win32-ia32-msvc.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-win32-ia32-msvc')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-win32-ia32-msvc/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else if (process.arch === 'arm64') {
      try {
        return require('./kfs-native.win32-arm64-msvc.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-win32-arm64-msvc')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-win32-arm64-msvc/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else {
      loadErrors.push(new Error(`Unsupported architecture on Windows: ${process.arch}`))
    }
  } else if (process.platform === 'darwin') {
    try {
      return require('./kfs-native.darwin-universal.node')
    } catch (e) {
      loadErrors.push(e)
    }
    try {
      const binding = require('@kunkunsh/file-search-native-darwin-universal')
      const bindingPackageVersion = require('@kunkunsh/file-search-native-darwin-universal/package.json').version
      if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
        throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
      }
      return binding
    } catch (e) {
      loadErrors.push(e)
    }
    if (process.arch === 'x64') {
      try {
        return require('./kfs-native.darwin-x64.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-darwin-x64')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-darwin-x64/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else if (process.arch === 'arm64') {
      try {
        return require('./kfs-native.darwin-arm64.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-darwin-arm64')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-darwin-arm64/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else {
      loadErrors.push(new Error(`Unsupported architecture on macOS: ${process.arch}`))
    }
  } else if (process.platform === 'freebsd') {
    if (process.arch === 'x64') {
      try {
        return require('./kfs-native.freebsd-x64.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-freebsd-x64')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-freebsd-x64/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else if (process.arch === 'arm64') {
      try {
        return require('./kfs-native.freebsd-arm64.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-freebsd-arm64')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-freebsd-arm64/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else {
      loadErrors.push(new Error(`Unsupported architecture on FreeBSD: ${process.arch}`))
    }
  } else if (process.platform === 'linux') {
    if (process.arch === 'x64') {
      if (isMusl()) {
        try {
          return require('./kfs-native.linux-x64-musl.node')
        } catch (e) {
          loadErrors.push(e)
        }
        try {
          const binding = require('@kunkunsh/file-search-native-linux-x64-musl')
          const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-x64-musl/package.json').version
          if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
            throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
          }
          return binding
        } catch (e) {
          loadErrors.push(e)
        }
      } else {
        try {
          return require('./kfs-native.linux-x64-gnu.node')
        } catch (e) {
          loadErrors.push(e)
        }
        try {
          const binding = require('@kunkunsh/file-search-native-linux-x64-gnu')
          const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-x64-gnu/package.json').version
          if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
            throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
          }
          return binding
        } catch (e) {
          loadErrors.push(e)
        }
      }
    } else if (process.arch === 'arm64') {
      if (isMusl()) {
        try {
          return require('./kfs-native.linux-arm64-musl.node')
        } catch (e) {
          loadErrors.push(e)
        }
        try {
          const binding = require('@kunkunsh/file-search-native-linux-arm64-musl')
          const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-arm64-musl/package.json').version
          if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
            throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
          }
          return binding
        } catch (e) {
          loadErrors.push(e)
        }
      } else {
        try {
          return require('./kfs-native.linux-arm64-gnu.node')
        } catch (e) {
          loadErrors.push(e)
        }
        try {
          const binding = require('@kunkunsh/file-search-native-linux-arm64-gnu')
          const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-arm64-gnu/package.json').version
          if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
            throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
          }
          return binding
        } catch (e) {
          loadErrors.push(e)
        }
      }
    } else if (process.arch === 'arm') {
      if (isMusl()) {
        try {
          return require('./kfs-native.linux-arm-musleabihf.node')
        } catch (e) {
          loadErrors.push(e)
        }
        try {
          const binding = require('@kunkunsh/file-search-native-linux-arm-musleabihf')
          const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-arm-musleabihf/package.json').version
          if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
            throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
          }
          return binding
        } catch (e) {
          loadErrors.push(e)
        }
      } else {
        try {
          return require('./kfs-native.linux-arm-gnueabihf.node')
        } catch (e) {
          loadErrors.push(e)
        }
        try {
          const binding = require('@kunkunsh/file-search-native-linux-arm-gnueabihf')
          const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-arm-gnueabihf/package.json').version
          if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
            throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
          }
          return binding
        } catch (e) {
          loadErrors.push(e)
        }
      }
    } else if (process.arch === 'loong64') {
      if (isMusl()) {
        try {
          return require('./kfs-native.linux-loong64-musl.node')
        } catch (e) {
          loadErrors.push(e)
        }
        try {
          const binding = require('@kunkunsh/file-search-native-linux-loong64-musl')
          const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-loong64-musl/package.json').version
          if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
            throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
          }
          return binding
        } catch (e) {
          loadErrors.push(e)
        }
      } else {
        try {
          return require('./kfs-native.linux-loong64-gnu.node')
        } catch (e) {
          loadErrors.push(e)
        }
        try {
          const binding = require('@kunkunsh/file-search-native-linux-loong64-gnu')
          const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-loong64-gnu/package.json').version
          if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
            throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
          }
          return binding
        } catch (e) {
          loadErrors.push(e)
        }
      }
    } else if (process.arch === 'riscv64') {
      if (isMusl()) {
        try {
          return require('./kfs-native.linux-riscv64-musl.node')
        } catch (e) {
          loadErrors.push(e)
        }
        try {
          const binding = require('@kunkunsh/file-search-native-linux-riscv64-musl')
          const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-riscv64-musl/package.json').version
          if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
            throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
          }
          return binding
        } catch (e) {
          loadErrors.push(e)
        }
      } else {
        try {
          return require('./kfs-native.linux-riscv64-gnu.node')
        } catch (e) {
          loadErrors.push(e)
        }
        try {
          const binding = require('@kunkunsh/file-search-native-linux-riscv64-gnu')
          const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-riscv64-gnu/package.json').version
          if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
            throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
          }
          return binding
        } catch (e) {
          loadErrors.push(e)
        }
      }
    } else if (process.arch === 'ppc64') {
      try {
        return require('./kfs-native.linux-ppc64-gnu.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-linux-ppc64-gnu')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-ppc64-gnu/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else if (process.arch === 's390x') {
      try {
        return require('./kfs-native.linux-s390x-gnu.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-linux-s390x-gnu')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-linux-s390x-gnu/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else {
      loadErrors.push(new Error(`Unsupported architecture on Linux: ${process.arch}`))
    }
  } else if (process.platform === 'openharmony') {
    if (process.arch === 'arm64') {
      try {
        return require('./kfs-native.openharmony-arm64.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-openharmony-arm64')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-openharmony-arm64/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else if (process.arch === 'x64') {
      try {
        return require('./kfs-native.openharmony-x64.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-openharmony-x64')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-openharmony-x64/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else if (process.arch === 'arm') {
      try {
        return require('./kfs-native.openharmony-arm.node')
      } catch (e) {
        loadErrors.push(e)
      }
      try {
        const binding = require('@kunkunsh/file-search-native-openharmony-arm')
        const bindingPackageVersion = require('@kunkunsh/file-search-native-openharmony-arm/package.json').version
        if (bindingPackageVersion !== '0.1.0' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {
          throw new Error(`Native binding package version mismatch, expected 0.1.0 but got ${bindingPackageVersion}. You can reinstall dependencies to fix this issue.`)
        }
        return binding
      } catch (e) {
        loadErrors.push(e)
      }
    } else {
      loadErrors.push(new Error(`Unsupported architecture on OpenHarmony: ${process.arch}`))
    }
  } else {
    loadErrors.push(new Error(`Unsupported OS: ${process.platform}, architecture: ${process.arch}`))
  }
}

nativeBinding = requireNative()

if (!nativeBinding || process.env.NAPI_RS_FORCE_WASI) {
  let wasiBinding = null
  let wasiBindingError = null
  try {
    wasiBinding = require('./kfs-native.wasi.cjs')
    nativeBinding = wasiBinding
  } catch (err) {
    if (process.env.NAPI_RS_FORCE_WASI) {
      wasiBindingError = err
    }
  }
  if (!nativeBinding || process.env.NAPI_RS_FORCE_WASI) {
    try {
      wasiBinding = require('@kunkunsh/file-search-native-wasm32-wasi')
      nativeBinding = wasiBinding
    } catch (err) {
      if (process.env.NAPI_RS_FORCE_WASI) {
        if (!wasiBindingError) {
          wasiBindingError = err
        } else {
          wasiBindingError.cause = err
        }
        loadErrors.push(err)
      }
    }
  }
  if (process.env.NAPI_RS_FORCE_WASI === 'error' && !wasiBinding) {
    const error = new Error('WASI binding not found and NAPI_RS_FORCE_WASI is set to error')
    error.cause = wasiBindingError
    throw error
  }
}

if (!nativeBinding) {
  if (loadErrors.length > 0) {
    throw new Error(
      `Cannot find native binding. ` +
        `npm has a bug related to optional dependencies (https://github.com/npm/cli/issues/4828). ` +
        'Please try `npm i` again after removing both package-lock.json and node_modules directory.',
      {
        cause: loadErrors.reduce((err, cur) => {
          cur.cause = err
          return cur
        }),
      },
    )
  }
  throw new Error(`Failed to load native binding`)
}

module.exports = nativeBinding
module.exports.FileSearchIndex = nativeBinding.FileSearchIndex
````

<!-- apply {"task": "M2.8", "file": "crates/kuntu-napi/index.d.ts", "mode": "file"} -->
````typescript
/* auto-generated by NAPI-RS */
/* eslint-disable */
export declare class FileSearchIndex {
  static openShared(home?: string | undefined | null, consumer?: string | undefined | null): FileSearchIndex
  static openClient(home?: string | undefined | null, consumer?: string | undefined | null): FileSearchIndex
  requestRebuild(roots: Array<SearchRootInput>): string
  static openReadOnly(home?: string | undefined | null, consumer?: string | undefined | null): FileSearchIndex
  close(): void
  mode(): string
  registerRoots(roots: Array<SearchRootInput>): void
  constructor(input: string | SharedStoreOptions)
  static withHome(home?: string | undefined | null): FileSearchIndex
  rebuild(roots: Array<SearchRootInput>): Promise<IndexRebuildStatsDto>
  refresh(roots: Array<SearchRootInput>): Promise<IndexRefreshStatsDto>
  repair(roots: Array<SearchRootInput>): Promise<IndexRepairStatsDto>
  status(): Promise<Array<IndexRootStatusDto>>
  search(request: SearchRequest): Promise<IndexedSearchOutcomeDto>
}

export interface IndexedSearchOutcomeDto {
  coverage: string
  candidateCount: number
  results: Array<SearchResultDto>
}

export interface IndexRebuildStatsDto {
  roots: number
  entries: number
  skipped: number
  errors: Array<string>
}

export interface IndexRefreshStatsDto {
  roots: number
  inserted: number
  updated: number
  deleted: number
  unchanged: number
  skipped: number
  errors: Array<string>
}

export interface IndexRepairStatsDto {
  roots: number
  dirtyRoots: number
  repairedRoots: number
  errors: Array<string>
}

export interface IndexRootStatusDto {
  status: string
  path: string
  entryCount: number
  generation: number
  dirty: boolean
  lastFullScanAt?: number
  lastIncrementalAt?: number
}

export interface SearchRequest {
  roots: Array<SearchRootInput>
  query: string
  limit?: number
  includeHidden?: boolean
  includeIgnored?: boolean
  extensions?: Array<string>
}

export interface SearchResultDto {
  path: string
  score: number
  provider: string
  matches: Array<string>
}

export interface SearchRootInput {
  path: string
  enabled?: boolean
  priority?: number
  includeHidden?: boolean
  includeIgnored?: boolean
}

export interface SharedStoreOptions {
  home?: string
  consumer?: string
}
````

<!-- apply {"task": "M2.8", "file": "crates/kuntu-napi/test/index.test.cjs", "mode": "file"} -->
````javascript
// End-to-end tests for the local NAPI package consumed from Node/Electron runtimes.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { test } = require("node:test");

const { FileSearchIndex } = require("../index.js");

async function withFixture(run) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "kfs-napi-"));
  try {
    return await run(dir);
  } finally {
    await fs.rm(dir, { recursive: true, force: true });
  }
}

test("FileSearchIndex rebuilds, reports status, and searches indexed files", async () => {
  await withFixture(async (dir) => {
    const root = path.join(dir, "root");
    const src = path.join(root, "src");
    await fs.mkdir(src, { recursive: true });
    const canonicalRoot = await fs.realpath(root);
    await fs.writeFile(path.join(root, "package.json"), "{}\n");
    await fs.writeFile(path.join(src, "notes.txt"), "notes\n");

    const index = new FileSearchIndex(path.join(dir, "kfs.sqlite"));
    const rebuild = await index.rebuild([{ path: root }]);
    assert.equal(rebuild.roots, 1);
    assert.ok(rebuild.entries >= 2);
    assert.equal(rebuild.errors.length, 0);

    const status = await index.status();
    assert.equal(status.length, 1);
    assert.equal(status[0].path, canonicalRoot);
    assert.ok(status[0].entryCount >= 2);
    assert.equal(status[0].dirty, false);

    const outcome = await index.search({
      roots: [{ path: root }],
      query: "package json",
      limit: 5,
    });

    assert.ok(outcome.candidateCount >= 1);
    assert.equal(outcome.results[0].path, path.join(canonicalRoot, "package.json"));
    assert.equal(outcome.results[0].provider, "sqlite");
    assert.ok(outcome.results[0].score > 0);
    assert.ok(outcome.results[0].matches.includes("BasenameToken"));
  });
});

test("FileSearchIndex refresh updates indexed results", async () => {
  await withFixture(async (dir) => {
    const root = path.join(dir, "root");
    await fs.mkdir(root, { recursive: true });
    const canonicalRoot = await fs.realpath(root);

    const index = new FileSearchIndex(path.join(dir, "kfs.sqlite"));
    await index.rebuild([{ path: root }]);

    await fs.writeFile(path.join(root, "readme.md"), "hello\n");
    const refresh = await index.refresh([{ path: root }]);
    assert.equal(refresh.roots, 1);
    assert.equal(refresh.inserted, 1);
    assert.equal(refresh.errors.length, 0);

    const outcome = await index.search({
      roots: [{ path: root }],
      query: "readme",
      extensions: ["md"],
      limit: 5,
    });

    assert.equal(outcome.results.length, 1);
    assert.equal(outcome.results[0].path, path.join(canonicalRoot, "readme.md"));
  });
});

test("openReadOnly enforces access on rebuild refresh repair and registration", async () => {
  await withFixture(async (dir) => {
    const home=path.join(dir,"shared");
    const root=path.join(dir,"files");await fs.mkdir(root);await fs.writeFile(path.join(root,"visible-file.md"),"x");
    const writer=FileSearchIndex.openShared(home,"strict");writer.registerRoots([{path:root}]);
    const reader=FileSearchIndex.openReadOnly(home,"strict");
    assert.equal(reader.mode(),"shared:read-only");
    for(const method of ["rebuild","refresh","repair"])await assert.rejects(reader[method]([{path:root}]),/read-only/);
    assert.throws(()=>reader.registerRoots([{path:root}]),/read-only/);
    assert.throws(()=>reader.requestRebuild([{path:root}]),/read-only/);
    await assert.rejects(reader.search({roots:[],query:""}),/scope/);
    await assert.rejects(reader.search({roots:[{path:path.join(dir,"foreign")}],query:"file"}),/scope/);
    const rows=await reader.status();assert.equal(rows.length,1);writer.close();reader.close();
  });
});

test("strict consumer ignores permissive indexing union and supplied query flags", async () => {
  await withFixture(async (dir) => {
    const home=path.join(dir,"shared");const root=path.join(dir,"files");await fs.mkdir(path.join(root,"node_modules"),{recursive:true});
    await fs.writeFile(path.join(root,"visible-file.md"),"x");await fs.writeFile(path.join(root,".hidden-file.md"),"x");await fs.writeFile(path.join(root,"node_modules","ignored-file.md"),"x");
    const strict=FileSearchIndex.openShared(home,"strict");const permissive=FileSearchIndex.openShared(home,"permissive");
    try {
      strict.registerRoots([{path:root}]);permissive.registerRoots([{path:root,includeHidden:true,includeIgnored:true}]);
      const deadline=Date.now()+6000;while(true){const rows=await permissive.status();if(rows[0]?.status==="ready"&&rows[0].entryCount>=4)break;assert.ok(Date.now()<deadline);await new Promise(r=>setTimeout(r,10));}
      const request={roots:[{path:root,includeHidden:true,includeIgnored:true}],query:"file",includeHidden:true,includeIgnored:true};
      const denied=await strict.search(request);assert.ok(denied.results.some(r=>r.path.endsWith("visible-file.md")));assert.ok(denied.results.every(r=>!r.path.includes(".hidden-file")&&!r.path.includes("node_modules")));
      const allowed=await permissive.search(request);assert.ok(allowed.results.some(r=>r.path.includes(".hidden-file")));assert.ok(allowed.results.some(r=>r.path.includes("node_modules")));
    } finally {strict.close();permissive.close();}
  });
});


test("openClient registers and queues without starting an indexer", async () => {
 await withFixture(async dir => {
  const home=path.join(dir,"shared"),root=path.join(dir,"files");await fs.mkdir(root);
  const client=FileSearchIndex.openClient(home,"client");
  try {
   client.registerRoots([{path:root}]);assert.equal(typeof client.requestRebuild([{path:root}]),"string");
   await new Promise(r=>setTimeout(r,150));assert.equal((await client.status())[0].status,"dirty");
   assert.equal((await fs.readdir(path.join(home,"locks"))).includes("indexer.lock"),false);
  } finally {client.close();}
 });
});
````

## Task M2.9: CLI, bounded daemon and import dispatch

Install complete host files and the final generated lockfile. RED/GREEN: `non_holder_bulk_endpoint_queues_and_empty_search_is_rejected`. CLI default home is shared; `--db` retains the private compatibility path. Watch requires a capability; daemon read endpoints open read-only and bulk endpoints queue. Four scoped workers, eight pending connections, no detached connection threads. `kfs store info|gc|doctor|import SOURCE` has parsing and dispatch. Import acquires a destination capability, copies source sidecars without modifying them and rebuilds enabled roots. Also run `strict_daemon_admission_ignores_another_consumers_union`.

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/Cargo.toml", "mode": "file"} -->
````toml
# CLI adapter crate for exercising the Rust file search core locally.
[package]
name = "kuntu-cli"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
kuntu-store.workspace = true
kuntu-core = { path = "../kuntu-core" }
kuntu-daemon = { path = "../kuntu-daemon" }
kuntu-index = { path = "../kuntu-index" }
kuntu-provider-spotlight = { path = "../kuntu-provider-spotlight" }
kuntu-watcher = { path = "../kuntu-watcher" }

[[bin]]
name = "kfs"
path = "src/main.rs"

[lints]
workspace = true
````

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/src/args.rs", "mode": "file"} -->
````rust
//! Minimal argument parser for the local `kfs` CLI.
//!
//! The parser intentionally stays dependency-free while the Rust prototype is
//! being shaped. Keeping it small also makes provider and index command
//! behavior easy to test.

use std::path::PathBuf;

use kuntu_core::{SearchQuery, SearchRoot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
  pub roots: Vec<SearchRoot>,
  pub db_path: Option<PathBuf>,
  pub provider: SearchProvider,
  pub command: CommandSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchProvider {
  Spotlight,
  Sqlite,
  Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSpec {
  Search {
    query: SearchQuery,
    json: bool,
  },
  Explain {
    path: PathBuf,
    query: Option<SearchQuery>,
  },
  Bench {
    query: SearchQuery,
  },
  Store {
    operation: String,
    source: Option<PathBuf>,
  },
  Index(IndexCommand),
  Watch {
    duration_ms: u64,
  },
  Daemon {
    addr: String,
    duration_ms: Option<u64>,
    allow_non_loopback: bool,
  },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexCommand {
  Rebuild,
  Refresh,
  Repair,
  Status,
}

pub fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
  if args.is_empty() {
    return Err(usage());
  }

  match args[0].as_str() {
    "store" => {
      let operation = args.get(1).ok_or_else(usage)?.clone();
      if !matches!(operation.as_str(), "info" | "gc" | "doctor" | "import") {
        return Err(usage());
      }
      let source = if operation == "import" {
        Some(PathBuf::from(args.get(2).ok_or_else(usage)?))
      } else {
        None
      };
      if args.len() != if source.is_some() { 3 } else { 2 } {
        return Err(usage());
      }
      Ok(ParsedArgs {
        roots: Vec::new(),
        db_path: None,
        provider: SearchProvider::Sqlite,
        command: CommandSpec::Store { operation, source },
      })
    }
    "search" => parse_search(&args[1..]),
    "explain" => parse_explain(&args[1..]),
    "bench" => parse_bench(&args[1..]),
    "index" => parse_index(&args[1..]),
    "watch" => parse_watch(&args[1..]),
    "daemon" => parse_daemon(&args[1..]),
    _ => Err(usage()),
  }
}

fn parse_search(args: &[String]) -> Result<ParsedArgs, String> {
  let (query_text, rest) = first_value(args, "search requires a query")?;
  let mut parsed = parse_common(rest)?;
  let query = parsed.query(query_text);
  let json = parsed.json;
  Ok(ParsedArgs {
    roots: parsed.take_roots()?,
    db_path: parsed.db_path,
    provider: parsed.provider,
    command: CommandSpec::Search { query, json },
  })
}

fn parse_explain(args: &[String]) -> Result<ParsedArgs, String> {
  let (path, rest) = first_value(args, "explain requires a path")?;
  let mut parsed = parse_common(rest)?;
  let query = parsed.query_text.take().map(SearchQuery::new);
  Ok(ParsedArgs {
    roots: parsed.take_roots()?,
    db_path: parsed.db_path,
    provider: parsed.provider,
    command: CommandSpec::Explain {
      path: PathBuf::from(path),
      query,
    },
  })
}

fn parse_bench(args: &[String]) -> Result<ParsedArgs, String> {
  let (query_text, rest) = first_value(args, "bench requires a query")?;
  let mut parsed = parse_common(rest)?;
  let query = parsed.query(query_text);
  Ok(ParsedArgs {
    roots: parsed.take_roots()?,
    db_path: parsed.db_path,
    provider: parsed.provider,
    command: CommandSpec::Bench { query },
  })
}

fn parse_index(args: &[String]) -> Result<ParsedArgs, String> {
  let Some(subcommand) = args.first() else {
    return Err("index requires rebuild or status".to_string());
  };
  let mut parsed = parse_common(&args[1..])?;
  let (roots, command) = match subcommand.as_str() {
    "rebuild" => {
      parsed.require_db()?;
      (parsed.take_roots()?, IndexCommand::Rebuild)
    }
    "refresh" => {
      parsed.require_db()?;
      (parsed.take_roots()?, IndexCommand::Refresh)
    }
    "repair" => {
      parsed.require_db()?;
      (parsed.take_roots()?, IndexCommand::Repair)
    }
    "status" => {
      parsed.require_db()?;
      (Vec::new(), IndexCommand::Status)
    }
    unknown => return Err(format!("unknown index command: {unknown}")),
  };
  Ok(ParsedArgs {
    roots,
    db_path: parsed.db_path,
    provider: parsed.provider,
    command: CommandSpec::Index(command),
  })
}

fn parse_watch(args: &[String]) -> Result<ParsedArgs, String> {
  let mut parsed = parse_common(args)?;
  parsed.require_db()?;
  let duration_ms = parsed.require_duration_ms()?;
  Ok(ParsedArgs {
    roots: parsed.take_roots()?,
    db_path: parsed.db_path,
    provider: parsed.provider,
    command: CommandSpec::Watch { duration_ms },
  })
}

fn parse_daemon(args: &[String]) -> Result<ParsedArgs, String> {
  let mut parsed = parse_common(args)?;
  parsed.require_db()?;
  let addr = parsed
    .addr
    .clone()
    .ok_or_else(|| "--addr is required for daemon".to_string())?;
  Ok(ParsedArgs {
    roots: parsed.take_roots()?,
    db_path: parsed.db_path,
    provider: parsed.provider,
    command: CommandSpec::Daemon {
      addr,
      duration_ms: parsed.duration_ms,
      allow_non_loopback: parsed.allow_non_loopback,
    },
  })
}

fn first_value<'a>(
  args: &'a [String],
  error: &'static str,
) -> Result<(&'a str, &'a [String]), String> {
  let Some(value) = args.first() else {
    return Err(error.to_string());
  };
  if value.starts_with("--") {
    return Err(error.to_string());
  }
  Ok((value.as_str(), &args[1..]))
}

#[derive(Debug)]
struct CommonArgs {
  roots: Vec<SearchRoot>,
  limit: Option<usize>,
  json: bool,
  include_hidden: bool,
  include_ignored: bool,
  extensions: Vec<String>,
  query_text: Option<String>,
  duration_ms: Option<u64>,
  addr: Option<String>,
  allow_non_loopback: bool,
  db_path: Option<PathBuf>,
  provider: SearchProvider,
}

impl Default for CommonArgs {
  fn default() -> Self {
    Self {
      roots: Vec::new(),
      limit: None,
      json: false,
      include_hidden: false,
      include_ignored: false,
      extensions: Vec::new(),
      query_text: None,
      duration_ms: None,
      addr: None,
      allow_non_loopback: false,
      db_path: None,
      provider: SearchProvider::Spotlight,
    }
  }
}

impl CommonArgs {
  fn take_roots(&mut self) -> Result<Vec<SearchRoot>, String> {
    if self.roots.is_empty() {
      return Err("at least one --root is required".to_string());
    }
    Ok(std::mem::take(&mut self.roots))
  }

  fn require_db(&self) -> Result<(), String> {
    Ok(())
  }

  fn require_duration_ms(&self) -> Result<u64, String> {
    self
      .duration_ms
      .ok_or_else(|| "--duration-ms is required for watch".to_string())
  }

  fn query(&mut self, query_text: &str) -> SearchQuery {
    let mut query = SearchQuery::new(query_text);
    if let Some(limit) = self.limit {
      query.limit = limit;
    }
    query.include_hidden = self.include_hidden;
    query.include_ignored = self.include_ignored;
    query.extensions = std::mem::take(&mut self.extensions);
    query
  }
}

fn parse_common(args: &[String]) -> Result<CommonArgs, String> {
  let mut parsed = CommonArgs::default();
  let mut index = 0;
  while index < args.len() {
    match args[index].as_str() {
      "--root" => {
        index += 1;
        let value = args
          .get(index)
          .ok_or_else(|| "--root requires a path".to_string())?;
        parsed.roots.push(SearchRoot::new(value));
      }
      "--db" => {
        index += 1;
        let value = args
          .get(index)
          .ok_or_else(|| "--db requires a path".to_string())?;
        parsed.db_path = Some(PathBuf::from(value));
      }
      "--provider" => {
        index += 1;
        let value = args
          .get(index)
          .ok_or_else(|| "--provider requires sqlite, spotlight, or auto".to_string())?;
        parsed.provider = parse_provider(value)?;
      }
      "--limit" => {
        index += 1;
        let value = args
          .get(index)
          .ok_or_else(|| "--limit requires a number".to_string())?;
        parsed.limit = Some(
          value
            .parse::<usize>()
            .map_err(|_| "--limit must be a positive integer".to_string())?,
        );
      }
      "--duration-ms" => {
        index += 1;
        let value = args
          .get(index)
          .ok_or_else(|| "--duration-ms requires a number".to_string())?;
        parsed.duration_ms = Some(
          value
            .parse::<u64>()
            .map_err(|_| "--duration-ms must be a positive integer".to_string())?,
        );
      }
      "--addr" => {
        index += 1;
        let value = args
          .get(index)
          .ok_or_else(|| "--addr requires host:port".to_string())?;
        parsed.addr = Some(value.clone());
      }
      "--allow-non-loopback" => parsed.allow_non_loopback = true,
      "--json" => parsed.json = true,
      "--include-hidden" => parsed.include_hidden = true,
      "--include-ignored" => parsed.include_ignored = true,
      "--ext" => {
        index += 1;
        let value = args
          .get(index)
          .ok_or_else(|| "--ext requires an extension".to_string())?;
        parsed
          .extensions
          .push(value.trim_start_matches('.').to_ascii_lowercase());
      }
      "--query" => {
        index += 1;
        let value = args
          .get(index)
          .ok_or_else(|| "--query requires text".to_string())?;
        parsed.query_text = Some(value.clone());
      }
      unknown => return Err(format!("unknown argument: {unknown}")),
    }
    index += 1;
  }
  Ok(parsed)
}

fn parse_provider(value: &str) -> Result<SearchProvider, String> {
  match value {
    "spotlight" => Ok(SearchProvider::Spotlight),
    "sqlite" => Ok(SearchProvider::Sqlite),
    "auto" => Ok(SearchProvider::Auto),
    _ => Err("--provider must be sqlite, spotlight, or auto".to_string()),
  }
}

fn usage() -> String {
  "usage: kfs search <query> --root <path> [--provider sqlite|spotlight|auto] [--db path] [--limit n] [--json] | kfs index rebuild|refresh|repair --root <path> --db <path> | kfs index status --db <path> | kfs watch --root <path> --db <path> --duration-ms n | kfs daemon --root <path> --db <path> --addr host:port [--duration-ms n] [--allow-non-loopback]".to_string()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
  }

  #[test]
  fn search_requires_an_explicit_root() {
    let err = parse_args(&strings(&["search", "package json"])).unwrap_err();

    assert_eq!(err, "at least one --root is required");
  }

  #[test]
  fn parses_search_roots_limit_json_provider_db_and_extensions() {
    let parsed = parse_args(&strings(&[
      "search",
      "package json",
      "--root",
      "/Users/alice/Dev",
      "--limit",
      "7",
      "--json",
      "--ext",
      "json",
      "--provider",
      "sqlite",
      "--db",
      "/tmp/kfs.sqlite",
    ]))
    .unwrap();

    assert_eq!(parsed.roots, vec![SearchRoot::new("/Users/alice/Dev")]);
    assert_eq!(parsed.db_path, Some(PathBuf::from("/tmp/kfs.sqlite")));
    assert_eq!(parsed.provider, SearchProvider::Sqlite);
    assert_eq!(
      parsed.command,
      CommandSpec::Search {
        query: SearchQuery {
          query: "package json".to_string(),
          limit: 7,
          include_hidden: false,
          include_ignored: false,
          include_files: true,
          include_directories: true,
          extensions: vec!["json".to_string()],
        },
        json: true,
      }
    );
  }

  #[test]
  fn parses_explain_with_optional_query() {
    let parsed = parse_args(&strings(&[
      "explain",
      "/Users/alice/Dev/project/package.json",
      "--root",
      "/Users/alice/Dev",
      "--query",
      "package json",
    ]))
    .unwrap();

    assert_eq!(
      parsed.command,
      CommandSpec::Explain {
        path: PathBuf::from("/Users/alice/Dev/project/package.json"),
        query: Some(SearchQuery::new("package json")),
      }
    );
  }

  #[test]
  fn parses_bench() {
    let parsed = parse_args(&strings(&[
      "bench",
      "main rs",
      "--root",
      "/Users/alice/Dev",
      "--limit",
      "3",
      "--provider",
      "auto",
    ]))
    .unwrap();

    assert_eq!(parsed.provider, SearchProvider::Auto);
    assert_eq!(
      parsed.command,
      CommandSpec::Bench {
        query: SearchQuery {
          query: "main rs".to_string(),
          limit: 3,
          include_hidden: false,
          include_ignored: false,
          include_files: true,
          include_directories: true,
          extensions: Vec::new(),
        },
      }
    );
  }

  #[test]
  fn parses_index_rebuild() {
    let parsed = parse_args(&strings(&[
      "index",
      "rebuild",
      "--root",
      "/Users/alice/Dev",
      "--db",
      "/tmp/kfs.sqlite",
    ]))
    .unwrap();

    assert_eq!(parsed.command, CommandSpec::Index(IndexCommand::Rebuild));
    assert_eq!(parsed.roots, vec![SearchRoot::new("/Users/alice/Dev")]);
    assert_eq!(parsed.db_path, Some(PathBuf::from("/tmp/kfs.sqlite")));
  }

  #[test]
  fn parses_index_status() {
    let parsed = parse_args(&strings(&["index", "status", "--db", "/tmp/kfs.sqlite"])).unwrap();

    assert_eq!(parsed.command, CommandSpec::Index(IndexCommand::Status));
    assert_eq!(parsed.db_path, Some(PathBuf::from("/tmp/kfs.sqlite")));
  }

  #[test]
  fn parses_index_refresh() {
    let parsed = parse_args(&strings(&[
      "index",
      "refresh",
      "--root",
      "/Users/alice/Dev",
      "--db",
      "/tmp/kfs.sqlite",
    ]))
    .unwrap();

    assert_eq!(parsed.command, CommandSpec::Index(IndexCommand::Refresh));
    assert_eq!(parsed.roots, vec![SearchRoot::new("/Users/alice/Dev")]);
    assert_eq!(parsed.db_path, Some(PathBuf::from("/tmp/kfs.sqlite")));
  }

  #[test]
  fn parses_index_repair() {
    let parsed = parse_args(&strings(&[
      "index",
      "repair",
      "--root",
      "/Users/alice/Dev",
      "--db",
      "/tmp/kfs.sqlite",
    ]))
    .unwrap();

    assert_eq!(parsed.command, CommandSpec::Index(IndexCommand::Repair));
    assert_eq!(parsed.roots, vec![SearchRoot::new("/Users/alice/Dev")]);
    assert_eq!(parsed.db_path, Some(PathBuf::from("/tmp/kfs.sqlite")));
  }

  #[test]
  fn parses_watch_with_required_duration() {
    let parsed = parse_args(&strings(&[
      "watch",
      "--root",
      "/Users/alice/Dev",
      "--db",
      "/tmp/kfs.sqlite",
      "--duration-ms",
      "250",
    ]))
    .unwrap();

    assert_eq!(parsed.command, CommandSpec::Watch { duration_ms: 250 });
    assert_eq!(parsed.roots, vec![SearchRoot::new("/Users/alice/Dev")]);
  }

  #[test]
  fn watch_requires_duration_to_stay_bounded() {
    let err = parse_args(&strings(&[
      "watch",
      "--root",
      "/Users/alice/Dev",
      "--db",
      "/tmp/kfs.sqlite",
    ]))
    .unwrap_err();

    assert_eq!(err, "--duration-ms is required for watch");
  }

  #[test]
  fn parses_daemon_with_optional_duration() {
    let parsed = parse_args(&strings(&[
      "daemon",
      "--root",
      "/Users/alice/Dev",
      "--db",
      "/tmp/kfs.sqlite",
      "--addr",
      "127.0.0.1:0",
      "--duration-ms",
      "1000",
    ]))
    .unwrap();

    assert_eq!(
      parsed.command,
      CommandSpec::Daemon {
        addr: "127.0.0.1:0".to_string(),
        duration_ms: Some(1000),
        allow_non_loopback: false
      }
    );
    assert_eq!(parsed.roots, vec![SearchRoot::new("/Users/alice/Dev")]);
    assert_eq!(parsed.db_path, Some(PathBuf::from("/tmp/kfs.sqlite")));
  }

  #[test]
  fn parses_daemon_non_loopback_opt_in() {
    let parsed = parse_args(&strings(&[
      "daemon",
      "--root",
      "/Users/alice/Dev",
      "--db",
      "/tmp/kfs.sqlite",
      "--addr",
      "0.0.0.0:47865",
      "--allow-non-loopback",
    ]))
    .unwrap();

    assert_eq!(
      parsed.command,
      CommandSpec::Daemon {
        addr: "0.0.0.0:47865".to_string(),
        duration_ms: None,
        allow_non_loopback: true
      }
    );
  }
  #[test]
  fn store_import_parser_accepts_source_and_rejects_missing_source() {
    let a = parse_args(&["store".into(), "import".into(), "/source/v2.db".into()]).unwrap();
    assert!(matches!(
      a.command,
      CommandSpec::Store {
        source: Some(_),
        ..
      }
    ));
    assert!(parse_args(&["store".into(), "import".into()]).is_err());
  }
}
````

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/src/main.rs", "mode": "file"} -->
````rust
//! Command-line adapter for the independent Rust file search prototype.
//!
//! The CLI exists for local testing and future integration experiments. It
//! requires explicit roots and delegates filtering/ranking to `kuntu-core`.

mod args;
mod output;

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use args::{parse_args, CommandSpec, IndexCommand, SearchProvider};
use kuntu_core::{
  CandidateProvider, MetadataIndex, SearchConfig, SearchEngineCore, SearchQuery, SearchResult,
};
use kuntu_daemon::{serve, DaemonConfig};
use kuntu_index::KuntuIndex;
use kuntu_provider_spotlight::SpotlightProvider;
use kuntu_store::Home;
use kuntu_watcher::{run_polling_watch, WatchOptions};
use output::{
  format_bench_stats, format_daemon_stats, format_explain_text, format_rebuild_stats,
  format_refresh_stats, format_repair_stats, format_results_json, format_results_text,
  format_status, format_watch_stats,
};

#[derive(Debug, Clone)]
struct SearchRun {
  results: Vec<SearchResult>,
  candidate_count: usize,
}

fn main() -> ExitCode {
  match run(std::env::args().skip(1).collect()) {
    Ok(text) => {
      println!("{text}");
      ExitCode::SUCCESS
    }
    Err(err) => {
      eprintln!("kfs: {err}");
      ExitCode::from(2)
    }
  }
}

fn run(raw_args: Vec<String>) -> Result<String, String> {
  let spec = parse_args(&raw_args)?;
  let default_db = if spec.db_path.is_none() {
    Some(
      kuntu_store::resolve()
        .map_err(|e| e.to_string())?
        .join("index.v3.db"),
    )
  } else {
    None
  };
  let db_path = spec.db_path.as_deref().or(default_db.as_deref());
  let config = SearchConfig { roots: spec.roots };
  let core = SearchEngineCore::new(config.clone());

  match spec.command {
    CommandSpec::Store { operation, source } => {
      let home = Home::open(&kuntu_store::resolve().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
      match operation.as_str() {
        "info" => Ok(format!(
          "store_id={} epoch=3",
          home.id().map_err(|e| e.to_string())?
        )),
        "gc" => {
          let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
          let mut count = 0;
          for epoch in 1..3 {
            if home.gc_epoch(epoch, now).map_err(|e| e.to_string())? {
              count += 1;
            }
          }
          Ok(format!("removed={count}"))
        }
        "doctor" => {
          let index = KuntuIndex::open_shared(home, true, "cli").map_err(|e| e.to_string())?;
          index.doctor().map_err(|e| e.to_string())
        }
        "import" => {
          let mut index = KuntuIndex::open_shared(home, false, "cli").map_err(|e| e.to_string())?;
          if !index.acquire_writer().map_err(|e| e.to_string())? {
            return Err("Indexer lease busy.".into());
          }
          let stats = index
            .import_v2(
              source
                .as_deref()
                .ok_or_else(|| "Missing import source.".to_string())?,
            )
            .map_err(|e| e.to_string())?;
          Ok(format_rebuild_stats(&stats))
        }
        _ => Err("Invalid store operation.".into()),
      }
    }
    CommandSpec::Search { query, json } => {
      let search = search_with_provider(&config, &core, &query, spec.provider, db_path)?;
      if json {
        Ok(format_results_json(&search.results))
      } else {
        Ok(format_results_text(&search.results))
      }
    }
    CommandSpec::Explain { path, query } => {
      let explain = core.explain_path(&path, query.as_ref());
      Ok(format_explain_text(&explain))
    }
    CommandSpec::Bench { query } => {
      let started = Instant::now();
      let search = search_with_provider(&config, &core, &query, spec.provider, db_path)?;
      Ok(format_bench_stats(
        search.candidate_count,
        search.results.len(),
        started.elapsed().as_millis(),
      ))
    }
    CommandSpec::Index(command) => run_index_command(&config, db_path, command),
    CommandSpec::Watch { duration_ms } => run_watch_command(&config, db_path, duration_ms),
    CommandSpec::Daemon {
      addr,
      duration_ms,
      allow_non_loopback,
    } => run_daemon_command(&config, db_path, addr, duration_ms, allow_non_loopback),
  }
}

fn search_with_provider(
  config: &SearchConfig,
  core: &SearchEngineCore,
  query: &SearchQuery,
  provider: SearchProvider,
  db_path: Option<&Path>,
) -> Result<SearchRun, String> {
  match provider {
    SearchProvider::Spotlight => search_spotlight(config, core, query),
    SearchProvider::Sqlite => {
      let path = db_path.ok_or_else(|| "--db is required for sqlite provider".to_string())?;
      search_sqlite(config, query, path)
    }
    SearchProvider::Auto => {
      if let Some(path) = db_path {
        let search = search_sqlite(config, query, path)?;
        if !search.results.is_empty() {
          return Ok(search);
        }
      }
      search_spotlight(config, core, query)
    }
  }
}

fn search_spotlight(
  config: &SearchConfig,
  core: &SearchEngineCore,
  query: &SearchQuery,
) -> Result<SearchRun, String> {
  let provider = SpotlightProvider::default();
  let candidates = provider
    .search_candidates(config, query)
    .map_err(|err| err.to_string())?;
  let candidate_count = candidates.len();
  Ok(SearchRun {
    results: core.search_candidates(query, candidates),
    candidate_count,
  })
}

fn open_cli_index(path: &Path, read_only: bool) -> kuntu_index::Result<KuntuIndex> {
  if path.file_name().is_some_and(|n| n == "index.v3.db") {
    KuntuIndex::open_shared(
      Home::open(
        path
          .parent()
          .ok_or_else(|| std::io::Error::other("Missing home."))?,
      )?,
      read_only,
      "cli",
    )
  } else if read_only {
    KuntuIndex::open_read_only(path)
  } else {
    KuntuIndex::open(path)
  }
}
fn search_sqlite(
  config: &SearchConfig,
  query: &SearchQuery,
  db_path: &Path,
) -> Result<SearchRun, String> {
  let index = open_cli_index(db_path, true).map_err(|err| err.to_string())?;
  let outcome = index
    .search_with_metrics(config, query)
    .map_err(|err| err.to_string())?;
  Ok(SearchRun {
    results: outcome.results,
    candidate_count: outcome.candidate_count,
  })
}

fn run_index_command(
  config: &SearchConfig,
  db_path: Option<&Path>,
  command: IndexCommand,
) -> Result<String, String> {
  let path = db_path.ok_or_else(|| "--db is required for index commands".to_string())?;
  let mut index = open_cli_index(path, false).map_err(|err| err.to_string())?;
  if index.is_shared() {
    for root in &config.roots {
      index.register_root(root).map_err(|e| e.to_string())?;
    }
    if !matches!(command, IndexCommand::Status)
      && !index.acquire_writer().map_err(|e| e.to_string())?
    {
      let kind = match command {
        IndexCommand::Rebuild => "rebuild",
        IndexCommand::Refresh => "refresh",
        IndexCommand::Repair => "repair",
        IndexCommand::Status => unreachable!(),
      };
      let id = index.enqueue(kind, config).map_err(|e| e.to_string())?;
      return Ok(format!("queued={id}"));
    }
  }
  match command {
    IndexCommand::Rebuild => {
      let stats = index.rebuild_index(config).map_err(|err| err.to_string())?;
      Ok(format_rebuild_stats(&stats))
    }
    IndexCommand::Refresh => {
      let stats = index.refresh_index(config).map_err(|err| err.to_string())?;
      Ok(format_refresh_stats(&stats))
    }
    IndexCommand::Repair => {
      let stats = index.repair_index(config).map_err(|err| err.to_string())?;
      Ok(format_repair_stats(&stats))
    }
    IndexCommand::Status => {
      let status = index.status().map_err(|err| err.to_string())?;
      let states = status
        .iter()
        .map(|r| {
          index
            .root_state_name(&r.path)
            .map(|s| format!("{} status={s}", r.path.display()))
            .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
      Ok(format!("{}\n{}", format_status(&status), states.join("\n")))
    }
  }
}

fn run_watch_command(
  config: &SearchConfig,
  db_path: Option<&Path>,
  duration_ms: u64,
) -> Result<String, String> {
  let path = db_path.ok_or_else(|| "--db is required for watch".to_string())?;
  let mut index = open_cli_index(path, false).map_err(|err| err.to_string())?;
  if index.is_shared() {
    for root in &config.roots {
      index.register_root(root).map_err(|e| e.to_string())?;
    }
    if !index.acquire_writer().map_err(|e| e.to_string())? {
      return Err("Indexer lease busy; watch requires a holder.".into());
    }
  }
  let options = WatchOptions::new(config.clone())
    .with_duration(Duration::from_millis(duration_ms))
    .with_poll_interval(Duration::from_millis(100));
  let stats = run_polling_watch(&options, |_event| index.refresh_index(config).map(|_| ()))
    .map_err(|err| err.to_string())?;
  if stats.dirty {
    for root in config.roots.iter().filter(|root| root.enabled) {
      index.mark_root_dirty(root).map_err(|err| err.to_string())?;
    }
  }
  Ok(format_watch_stats(&stats))
}

fn run_daemon_command(
  config: &SearchConfig,
  db_path: Option<&Path>,
  addr: String,
  duration_ms: Option<u64>,
  allow_non_loopback: bool,
) -> Result<String, String> {
  let path = db_path.ok_or_else(|| "--db is required for daemon".to_string())?;
  let stats = serve(DaemonConfig {
    db_path: path.to_path_buf(),
    addr,
    duration: duration_ms.map(Duration::from_millis),
    max_requests: None,
    allowed_roots: config.roots.clone(),
    allow_non_loopback,
  })
  .map_err(|err| err.to_string())?;
  Ok(format_daemon_stats(&stats))
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::path::PathBuf;
  use std::time::{SystemTime, UNIX_EPOCH};

  use super::*;

  #[test]
  fn bench_reports_candidate_and_result_counts_for_sqlite() {
    let root = temp_dir("bench-root");
    let db_path = temp_dir("bench-db").with_extension("sqlite");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("needlebench-target.md"), "needle\n").unwrap();
    fs::write(root.join("other-target.md"), "other\n").unwrap();

    run(vec![
      "index".to_string(),
      "rebuild".to_string(),
      "--root".to_string(),
      root.to_string_lossy().into_owned(),
      "--db".to_string(),
      db_path.to_string_lossy().into_owned(),
    ])
    .unwrap();
    let output = run(vec![
      "bench".to_string(),
      "needlebench".to_string(),
      "--root".to_string(),
      root.to_string_lossy().into_owned(),
      "--provider".to_string(),
      "sqlite".to_string(),
      "--db".to_string(),
      db_path.to_string_lossy().into_owned(),
    ])
    .unwrap();

    cleanup(&root, &db_path);

    assert!(output.contains("candidates=1"));
    assert!(output.contains("results=1"));
    assert!(output.contains("elapsed_ms="));
  }

  fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    std::env::temp_dir().join(format!("kuntu-cli-{name}-{nonce}"))
  }

  fn cleanup(root: &Path, db_path: &Path) {
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(db_path.with_extension("sqlite-shm"));
    let _ = fs::remove_file(db_path.with_extension("sqlite-wal"));
  }
}
````

<!-- apply {"task": "M2.9", "file": "crates/kuntu-daemon/Cargo.toml", "mode": "file"} -->
````toml
# HTTP daemon crate for the independent file-search workspace.
[package]
name = "kuntu-daemon"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
kuntu-core = { path = "../kuntu-core" }
kuntu-index = { path = "../kuntu-index" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
kuntu-store.workspace = true

[lib]
path = "src/lib.rs"

[lints]
workspace = true
````

<!-- apply {"task": "M2.9", "file": "crates/kuntu-daemon/src/lib.rs", "mode": "file"} -->
````rust
//! HTTP JSON daemon for the independent file-search Rust workspace.
//!
//! The daemon is intentionally framework-free and reuses the same core and
//! SQLite crates as the CLI. It is a service adapter, not the search API
//! boundary; future desktop, REST, or native hosts can reuse the same request
//! handlers or replace the transport.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use kuntu_core::{BackendError, MetadataIndex, SearchConfig, SearchQuery, SearchRoot};
use kuntu_index::{match_names, KuntuIndex, WriterService};
use kuntu_store::Home;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct DaemonConfig {
  pub db_path: PathBuf,
  pub addr: String,
  pub duration: Option<Duration>,
  pub max_requests: Option<usize>,
  pub allowed_roots: Vec<SearchRoot>,
  pub allow_non_loopback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonRunStats {
  pub addr: String,
  pub requests: usize,
  pub elapsed_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpRequest {
  method: String,
  path: String,
  body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpResponse {
  status: u16,
  body: String,
}

#[derive(Debug, Deserialize)]
struct RootsRequest {
  roots: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SearchRequest {
  query: String,
  roots: Vec<String>,
  limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct SearchResultDto {
  path: String,
  score: i32,
  provider: String,
  matches: Vec<String>,
}

pub fn serve(config: DaemonConfig) -> Result<DaemonRunStats, BackendError> {
  validate_bind_addr(&config)?;
  let listener = TcpListener::bind(&config.addr).map_err(BackendError::from)?;
  listener.set_nonblocking(true).map_err(BackendError::from)?;
  let addr = listener
    .local_addr()
    .map_err(BackendError::from)?
    .to_string();
  let started = Instant::now();
  let shared = config
    .db_path
    .file_name()
    .is_some_and(|n| n == "index.v3.db");
  let home = if shared {
    Some(
      Home::open(
        config
          .db_path
          .parent()
          .ok_or_else(|| BackendError::new("Missing store home."))?,
      )
      .map_err(BackendError::from)?,
    )
  } else {
    None
  };
  let _service = home.clone().map(WriterService::start);
  let init = if let Some(h) = &home {
    KuntuIndex::open_shared(h.clone(), false, "daemon")
  } else {
    KuntuIndex::open(&config.db_path)
  }
  .map_err(|e| BackendError::new(e.to_string()))?;
  for root in &config.allowed_roots {
    init
      .register_root(root)
      .map_err(|e| BackendError::new(e.to_string()))?;
  }
  drop(init);
  let mut requests = 0;
  let (sender, receiver) = std::sync::mpsc::sync_channel::<TcpStream>(8);
  let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
  thread::scope(|scope| -> Result<(), BackendError> {
    for _ in 0..4 {
      let receiver = receiver.clone();
      let home = home.clone();
      let config = &config;
      scope.spawn(move || loop {
        let stream = receiver.lock().expect("worker queue").recv();
        let Ok(stream) = stream else { break };
        let mut stream = stream;
        let request = match read_http_request(&mut stream) {
          Ok(Some(r)) => r,
          _ => continue,
        };
        let readonly = matches!(
          (request.method.as_str(), request.path.as_str()),
          ("GET", _) | ("POST", "/search")
        );
        let opened = if let Some(h) = &home {
          KuntuIndex::open_shared(h.clone(), readonly, "daemon")
        } else if readonly {
          KuntuIndex::open_read_only(&config.db_path)
        } else {
          KuntuIndex::open(&config.db_path)
        };
        if let Ok(mut index) = opened {
          let response = handle_request(&mut index, request, &config.allowed_roots);
          let _ = write_http_response(&mut stream, &response);
        }
      });
    }
    let outcome = (|| {
      loop {
        if config.duration.is_some_and(|d| started.elapsed() >= d)
          || config.max_requests.is_some_and(|m| requests >= m)
        {
          break;
        }
        match listener.accept() {
          Ok((stream, _)) => {
            sender
              .send(stream)
              .map_err(|e| BackendError::new(e.to_string()))?;
            requests += 1;
          }
          Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            thread::sleep(Duration::from_millis(10))
          }
          Err(e) => return Err(BackendError::from(e)),
        }
      }
      Ok(())
    })();
    drop(sender);
    outcome
  })?;
  Ok(DaemonRunStats {
    addr,
    requests,
    elapsed_ms: started.elapsed().as_millis(),
  })
}

fn handle_request(
  index: &mut KuntuIndex,
  request: HttpRequest,
  allowed_roots: &[SearchRoot],
) -> HttpResponse {
  match (request.method.as_str(), request.path.as_str()) {
    ("GET", "/health") => json_response(200, json!({ "ok": true })),
    ("GET", "/status") => match index.status() {
      Ok(status) => json_response(
        200,
        json!({
            "roots": status.into_iter().map(|root| {
                json!({
                    "path": root.path.to_string_lossy(),
                    "entry_count": root.entry_count,
                    "generation": root.generation,
                    "dirty": root.dirty,
                    "status": index.root_state_name(&root.path).unwrap_or_else(|_|"dirty".into()),
                    "last_full_scan_at": root.last_full_scan_at,
                    "last_incremental_at": root.last_incremental_at
                })
            }).collect::<Vec<_>>()
        }),
      ),
      Err(err) => json_response(500, json!({ "error": err.to_string() })),
    },
    ("POST", "/search") => match parse_json::<SearchRequest>(&request.body) {
      Ok(search) => {
        let config = match config_from_requested_roots(&search.roots, allowed_roots) {
          Ok(config) => config,
          Err(err) => return json_response(400, json!({ "error": err.message })),
        };
        let mut query = SearchQuery::new(search.query);
        if let Some(limit) = search.limit {
          query.limit = limit;
        }
        match index.search_with_metrics(&config, &query) {
          Ok(outcome) => json_response(
            200,
            json!({
                "candidate_count": outcome.candidate_count,
                "truncated": outcome.truncated,
                "coverage": match outcome.coverage {
                    kuntu_index::CandidateCoverage::Complete => "complete",
                    kuntu_index::CandidateCoverage::Truncated => "truncated",
                    kuntu_index::CandidateCoverage::Building => "building",
                },
                "results": outcome.results.into_iter().map(SearchResultDto::from).collect::<Vec<_>>()
            }),
          ),
          Err(err) => json_response(500, json!({ "error": err.to_string() })),
        }
      }
      Err(err) => json_response(400, json!({ "error": err.message })),
    },
    ("POST", "/index/rebuild") => match parse_roots_request(&request.body, allowed_roots) {
      Ok(config) => {
        if index.is_shared() {
          return match index.enqueue("rebuild", &config) {
            Ok(id) => json_response(202, json!({"request_id":id})),
            Err(e) => json_response(409, json!({"error":e.to_string()})),
          };
        }
        match index.rebuild_index(&config) {
          Ok(stats) => json_response(
            200,
            json!({
                "stats": {
                    "roots": stats.roots,
                    "entries": stats.entries,
                    "skipped": stats.skipped,
                    "errors": stats.errors
                }
            }),
          ),
          Err(err) => json_response(500, json!({ "error": err.to_string() })),
        }
      }
      Err(err) => json_response(400, json!({ "error": err.message })),
    },
    ("POST", "/index/refresh") => match parse_roots_request(&request.body, allowed_roots) {
      Ok(config) => {
        if index.is_shared() {
          return match index.enqueue("refresh", &config) {
            Ok(id) => json_response(202, json!({"request_id":id})),
            Err(e) => json_response(409, json!({"error":e.to_string()})),
          };
        }
        match index.refresh_index(&config) {
          Ok(stats) => json_response(
            200,
            json!({
                "stats": {
                    "roots": stats.roots,
                    "inserted": stats.inserted,
                    "updated": stats.updated,
                    "deleted": stats.deleted,
                    "unchanged": stats.unchanged,
                    "skipped": stats.skipped,
                    "errors": stats.errors
                }
            }),
          ),
          Err(err) => json_response(500, json!({ "error": err.to_string() })),
        }
      }
      Err(err) => json_response(400, json!({ "error": err.message })),
    },
    _ => json_response(404, json!({ "error": "not found" })),
  }
}

fn read_http_request(stream: &mut TcpStream) -> Result<Option<HttpRequest>, BackendError> {
  stream
    .set_read_timeout(Some(Duration::from_secs(2)))
    .map_err(BackendError::from)?;
  let mut buffer = Vec::new();
  let mut chunk = [0_u8; 4096];
  loop {
    match stream.read(&mut chunk) {
      Ok(0) => break,
      Ok(count) => {
        buffer.extend_from_slice(&chunk[..count]);
        if request_is_complete(&buffer) {
          break;
        }
      }
      Err(err)
        if matches!(
          err.kind(),
          std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) =>
      {
        break;
      }
      Err(err) => return Err(BackendError::from(err)),
    }
  }
  if buffer.is_empty() {
    return Ok(None);
  }
  parse_http_request(&buffer).map(Some)
}

fn parse_http_request(buffer: &[u8]) -> Result<HttpRequest, BackendError> {
  let header_end =
    find_header_end(buffer).ok_or_else(|| BackendError::new("request headers were incomplete"))?;
  let headers = String::from_utf8_lossy(&buffer[..header_end]);
  let mut lines = headers.lines();
  let request_line = lines
    .next()
    .ok_or_else(|| BackendError::new("missing request line"))?;
  let mut parts = request_line.split_whitespace();
  let method = parts
    .next()
    .ok_or_else(|| BackendError::new("missing method"))?
    .to_string();
  let path = parts
    .next()
    .ok_or_else(|| BackendError::new("missing path"))?
    .to_string();
  Ok(HttpRequest {
    method,
    path,
    body: buffer[header_end + 4..].to_vec(),
  })
}

fn request_is_complete(buffer: &[u8]) -> bool {
  let Some(header_end) = find_header_end(buffer) else {
    return false;
  };
  let content_length = content_length(&buffer[..header_end]).unwrap_or(0);
  buffer.len() >= header_end + 4 + content_length
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
  buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(headers: &[u8]) -> Option<usize> {
  let headers = String::from_utf8_lossy(headers);
  headers.lines().find_map(|line| {
    let (name, value) = line.split_once(':')?;
    if name.eq_ignore_ascii_case("content-length") {
      value.trim().parse::<usize>().ok()
    } else {
      None
    }
  })
}

fn write_http_response(
  stream: &mut TcpStream,
  response: &HttpResponse,
) -> Result<(), BackendError> {
  let reason = match response.status {
    200 => "OK",
    400 => "Bad Request",
    404 => "Not Found",
    500 => "Internal Server Error",
    _ => "OK",
  };
  let text = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        reason,
        response.body.len(),
        response.body
    );
  stream
    .write_all(text.as_bytes())
    .map_err(BackendError::from)
}

fn parse_roots_request(
  body: &[u8],
  allowed_roots: &[SearchRoot],
) -> Result<SearchConfig, BackendError> {
  let request = parse_json::<RootsRequest>(body)?;
  config_from_requested_roots(&request.roots, allowed_roots)
}

fn parse_json<T>(body: &[u8]) -> Result<T, BackendError>
where
  T: for<'de> Deserialize<'de>,
{
  serde_json::from_slice(body).map_err(|err| BackendError::new(err.to_string()))
}

fn config_from_requested_roots(
  roots: &[String],
  allowed_roots: &[SearchRoot],
) -> Result<SearchConfig, BackendError> {
  if roots.is_empty() {
    return Err(BackendError::new("at least one root is required"));
  }
  let allowed = allowed_roots
    .iter()
    .filter(|root| root.enabled)
    .collect::<Vec<_>>();
  if allowed.is_empty() {
    return Err(BackendError::new("daemon has no allowed roots"));
  }

  let mut requested_roots = Vec::with_capacity(roots.len());
  for root in roots {
    let requested = SearchRoot::new(root);
    let Some(allowed_root) = allowed
      .iter()
      .copied()
      .find(|allowed_root| requested.path.starts_with(&allowed_root.path))
    else {
      return Err(BackendError::new(format!(
        "root is outside daemon allowed roots: {}",
        requested.path.to_string_lossy()
      )));
    };
    requested_roots.push(
      requested
        .include_hidden(allowed_root.include_hidden)
        .include_ignored(allowed_root.include_ignored)
        .with_priority(allowed_root.priority),
    );
  }

  Ok(SearchConfig {
    roots: requested_roots,
  })
}

pub fn validate_bind_addr(config: &DaemonConfig) -> Result<(), BackendError> {
  if config.allow_non_loopback || addr_is_loopback(&config.addr) {
    return Ok(());
  }
  Err(BackendError::new(
    "non-loopback daemon bind requires --allow-non-loopback",
  ))
}

fn addr_is_loopback(addr: &str) -> bool {
  if let Ok(socket_addr) = addr.parse::<SocketAddr>() {
    return socket_addr.ip().is_loopback();
  }
  addr
    .split_once(':')
    .is_some_and(|(host, _port)| host == "localhost")
}

fn json_response(status: u16, value: serde_json::Value) -> HttpResponse {
  HttpResponse {
    status,
    body: value.to_string(),
  }
}

impl From<kuntu_core::SearchResult> for SearchResultDto {
  fn from(result: kuntu_core::SearchResult) -> Self {
    Self {
      path: result.path.to_string_lossy().into_owned(),
      score: result.score,
      provider: result.provider,
      matches: match_names(&result.matches)
        .into_iter()
        .map(ToOwned::to_owned)
        .collect(),
    }
  }
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::time::{SystemTime, UNIX_EPOCH};

  use super::*;

  #[test]
  fn parse_http_request_reads_method_path_and_body() {
    let request =
      parse_http_request(b"POST /search HTTP/1.1\r\nContent-Length: 13\r\n\r\n{\"query\":\"x\"}")
        .unwrap();

    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/search");
    assert_eq!(request.body, br#"{"query":"x"}"#);
  }

  #[test]
  fn health_endpoint_returns_ok_json() {
    let mut index = KuntuIndex::open_memory().unwrap();

    let response = handle_request(
      &mut index,
      HttpRequest {
        method: "GET".to_string(),
        path: "/health".to_string(),
        body: Vec::new(),
      },
      &[],
    );

    assert_eq!(response.status, 200);
    assert!(response.body.contains("\"ok\":true"));
  }

  #[test]
  fn search_endpoint_returns_indexed_results() {
    let root = temp_dir("search");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("daemon-target.md"), "daemon\n").unwrap();
    let mut index = KuntuIndex::open_memory().unwrap();
    let allowed_roots = vec![SearchRoot::new(&root)];
    // Paths are serialized, not format!-ed: a Windows path's backslashes are
    // JSON escapes, and a hand-built literal is invalid JSON off POSIX.
    let rebuild = handle_request(
      &mut index,
      HttpRequest {
        method: "POST".to_string(),
        path: "/index/rebuild".to_string(),
        body: serde_json::to_vec(&json!({ "roots": [root.to_string_lossy()] })).unwrap(),
      },
      &allowed_roots,
    );
    let response = handle_request(
      &mut index,
      HttpRequest {
        method: "POST".to_string(),
        path: "/search".to_string(),
        body: serde_json::to_vec(&json!({
          "query": "daemon target",
          "roots": [root.to_string_lossy()],
          "limit": 5
        }))
        .unwrap(),
      },
      &allowed_roots,
    );
    let _ = fs::remove_dir_all(&root);

    assert_eq!(rebuild.status, 200);
    assert_eq!(response.status, 200);
    assert!(response.body.contains("daemon-target.md"));
    assert!(response.body.contains("candidate_count"));
  }

  #[test]
  fn rejects_roots_outside_daemon_allowlist() {
    let allowed = temp_dir("allowed");
    let denied = temp_dir("denied");
    fs::create_dir_all(&allowed).unwrap();
    fs::create_dir_all(&denied).unwrap();
    let allowed_roots = vec![SearchRoot::new(&allowed)];

    let err = config_from_requested_roots(&[denied.to_string_lossy().into_owned()], &allowed_roots)
      .unwrap_err();

    let _ = fs::remove_dir_all(&allowed);
    let _ = fs::remove_dir_all(&denied);

    assert!(err.message.contains("outside daemon allowed roots"));
  }

  #[test]
  fn daemon_rejects_non_loopback_bind_without_opt_in() {
    let config = DaemonConfig {
      db_path: PathBuf::from("/tmp/kfs-review-daemon.sqlite"),
      addr: "0.0.0.0:0".to_string(),
      duration: Some(Duration::from_millis(1)),
      max_requests: Some(0),
      allowed_roots: Vec::new(),
      allow_non_loopback: false,
    };

    let err = validate_bind_addr(&config).unwrap_err();

    assert!(err.message.contains("non-loopback"));
  }

  fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    std::env::temp_dir().join(format!("kuntu-daemon-{name}-{nonce}"))
  }
  #[test]
  fn non_holder_bulk_endpoint_queues_and_empty_search_is_rejected() {
    let base = PathBuf::from(std::env::var_os("KUNTU_TEST_DIR").expect("scratch KUNTU_TEST_DIR"));
    let home = Home::open(&base.join(format!("daemon-shared-{}", std::process::id()))).unwrap();
    let root = home.path.join("files");
    std::fs::create_dir_all(&root).unwrap();
    let roots = vec![SearchRoot::new(&root)];
    let mut holder = KuntuIndex::open_shared(home.clone(), false, "holder").unwrap();
    assert!(holder.acquire_writer().unwrap());
    let mut client = KuntuIndex::open_shared(home, false, "daemon").unwrap();
    client.register_root(&roots[0]).unwrap();
    let response = handle_request(
      &mut client,
      HttpRequest {
        method: "POST".into(),
        path: "/index/rebuild".into(),
        body: serde_json::to_vec(&json!({"roots":[root.to_string_lossy()]})).unwrap(),
      },
      &roots,
    );
    assert_eq!(response.status, 202);
    let response = handle_request(
      &mut client,
      HttpRequest {
        method: "POST".into(),
        path: "/search".into(),
        body: br#"{"roots":[],"query":""}"#.to_vec(),
      },
      &roots,
    );
    assert_eq!(response.status, 400);
  }
  #[test]
  fn strict_daemon_admission_ignores_another_consumers_union() {
    let base = PathBuf::from(std::env::var_os("KUNTU_TEST_DIR").expect("scratch directory"));
    let path = base.join(format!("daemon-policy-{}", std::process::id()));
    let home = Home::open(&path).unwrap();
    let root = home.path.join("files");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("visible-file.md"), "x").unwrap();
    std::fs::write(root.join(".hidden-file.md"), "x").unwrap();
    let allowed = SearchRoot::new(&root);
    let mut permissive = KuntuIndex::open_shared(home.clone(), false, "permissive").unwrap();
    let mut broad = allowed.clone();
    broad.include_hidden = true;
    permissive.register_root(&broad).unwrap();
    assert!(permissive.acquire_writer().unwrap());
    permissive
      .rebuild(&SearchConfig { roots: vec![broad] })
      .unwrap();
    let mut daemon = KuntuIndex::open_shared(home, false, "daemon").unwrap();
    daemon.register_root(&allowed).unwrap();
    let response = handle_request(
      &mut daemon,
      HttpRequest {
        method: "POST".into(),
        path: "/search".into(),
        body: serde_json::to_vec(
          &json!({"query":"file","roots":[root.to_string_lossy()],"include_hidden":true}),
        )
        .unwrap(),
      },
      &[allowed],
    );
    assert_eq!(response.status, 200);
    assert!(response.body.contains("visible-file"));
    assert!(!response.body.contains(".hidden-file"));
  }
}
````

<!-- apply {"task": "M2.9", "file": "Cargo.lock", "mode": "file"} -->
````toml
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "aead"
version = "0.5.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d122413f284cf2d62fb1b7db97e02edb8cda96d769b16e443a4f6195e35662b0"
dependencies = [
 "crypto-common",
 "generic-array",
]

[[package]]
name = "aegis"
version = "0.9.20"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "41585f248ed9406567ae6d61ca2a55ed3dc7348e65203a01229881253acb95e3"
dependencies = [
 "cc",
 "softaes",
]

[[package]]
name = "aes"
version = "0.8.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b169f7a6d4742236a0a00c541b845991d0ac43e546831af1249753ab4c3aa3a0"
dependencies = [
 "cfg-if",
 "cipher",
 "cpufeatures 0.2.17",
]

[[package]]
name = "aes-gcm"
version = "0.10.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "831010a0f742e1209b3bcea8fab6a8e149051ba6099432c8cb2cc117dec3ead1"
dependencies = [
 "aead",
 "aes",
 "cipher",
 "ctr",
 "ghash",
 "subtle",
]

[[package]]
name = "aho-corasick"
version = "1.1.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c982642fa9e8606056828ee9a8505737230110bb1099153c79efe865c59d12ba"
dependencies = [
 "memchr",
]

[[package]]
name = "allocator-api2"
version = "0.4.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c880a97d28a3681c0267bd29cff89621202715b065127cd445fa0f0fe0aa2880"

[[package]]
name = "android_system_properties"
version = "0.1.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ae221649c9976a6f6c56ae1facf410f3ddb33cc661c4b7b61020a912d4237fbc"
dependencies = [
 "libc",
]

[[package]]
name = "antithesis_sdk"
version = "0.2.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "08410fcac93669a476c006cd6c4512ac1e2b30fd117231a5d55d8a2c76599b82"
dependencies = [
 "libc",
 "libloading 0.8.9",
 "linkme",
 "once_cell",
 "rand 0.8.8",
 "rustc_version_runtime",
 "serde",
 "serde_json",
]

[[package]]
name = "anyhow"
version = "1.0.104"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "330a5ed07fa54e4702c9d6c4174f74427fc0ef6e214bbd677ae50a5099946470"

[[package]]
name = "arc-swap"
version = "1.9.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c049c0be4daef0b145cb3555416b3b8ef5b7888a38aea1a3a155801fe7b0810b"
dependencies = [
 "rustversion",
]

[[package]]
name = "aristo"
version = "0.4.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "fcdeefa800110050103e2459a2f8dbdbd560ad046e30f1f87e24ce19c2cb8bde"
dependencies = [
 "aristo-macros",
]

[[package]]
name = "aristo-macros"
version = "0.4.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d64a66d21a80182b35b1741997a6d2456911f54b7eb1918aa4e2382fc205268d"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "assoc"
version = "0.1.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bfdc70193dadb9d7287fa4b633f15f90c876915b31f6af17da307fc59c9859a8"

[[package]]
name = "autocfg"
version = "1.5.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f2032f911046de80f0a198e0901378627c33f59ea0ac00e363d481118bd70a53"

[[package]]
name = "base64"
version = "0.22.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "72b3254f16251a8381aa12e40e3c4d2f0199f8c6508fbecb9d91f575e0fbb8c6"

[[package]]
name = "bigdecimal"
version = "0.4.11"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8fa3f3d8cbf4dffcfe4991de61d012bef509a409ecbe9dd41049bfe32b4d4653"
dependencies = [
 "autocfg",
 "libm",
 "num-bigint",
 "num-integer",
 "num-traits",
]

[[package]]
name = "bindgen"
version = "0.69.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "271383c67ccabffb7381723dea0672a673f292304fcb45c01cc648c7a8d58088"
dependencies = [
 "bitflags",
 "cexpr",
 "clang-sys",
 "itertools 0.12.1",
 "lazy_static",
 "lazycell",
 "log",
 "prettyplease",
 "proc-macro2",
 "quote",
 "regex",
 "rustc-hash 1.1.0",
 "shlex 1.3.0",
 "syn 2.0.117",
 "which",
]

[[package]]
name = "bitflags"
version = "2.11.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c4512299f36f043ab09a583e57bceb5a5aab7a73db1805848e8fef3c9e8c78b3"

[[package]]
name = "bitvec"
version = "1.1.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ddcec3d12c579d40898fe0a9a358a803c23e9c52ca3c425707f81c9436211837"
dependencies = [
 "funty",
 "radium",
 "tap",
 "wyz",
]

[[package]]
name = "branches"
version = "0.4.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7436b21fd6195415058eb41c9090e156b556bc7e0377bb57c5c026a421521525"
dependencies = [
 "rustc_version",
]

[[package]]
name = "bstr"
version = "1.13.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "6bb31b46c14244e20ee9984b11bf5c992b91fb6939fea616e3512c8baecdbe5f"
dependencies = [
 "memchr",
 "serde_core",
]

[[package]]
name = "bumpalo"
version = "3.20.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "72f5acc6cb2ba439de613abc23857ec3d78374d8ed5ac84e9d11336e87da8649"

[[package]]
name = "bytemuck"
version = "1.25.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "95832e849adfb21180ccb6826a99da14e5d266ae5c2e668e1602cf234f153797"
dependencies = [
 "bytemuck_derive",
]

[[package]]
name = "bytemuck_derive"
version = "1.12.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "6a1f896587b6f2c069c73d2f0913e2d590c3990285cd2f0b6aa02b786b4c679c"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 3.0.5",
]

[[package]]
name = "byteorder"
version = "1.5.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1fd0f2584146f6f2ef48085050886acf353beff7305ebd1ae69500e27c67f64b"

[[package]]
name = "bytes"
version = "1.12.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "fc652a48c352aef3ea3aed32080501cf3ef6ed5da78602a020c991775b0aff04"

[[package]]
name = "cc"
version = "1.8.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "6651c9ed80effdc7db0ff72512157f901af5e3549e341e24b1dd4887d836d838"
dependencies = [
 "find-msvc-tools",
 "jobserver",
 "libc",
 "shlex 2.0.1",
]

[[package]]
name = "cexpr"
version = "0.6.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "6fac387a98bb7c37292057cffc56d62ecb629900026402633ae9160df93a8766"
dependencies = [
 "nom",
]

[[package]]
name = "cfg-if"
version = "1.0.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801"

[[package]]
name = "cfg_aliases"
version = "0.2.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f079e83a288787bcd14a6aea84cee5c87a67c5a3e660c30f557a3d24761b3527"

[[package]]
name = "cfg_block"
version = "0.1.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "18758054972164c3264f7c8386f5fc6da6114cb46b619fd365d4e3b2dc3ae487"

[[package]]
name = "chacha20"
version = "0.10.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "65c35e4b699c7e15ccbe7ee35c005e4fc0a278d22238a2857e6ce2dadeda1b06"
dependencies = [
 "cfg-if",
 "cpufeatures 0.3.1",
 "rand_core 0.10.1",
]

[[package]]
name = "chrono"
version = "0.4.45"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1aa79e62e7697b8e29b513a68abacf485adcd1fe8284a4316c5ae868e6633327"
dependencies = [
 "iana-time-zone",
 "js-sys",
 "num-traits",
 "wasm-bindgen",
 "windows-link",
]

[[package]]
name = "cipher"
version = "0.4.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "773f3b9af64447d2ce9850330c473515014aa235e6a783b02db81ff39e4a3dad"
dependencies = [
 "crypto-common",
 "inout",
]

[[package]]
name = "clang-sys"
version = "1.9.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "157a8ba7b480713b56f4c09fd13fc3e0a22a5dfab8097ba61cbc5feef950788a"
dependencies = [
 "glob",
 "libc",
 "libloading 0.8.9",
]

[[package]]
name = "concurrent-queue"
version = "2.5.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "4ca0197aee26d1ae37445ee532fefce43251d24cc7c166799f4d46817f1d3973"
dependencies = [
 "crossbeam-utils",
]

[[package]]
name = "convert_case"
version = "0.11.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "affbf0190ed2caf063e3def54ff444b449371d55c58e513a95ab98eca50adb49"
dependencies = [
 "unicode-segmentation",
]

[[package]]
name = "core-foundation-sys"
version = "0.8.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "773648b94d0e5d620f64f280777445740e61fe701025087ec8b57f45c791888b"

[[package]]
name = "cpufeatures"
version = "0.2.17"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "59ed5838eebb26a2bb2e58f6d5b5316989ae9d08bab10e0e6d103e656d1b0280"
dependencies = [
 "libc",
]

[[package]]
name = "cpufeatures"
version = "0.3.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "5ca28b0ae3115b884660db4118d803791fd6756b6e88f39c0f3f7859060d7566"
dependencies = [
 "libc",
]

[[package]]
name = "crc32c"
version = "0.6.8"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3a47af21622d091a8f0fb295b88bc886ac74efcc613efc19f5d0b21de5c89e47"
dependencies = [
 "rustc_version",
]

[[package]]
name = "crossbeam-channel"
version = "0.5.17"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "98b0cc327b5bc766e7fda9c9260cc0fa81b43a8e240440422dff70788e3f9ef1"
dependencies = [
 "crossbeam-utils",
]

[[package]]
name = "crossbeam-deque"
version = "0.8.8"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "622f3fc73690be383c7214310406f28a90e6edeadc3cea882f9d71e495b9711a"
dependencies = [
 "crossbeam-epoch",
 "crossbeam-utils",
]

[[package]]
name = "crossbeam-epoch"
version = "0.9.21"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "dc74980687109a3b14c72fd458107bf0baa1da1a1a805e178d15501ba9b86d9d"
dependencies = [
 "crossbeam-utils",
]

[[package]]
name = "crossbeam-utils"
version = "0.8.23"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "a31eee39dddec8330830986fcd7625edb5a24ec90ea038215273bbc3adb08ac6"

[[package]]
name = "crypto-common"
version = "0.1.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "78c8292055d1c1df0cce5d180393dc8cce0abec0a7102adb6c7b1eef6016d60a"
dependencies = [
 "generic-array",
 "rand_core 0.6.4",
 "typenum",
]

[[package]]
name = "ctor"
version = "1.0.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "01334b89b69ff726750c5ce5073fc8bd860e99aa9a8fc5ca11b04730e3aee97a"

[[package]]
name = "ctr"
version = "0.9.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0369ee1ad671834580515889b80f2ea915f23b8be8d0daa4bbaf2ac5c7590835"
dependencies = [
 "cipher",
]

[[package]]
name = "deranged"
version = "0.5.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "9e9de72ce2ad1f90dc62fa25f0f430ef85eb4b0d8fa0be4f30373bc40a21d28e"

[[package]]
name = "displaydoc"
version = "0.2.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c6232dd377dcc64799954cbd3a9bb882e9cdc1308ccd87b1c098f1fb2eaf82a8"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 3.0.5",
]

[[package]]
name = "either"
version = "1.18.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "252afb9ae5eaa683babdc6a068b3f5726eb19e05070c731f9b2a23a7c3e8ed34"

[[package]]
name = "env_filter"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "900d271a03799a1ee8d1ca9b19893b48ca674a9284fefcfb85f05e74ed314217"
dependencies = [
 "log",
 "regex",
]

[[package]]
name = "env_logger"
version = "0.11.11"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "de671bd27a75a797dc9ae289ba1e77276e75e2026408aab65185384e2d5cd3f6"
dependencies = [
 "env_filter",
 "log",
]

[[package]]
name = "errno"
version = "0.3.14"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "39cab71617ae0d63f51a36d69f866391735b51691dbda63cf6f96d042b63efeb"
dependencies = [
 "libc",
 "windows-sys 0.61.2",
]

[[package]]
name = "fallible-iterator"
version = "0.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2acce4a10f12dc2fb14a218589d4f1f62ef011b2d0cc4b3cb1bba8e94da14649"

[[package]]
name = "fallible-streaming-iterator"
version = "0.1.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7360491ce676a36bf9bb3c56c1aa791658183a54d2744120f27285738d90465a"

[[package]]
name = "fastbloom"
version = "0.14.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "4e7f34442dbe69c60fe8eaf58a8cafff81a1f278816d8ab4db255b3bef4ac3c4"
dependencies = [
 "getrandom 0.3.4",
 "libm",
 "siphasher",
]

[[package]]
name = "fastrand"
version = "2.5.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "da7c62ceae207dd37ea5b845da6a0696c799f85e97da1ab5b7910be3c1c80223"

[[package]]
name = "find-msvc-tools"
version = "0.1.14"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aedcfb3409746eddb02b9e19ebda1c3394f759a152e48ee875a0844d1b955484"

[[package]]
name = "foldhash"
version = "0.2.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "77ce24cb58228fbb8aa041425bb1050850ac19177686ea6e0f41a70416f56fdb"

[[package]]
name = "fs4"
version = "0.13.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8640e34b88f7652208ce9e88b1a37a2ae95227d84abec377ccd3c5cfeb141ed4"
dependencies = [
 "rustix 1.1.4",
 "windows-sys 0.59.0",
]

[[package]]
name = "funty"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e6d5a32815ae3f33302d95fdcb2ce17862f8c65363dcfd29360480ba1001fc9c"

[[package]]
name = "futures"
version = "0.3.32"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8b147ee9d1f6d097cef9ce628cd2ee62288d963e16fb287bd9286455b241382d"
dependencies = [
 "futures-channel",
 "futures-core",
 "futures-executor",
 "futures-io",
 "futures-sink",
 "futures-task",
 "futures-util",
]

[[package]]
name = "futures-channel"
version = "0.3.32"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "07bbe89c50d7a535e539b8c17bc0b49bdb77747034daa8087407d655f3f7cc1d"
dependencies = [
 "futures-core",
 "futures-sink",
]

[[package]]
name = "futures-core"
version = "0.3.32"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7e3450815272ef58cec6d564423f6e755e25379b217b0bc688e295ba24df6b1d"

[[package]]
name = "futures-executor"
version = "0.3.32"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "baf29c38818342a3b26b5b923639e7b1f4a61fc5e76102d4b1981c6dc7a7579d"
dependencies = [
 "futures-core",
 "futures-task",
 "futures-util",
]

[[package]]
name = "futures-io"
version = "0.3.32"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "cecba35d7ad927e23624b22ad55235f2239cfa44fd10428eecbeba6d6a717718"

[[package]]
name = "futures-macro"
version = "0.3.32"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e835b70203e41293343137df5c0664546da5745f82ec9b84d40be8336958447b"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "futures-sink"
version = "0.3.32"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c39754e157331b013978ec91992bde1ac089843443c49cbc7f46150b0fad0893"

[[package]]
name = "futures-task"
version = "0.3.32"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "037711b3d59c33004d3856fbdc83b99d4ff37a24768fa1be9ce3538a1cde4393"

[[package]]
name = "futures-util"
version = "0.3.32"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "389ca41296e6190b48053de0321d02a77f32f8a5d2461dd38762c0593805c6d6"
dependencies = [
 "futures-channel",
 "futures-core",
 "futures-io",
 "futures-macro",
 "futures-sink",
 "futures-task",
 "memchr",
 "pin-project-lite",
 "slab",
]

[[package]]
name = "genawaiter"
version = "0.99.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c86bd0361bcbde39b13475e6e36cb24c329964aa2611be285289d1e4b751c1a0"
dependencies = [
 "genawaiter-macro",
]

[[package]]
name = "genawaiter-macro"
version = "0.99.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0b32dfe1fdfc0bbde1f22a5da25355514b5e450c33a6af6770884c8750aedfbc"

[[package]]
name = "generator"
version = "0.8.10"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "54ade96dc9003043bce7c035c85a9df5a858bfb2039c5a2e6fdf00f324f6c551"
dependencies = [
 "cc",
 "cfg-if",
 "libc",
 "log",
 "rustversion",
 "windows-link",
 "windows-result",
]

[[package]]
name = "generic-array"
version = "0.14.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "85649ca51fd72272d7821adaf274ad91c288277713d9c18820d8499a7ff69e9a"
dependencies = [
 "typenum",
 "version_check",
]

[[package]]
name = "getrandom"
version = "0.2.17"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ff2abc00be7fca6ebc474524697ae276ad847ad0a6b3faa4bcb027e9a4614ad0"
dependencies = [
 "cfg-if",
 "libc",
 "wasi",
]

[[package]]
name = "getrandom"
version = "0.3.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "899def5c37c4fd7b2664648c28120ecec138e4d395b459e5ca34f9cce2dd77fd"
dependencies = [
 "cfg-if",
 "libc",
 "r-efi 5.3.0",
 "wasip2",
]

[[package]]
name = "getrandom"
version = "0.4.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "300e883d756b2e4ec94e02791f39b04b522276138852cfc41d9fb7e904106099"
dependencies = [
 "cfg-if",
 "libc",
 "r-efi 6.0.0",
 "rand_core 0.10.1",
]

[[package]]
name = "ghash"
version = "0.5.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f0d8a4362ccb29cb0b265253fb0a2728f592895ee6854fd9bc13f2ffda266ff1"
dependencies = [
 "opaque-debug",
 "polyval",
]

[[package]]
name = "glob"
version = "0.3.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e4eba85ea1d0a966a983acd07deee566e67395d2d96b6fb39e62b5a833f1eb0b"

[[package]]
name = "globset"
version = "0.4.20"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "07c34a9410465b45bd9787443bc7370f37735bad04b0f0cd57ff1a3186c98988"
dependencies = [
 "aho-corasick",
 "bstr",
 "log",
 "regex-automata",
 "regex-syntax",
]

[[package]]
name = "hashbrown"
version = "0.16.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "841d1cc9bed7f9236f321df977030373f4a4163ae1a7dbfe1a51a2c1a51d9100"
dependencies = [
 "foldhash",
]

[[package]]
name = "hashbrown"
version = "0.17.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ed5909b6e89a2db4456e54cd5f673791d7eca6732202bbf2a9cc504fe2f9b84a"
dependencies = [
 "foldhash",
]

[[package]]
name = "hashlink"
version = "0.12.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "a596f1b20ed2cc5ecac41a164aaebc7258057060f06c0cf7a2ba3991ee7990fb"
dependencies = [
 "hashbrown 0.17.1",
]

[[package]]
name = "heck"
version = "0.5.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2304e00983f87ffb38b55b444b5e3b60a884b5d30c0fca7d82fe33449bbe55ea"

[[package]]
name = "hermit-abi"
version = "0.5.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e17592d60ebacc7d5e169f4663c5f84f9161cc90328abcfe8456f41e4dfcb284"

[[package]]
name = "hex"
version = "0.4.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7f24254aa9a54b5c858eaee2f5bccdb46aaf0e486a595ed5fd8f86ba55232a70"

[[package]]
name = "home"
version = "0.5.12"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "cc627f471c528ff0c4a49e1d5e60450c8f6461dd6d10ba9dcd3a61d3dff7728d"
dependencies = [
 "windows-sys 0.61.2",
]

[[package]]
name = "http"
version = "1.5.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "918d3568bebf352712bc2ef3d46a8bcf1a75b373be6539de198e9105cbbf9ce0"
dependencies = [
 "bytes",
 "itoa",
]

[[package]]
name = "iana-time-zone"
version = "0.1.65"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e31bc9ad994ba00e440a8aa5c9ef0ec67d5cb5e5cb0cc7f8b744a35b389cc470"
dependencies = [
 "android_system_properties",
 "core-foundation-sys",
 "iana-time-zone-haiku",
 "js-sys",
 "log",
 "wasm-bindgen",
 "windows-core",
]

[[package]]
name = "iana-time-zone-haiku"
version = "0.1.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f31827a206f56af32e590ba56d5d2d085f558508192593743f16b2306495269f"
dependencies = [
 "cc",
]

[[package]]
name = "icu_collator"
version = "2.3.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "08984ed58ac439ebf3e13d2cf26b0c46a60afcd21721c1d14087c0b240344dda"
dependencies = [
 "icu_collator_data",
 "icu_collections",
 "icu_locale_core",
 "icu_locale_fallback",
 "icu_normalizer",
 "icu_properties",
 "icu_provider",
 "smallvec",
 "utf16_iter",
 "utf8_iter",
 "zerovec",
]

[[package]]
name = "icu_collator_data"
version = "2.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f7d7e54efdddeb1208c08dd5d32b53a879ac00d2d3d051b2255fd26821d16368"

[[package]]
name = "icu_collections"
version = "2.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "fa68d21081c4a05d5a901a1c62add574c77048b6a1c67be3b50ce0b60d4ca513"
dependencies = [
 "displaydoc",
 "potential_utf",
 "utf8_iter",
 "yoke",
 "zerofrom",
 "zerovec",
]

[[package]]
name = "icu_locale"
version = "2.3.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "785f595c61ef57169a467eeed7b4b6936a66f6cacbb116a96ee86da983251bf4"
dependencies = [
 "icu_collections",
 "icu_locale_core",
 "icu_locale_data",
 "icu_locale_fallback",
 "icu_provider",
 "tinystr",
 "zerovec",
]

[[package]]
name = "icu_locale_core"
version = "2.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d56e28588da92eee5c3201a6eff33fabdd49b62269c8938d4ff050ce4d900deb"
dependencies = [
 "displaydoc",
 "litemap",
 "serde",
 "tinystr",
 "writeable",
 "zerovec",
]

[[package]]
name = "icu_locale_data"
version = "2.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d37d91460e362a5cf58907cd7ce871411775ee6294e49a5cc8e6cec16dfa75ea"

[[package]]
name = "icu_locale_fallback"
version = "2.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "251af8e57c9400e3eb58242fe5b8b1152b2a64fdf4cf632f923c38ccee6f2fa9"
dependencies = [
 "icu_locale_core",
 "icu_locale_fallback_data",
 "icu_provider",
 "potential_utf",
 "tinystr",
 "zerovec",
]

[[package]]
name = "icu_locale_fallback_data"
version = "2.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "decf2a22ec8fa68f1a0c1129a3f8583f8f8bc24e8b9ccbe98ead99f62a4dc3a8"

[[package]]
name = "icu_normalizer"
version = "2.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "12f9cf5f235641ed274641dd81c3f28d870e276763d0797aeeab72317b1c646f"
dependencies = [
 "icu_collections",
 "icu_normalizer_data",
 "icu_properties",
 "icu_provider",
 "smallvec",
 "utf16_iter",
 "utf8_iter",
 "write16",
 "zerovec",
]

[[package]]
name = "icu_normalizer_data"
version = "2.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1563da1ed3e0b3bf3d74c9b85917ac9c56464d2f57242270c09c9e752f8021a0"

[[package]]
name = "icu_properties"
version = "2.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7e7ca276ad3145661a65914e6daf131ca5120cd3dcee8f8f3214b8875184a148"
dependencies = [
 "displaydoc",
 "icu_collections",
 "icu_locale_core",
 "icu_properties_data",
 "icu_provider",
 "zerotrie",
 "zerovec",
]

[[package]]
name = "icu_properties_data"
version = "2.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e590f038c1464a96894fd6d10127e90a8be4509f56ff7ecef851b15cee0b7caa"

[[package]]
name = "icu_provider"
version = "2.3.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d27bbb9d3abbefac45d55f647c9de1d44aafcd1186eb91879afef17c396c3e73"
dependencies = [
 "displaydoc",
 "icu_locale_core",
 "serde",
 "stable_deref_trait",
 "writeable",
 "yoke",
 "zerofrom",
 "zerotrie",
 "zerovec",
]

[[package]]
name = "ignore"
version = "0.4.33"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "00b69833ed729dc5aa7d19541d96d6cf8e9137194207a04916d658e43168402f"
dependencies = [
 "crossbeam-deque",
 "globset",
 "log",
 "memchr",
 "regex-automata",
 "same-file",
 "walkdir",
 "winapi-util",
]

[[package]]
name = "inout"
version = "0.1.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "879f10e63c20629ecabbb64a8010319738c66a5cd0c29b02d63d272b03751d01"
dependencies = [
 "generic-array",
]

[[package]]
name = "intrusive-collections"
version = "0.9.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "189d0897e4cbe8c75efedf3502c18c887b05046e59d28404d4d8e46cbc4d1e86"
dependencies = [
 "memoffset",
]

[[package]]
name = "io-uring"
version = "0.7.15"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ed3bd0ecfbb87805f538bb7b32e5239ca0763890c623e349860ecba69469f2bb"
dependencies = [
 "bitflags",
 "cfg-if",
 "libc",
]

[[package]]
name = "itertools"
version = "0.12.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ba291022dbbd398a455acf126c1e341954079855bc60dfdda641363bd6922569"
dependencies = [
 "either",
]

[[package]]
name = "itertools"
version = "0.14.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2b192c782037fadd9cfa75548310488aabdbf3d2da73885b31bd0abd03351285"
dependencies = [
 "either",
]

[[package]]
name = "itoa"
version = "1.0.18"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8f42a60cbdf9a97f5d2305f08a87dc4e09308d1276d28c869c684d7777685682"

[[package]]
name = "jobserver"
version = "0.1.35"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1c00acbd29eabad4a2392fa0e921c874934dbbf4194312ad20f04a0ed67a3cb3"
dependencies = [
 "getrandom 0.4.3",
 "libc",
]

[[package]]
name = "js-sys"
version = "0.3.105"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ce57d20d1ea864ce2ac172ab472d409214f4fd359f0b2a2775abdf522e2af99e"
dependencies = [
 "cfg-if",
 "futures-util",
 "wasm-bindgen",
]

[[package]]
name = "kuntu-cli"
version = "0.1.0"
dependencies = [
 "kuntu-core",
 "kuntu-daemon",
 "kuntu-index",
 "kuntu-provider-spotlight",
 "kuntu-store",
 "kuntu-watcher",
]

[[package]]
name = "kuntu-core"
version = "0.1.0"

[[package]]
name = "kuntu-crawler"
version = "0.1.0"
dependencies = [
 "kuntu-core",
]

[[package]]
name = "kuntu-daemon"
version = "0.1.0"
dependencies = [
 "kuntu-core",
 "kuntu-index",
 "kuntu-store",
 "serde",
 "serde_json",
]

[[package]]
name = "kuntu-index"
version = "0.1.0"
dependencies = [
 "futures",
 "kuntu-core",
 "kuntu-crawler",
 "kuntu-legacy-index",
 "kuntu-store",
 "kuntu-vfs",
 "nix",
 "rusqlite",
 "serde_json",
 "turso",
 "zstd",
]

[[package]]
name = "kuntu-legacy-index"
version = "0.1.0"
dependencies = [
 "futures",
 "kuntu-core",
 "kuntu-crawler",
 "turso",
]

[[package]]
name = "kuntu-napi"
version = "0.1.0"
dependencies = [
 "kuntu-core",
 "kuntu-index",
 "kuntu-legacy-index",
 "kuntu-store",
 "napi",
 "napi-build",
 "napi-derive",
 "rusqlite",
]

[[package]]
name = "kuntu-provider-spotlight"
version = "0.1.0"
dependencies = [
 "kuntu-core",
]

[[package]]
name = "kuntu-scan"
version = "0.3.0"
dependencies = [
 "ignore",
 "libc",
 "objc2",
 "objc2-foundation",
 "rayon",
 "serde",
 "serde_json",
 "windows-sys 0.61.2",
]

[[package]]
name = "kuntu-store"
version = "0.1.0"
dependencies = [
 "fs4",
 "getrandom 0.3.4",
 "nix",
 "serde",
 "serde_json",
]

[[package]]
name = "kuntu-vfs"
version = "0.1.0"
dependencies = [
 "cc",
 "rusqlite",
]

[[package]]
name = "kuntu-watcher"
version = "0.1.0"
dependencies = [
 "kuntu-core",
 "kuntu-crawler",
]

[[package]]
name = "lazy_static"
version = "1.5.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "20870f649af7073d53e38067b2a84312175d56ea15217e1b15bc83506ec50afb"

[[package]]
name = "lazycell"
version = "1.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "830d08ce1d1d941e6b30645f1a0eb5643013d835ce3779a5fc208261dbe10f55"

[[package]]
name = "libc"
version = "0.2.189"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2"

[[package]]
name = "libloading"
version = "0.8.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d7c4b02199fee7c5d21a5ae7d8cfa79a6ef5bb2fc834d6e9058e89c825efdc55"
dependencies = [
 "cfg-if",
 "windows-link",
]

[[package]]
name = "libloading"
version = "0.9.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "754ca22de805bb5744484a5b151a9e1a8e837d5dc232c2d7d8c2e3492edc8b60"
dependencies = [
 "cfg-if",
 "windows-link",
]

[[package]]
name = "libm"
version = "0.2.16"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b6d2cec3eae94f9f509c767b45932f1ada8350c4bdb85af2fcab4a3c14807981"

[[package]]
name = "libsqlite3-sys"
version = "0.38.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f1d20bef17f513b9b3004532233187769cd072d790971f4e4da0e346eb6401e8"
dependencies = [
 "cc",
 "pkg-config",
 "vcpkg",
]

[[package]]
name = "linkme"
version = "0.3.37"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3045e122bd98aef8ec3ad58ce84f0791f64e70163d1a02710af4aa11a4d54cc5"
dependencies = [
 "linkme-impl",
]

[[package]]
name = "linkme-impl"
version = "0.3.37"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "77060ebe535362c3da75682cd17b0431017b6e7c5661e714fc69a7ad017d1301"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 3.0.5",
]

[[package]]
name = "linux-raw-sys"
version = "0.4.15"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d26c52dbd32dccf2d10cac7725f8eae5296885fb5703b261f7d0a0739ec807ab"

[[package]]
name = "linux-raw-sys"
version = "0.12.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "32a66949e030da00e8c7d4434b251670a91556f4144941d37452769c25d58a53"

[[package]]
name = "litemap"
version = "0.8.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "47d9d19d1d6efa0109d2f65ff4c85cddd50bd572e5a00127ab10987290bcefae"

[[package]]
name = "lock_api"
version = "0.4.14"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "224399e74b87b5f3557511d98dff8b14089b3dadafcab6bb93eab67d3aace965"
dependencies = [
 "scopeguard",
]

[[package]]
name = "log"
version = "0.4.34"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f9f8bd3e56ce4dfc153cf470fffbfa98c7620958b312ca5c3a4b8d5181fd13c6"

[[package]]
name = "loom"
version = "0.7.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "419e0dc8046cb947daa77eb95ae174acfbddb7673b4151f56d1eed8e93fbfaca"
dependencies = [
 "cfg-if",
 "generator",
 "scoped-tls",
 "tracing",
 "tracing-subscriber",
]

[[package]]
name = "matchers"
version = "0.2.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d1525a2a28c7f4fa0fc98bb91ae755d1e2d1505079e05539e35bc876b5d65ae9"
dependencies = [
 "regex-automata",
]

[[package]]
name = "memchr"
version = "2.8.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "6b947ae49db0d222b1dbc6b113ce7248a3fc3a6ca21b696717bfc000ba4484d8"

[[package]]
name = "memoffset"
version = "0.9.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "488016bfae457b036d996092f6cb448677611ce4449e970ceaf42695203f218a"
dependencies = [
 "autocfg",
]

[[package]]
name = "miette"
version = "7.6.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "5f98efec8807c63c752b5bd61f862c165c115b0a35685bdcfd9238c7aeb592b7"
dependencies = [
 "cfg-if",
 "miette-derive",
 "unicode-width",
]

[[package]]
name = "miette-derive"
version = "7.6.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "db5b29714e950dbb20d5e6f74f9dcec4edbcc1067bb7f8ed198c097b8c1a818b"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "minimal-lexical"
version = "0.2.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "68354c5c6bd36d73ff3feceb05efa59b6acb7626617f4962be322a825e61f79a"

[[package]]
name = "napi"
version = "3.9.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f1d395473824516f38dd1071a1a37bc57daa7be65b293ebba4ead5f7abb017a2"
dependencies = [
 "bitflags",
 "ctor",
 "futures",
 "napi-build",
 "napi-sys",
 "nohash-hasher",
 "rustc-hash 2.1.2",
]

[[package]]
name = "napi-build"
version = "2.3.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c9c366d2c8c60b86fa632df75f745509b52f9128f91a6bad4c796e44abb505e1"

[[package]]
name = "napi-derive"
version = "3.5.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "89b3f766e04667e6da0e181e2da4f85475d5a6513b7cf6a80bea184e224a5b42"
dependencies = [
 "convert_case",
 "ctor",
 "napi-derive-backend",
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "napi-derive-backend"
version = "5.0.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0d5af30503edf933ce7377cf6d4c877a62b0f1107ea05585f1b5e430e88d5baf"
dependencies = [
 "convert_case",
 "proc-macro2",
 "quote",
 "semver",
 "syn 2.0.117",
]

[[package]]
name = "napi-sys"
version = "3.2.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8eb602b84d7c1edae45e50bbf1374696548f36ae179dfa667f577e384bb90c2b"
dependencies = [
 "libloading 0.9.0",
]

[[package]]
name = "nix"
version = "0.30.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "74523f3a35e05aba87a1d978330aef40f67b0304ac79c1c00b294c9830543db6"
dependencies = [
 "bitflags",
 "cfg-if",
 "cfg_aliases",
 "libc",
]

[[package]]
name = "nohash-hasher"
version = "0.2.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2bf50223579dc7cdcfb3bfcacf7069ff68243f8c363f62ffa99cf000a6b9c451"

[[package]]
name = "nom"
version = "7.1.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d273983c5a657a70a3e8f2a01329822f3b8c8172b73826411a55751e404a0a4a"
dependencies = [
 "memchr",
 "minimal-lexical",
]

[[package]]
name = "nu-ansi-term"
version = "0.50.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7957b9740744892f114936ab4a57b3f487491bbeafaf8083688b16841a4240e5"
dependencies = [
 "windows-sys 0.61.2",
]

[[package]]
name = "num-bigint"
version = "0.4.8"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c89e69e7e0f03bea5ef08013795c25018e101932225a656383bd384495ecc367"
dependencies = [
 "num-integer",
 "num-traits",
]

[[package]]
name = "num-conv"
version = "0.2.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "521739c6d2bac4aa25192232afe6841231376b2b26d4d9fae5ecf8ca5772e441"

[[package]]
name = "num-integer"
version = "0.1.47"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7ce2d95d4b3734dc35aa2f45e1aa22cd416814592a4f9d9205e11affd5b8e10b"
dependencies = [
 "num-traits",
]

[[package]]
name = "num-traits"
version = "0.2.19"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "071dfc062690e90b734c0b2273ce72ad0ffa95f0c74596bc250dcfd960262841"
dependencies = [
 "autocfg",
]

[[package]]
name = "objc2"
version = "0.6.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3a12a8ed07aefc768292f076dc3ac8c48f3781c8f2d5851dd3d98950e8c5a89f"
dependencies = [
 "objc2-encode",
]

[[package]]
name = "objc2-encode"
version = "4.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ef25abbcd74fb2609453eb695bd2f860d389e457f67dc17cafc8b8cbc89d0c33"

[[package]]
name = "objc2-foundation"
version = "0.3.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e3e0adef53c21f888deb4fa59fc59f7eb17404926ee8a6f59f5df0fd7f9f3272"
dependencies = [
 "bitflags",
 "objc2",
]

[[package]]
name = "once_cell"
version = "1.21.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "9f7c3e4beb33f85d45ae3e3a1792185706c8e16d043238c593331cc7cd313b50"

[[package]]
name = "opaque-debug"
version = "0.3.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c08d65885ee38876c4f86fa503fb49d7b507c2b62552df7c70b2fce627e06381"

[[package]]
name = "owo-colors"
version = "3.5.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c1b04fb49957986fdce4d6ee7a65027d55d4b6d2265e5848bbb507b58ccfdb6f"

[[package]]
name = "pack1"
version = "1.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e3b7bb0ecf2e447b1f20ee94ee79ef6eed1e9d4b3c36ce1903b9dea3bf205523"
dependencies = [
 "bytemuck",
]

[[package]]
name = "parking_lot"
version = "0.12.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "93857453250e3077bd71ff98b6a65ea6621a19bb0f559a85248955ac12c45a1a"
dependencies = [
 "lock_api",
 "parking_lot_core",
]

[[package]]
name = "parking_lot_core"
version = "0.9.12"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2621685985a2ebf1c516881c026032ac7deafcda1a2c9b7850dc81e3dfcb64c1"
dependencies = [
 "cfg-if",
 "libc",
 "redox_syscall",
 "smallvec",
 "windows-link",
]

[[package]]
name = "pastey"
version = "0.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2ee67f1008b1ba2321834326597b8e186293b049a023cdef258527550b9935b4"

[[package]]
name = "pin-project-lite"
version = "0.2.17"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "a89322df9ebe1c1578d689c92318e070967d1042b512afbe49518723f4e6d5cd"

[[package]]
name = "pkg-config"
version = "0.3.34"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f6b464fbc74e149a392436b17d523f769e057cb6877f6a5c4618bc6f11800548"

[[package]]
name = "polling"
version = "3.11.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "5d0e4f59085d47d8241c88ead0f274e8a0cb551f3625263c05eb8dd897c34218"
dependencies = [
 "cfg-if",
 "concurrent-queue",
 "hermit-abi",
 "pin-project-lite",
 "rustix 1.1.4",
 "windows-sys 0.61.2",
]

[[package]]
name = "polyval"
version = "0.6.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "9d1fe60d06143b2430aa532c94cfe9e29783047f06c0d7fd359a9a51b729fa25"
dependencies = [
 "cfg-if",
 "cpufeatures 0.2.17",
 "opaque-debug",
 "universal-hash",
]

[[package]]
name = "potential_utf"
version = "0.1.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d83eb9bc6d8e5cf568e7a1101d60ee05e81ed50ea106026f3d18deeb046d7661"
dependencies = [
 "serde_core",
 "writeable",
 "zerovec",
]

[[package]]
name = "powerfmt"
version = "0.2.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "4a6394b9e965e73d0a289ee54f589087e2c676aedf60885baf52c76b771e4958"

[[package]]
name = "ppv-lite86"
version = "0.2.21"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "85eae3c4ed2f50dcfe72643da4befc30deadb458a9b590d720cde2f2b1e97da9"
dependencies = [
 "zerocopy",
]

[[package]]
name = "prettyplease"
version = "0.2.37"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "479ca8adacdd7ce8f1fb39ce9ecccbfe93a3f1344b3d0d97f20bc0196208f62b"
dependencies = [
 "proc-macro2",
 "syn 2.0.117",
]

[[package]]
name = "proc-macro2"
version = "1.0.106"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8fd00f0bb2e90d81d1044c2b32617f68fcb9fa3bb7640c23e9c748e53fb30934"
dependencies = [
 "unicode-ident",
]

[[package]]
name = "prost"
version = "0.14.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "528ac67416ff8646872a3c02cad9cc4ee5dc9f9540c9b10771855c95cb2e5ae1"
dependencies = [
 "bytes",
 "prost-derive",
]

[[package]]
name = "prost-derive"
version = "0.14.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b570b25f7617e43d59005d0990ccb79e950a423952cea19671b7a876da390adf"
dependencies = [
 "anyhow",
 "itertools 0.14.0",
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "quote"
version = "1.0.45"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "41f2619966050689382d2b44f664f4bc593e129785a36d6ee376ddf37259b924"
dependencies = [
 "proc-macro2",
]

[[package]]
name = "r-efi"
version = "5.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "69cdb34c158ceb288df11e18b4bd39de994f6657d83847bdffdbd7f346754b0f"

[[package]]
name = "r-efi"
version = "6.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f8dcc9c7d52a811697d2151c701e0d08956f92b0e24136cf4cf27b57a6a0d9bf"

[[package]]
name = "radium"
version = "0.7.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "dc33ff2d4973d518d823d61aa239014831e521c75da58e3df4840d3f47749d09"

[[package]]
name = "rand"
version = "0.8.8"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e058c7de0b26af77780c769414d6257830bb240f3c38477dbc2c16e5f54d6d4c"
dependencies = [
 "libc",
 "rand_chacha 0.3.1",
 "rand_core 0.6.4",
]

[[package]]
name = "rand"
version = "0.9.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b9ef1d0d795eb7d84685bca4f72f3649f064e6641543d3a8c415898726a57b41"
dependencies = [
 "rand_chacha 0.9.0",
 "rand_core 0.9.5",
]

[[package]]
name = "rand"
version = "0.10.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "65c9fb96cbc91e3478eaae79a69fcd3f1ae4ad052e471fe6732fff548984b4af"
dependencies = [
 "chacha20",
 "getrandom 0.4.3",
 "rand_core 0.10.1",
]

[[package]]
name = "rand_chacha"
version = "0.3.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e6c10a63a0fa32252be49d21e7709d4d4baf8d231c2dbce1eaa8141b9b127d88"
dependencies = [
 "ppv-lite86",
 "rand_core 0.6.4",
]

[[package]]
name = "rand_chacha"
version = "0.9.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d3022b5f1df60f26e1ffddd6c66e8aa15de382ae63b3a0c1bfc0e4d3e3f325cb"
dependencies = [
 "ppv-lite86",
 "rand_core 0.9.5",
]

[[package]]
name = "rand_core"
version = "0.6.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ec0be4795e2f6a28069bec0b5ff3e2ac9bafc99e6a9a7dc3547996c5c816922c"
dependencies = [
 "getrandom 0.2.17",
]

[[package]]
name = "rand_core"
version = "0.9.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "76afc826de14238e6e8c374ddcc1fa19e374fd8dd986b0d2af0d02377261d83c"
dependencies = [
 "getrandom 0.3.4",
]

[[package]]
name = "rand_core"
version = "0.10.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "63b8176103e19a2643978565ca18b50549f6101881c443590420e4dc998a3c69"

[[package]]
name = "rand_pcg"
version = "0.3.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "59cad018caf63deb318e5a4586d99a24424a364f40f1e5778c29aca23f4fc73e"
dependencies = [
 "rand_core 0.6.4",
]

[[package]]
name = "rapidhash"
version = "4.5.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "5da7e78a036ce858e8d55b7e7dc8ba3a88b78350fd2155d3591bbd966b58589e"
dependencies = [
 "rustversion",
]

[[package]]
name = "rayon"
version = "1.12.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "fb39b166781f92d482534ef4b4b1b2568f42613b53e5b6c160e24cfbfa30926d"
dependencies = [
 "either",
 "rayon-core",
]

[[package]]
name = "rayon-core"
version = "1.13.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "22e18b0f0062d30d4230b2e85ff77fdfe4326feb054b9783a3460d8435c8ab91"
dependencies = [
 "crossbeam-deque",
 "crossbeam-utils",
]

[[package]]
name = "redox_syscall"
version = "0.5.18"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ed2bf2547551a7053d6fdfafda3f938979645c44812fbfcda098faae3f1a362d"
dependencies = [
 "bitflags",
]

[[package]]
name = "regex"
version = "1.13.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f020237b6c8eed93db2e2cb53c00c60a8e1bc73da7d073199a1180401450218d"
dependencies = [
 "aho-corasick",
 "memchr",
 "regex-automata",
 "regex-syntax",
]

[[package]]
name = "regex-automata"
version = "0.4.18"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ad8553b9b26413251cbf30e620595c7a41b3887f03da04579c0e6b0d6a06b4b2"
dependencies = [
 "aho-corasick",
 "memchr",
 "regex-syntax",
]

[[package]]
name = "regex-syntax"
version = "0.8.11"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d6f6ff9a378485b298a5286656da665ba74413d36db0979633275d2e708145d4"

[[package]]
name = "roaring"
version = "0.11.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "18bd8a37d17a58532776dcdf6041ce64929adca78e8489d5cacbafe99229d3e1"
dependencies = [
 "bytemuck",
 "byteorder",
]

[[package]]
name = "rsqlite-vfs"
version = "0.1.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c51c9ae4df8a7fba42103df5c621fa3c37eccf3a3c650879e90fc48b11cc192c"
dependencies = [
 "hashbrown 0.16.1",
 "thiserror",
]

[[package]]
name = "rusqlite"
version = "0.40.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "23f2a97da3e3873c73cb2a2e71b35c40ff95e0b1eefa8d72d8499a6928c3b5b3"
dependencies = [
 "bitflags",
 "fallible-iterator",
 "fallible-streaming-iterator",
 "hashlink",
 "libsqlite3-sys",
 "smallvec",
 "sqlite-wasm-rs",
]

[[package]]
name = "rustc-hash"
version = "1.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "08d43f7aa6b08d49f382cde6a7982047c3426db949b1424bc4b7ec9ae12c6ce2"

[[package]]
name = "rustc-hash"
version = "2.1.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "94300abf3f1ae2e2b8ffb7b58043de3d399c73fa6f4b73826402a5c457614dbe"

[[package]]
name = "rustc_version"
version = "0.4.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "cfcb3a22ef46e85b45de6ee7e79d063319ebb6594faafcf1c225ea92ab6e9b92"
dependencies = [
 "semver",
]

[[package]]
name = "rustc_version_runtime"
version = "0.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2dd18cd2bae1820af0b6ad5e54f4a51d0f3fcc53b05f845675074efcc7af071d"
dependencies = [
 "rustc_version",
 "semver",
]

[[package]]
name = "rustix"
version = "0.38.44"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "fdb5bc1ae2baa591800df16c9ca78619bf65c0488b41b96ccec5d11220d8c154"
dependencies = [
 "bitflags",
 "errno",
 "libc",
 "linux-raw-sys 0.4.15",
 "windows-sys 0.59.0",
]

[[package]]
name = "rustix"
version = "1.1.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b6fe4565b9518b83ef4f91bb47ce29620ca828bd32cb7e408f0062e9930ba190"
dependencies = [
 "bitflags",
 "errno",
 "libc",
 "linux-raw-sys 0.12.1",
 "windows-sys 0.61.2",
]

[[package]]
name = "rustversion"
version = "1.0.23"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "cf54715a573b99ac80df0bc206da022bcd442c974952c7b9720069370852e21f"

[[package]]
name = "ryu"
version = "1.0.23"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "9774ba4a74de5f7b1c1451ed6cd5285a32eddb5cccb8cc655a4e50009e06477f"

[[package]]
name = "same-file"
version = "1.0.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "93fc1dc3aaa9bfed95e02e6eadabb4baf7e3078b0bd1b4d7b6b0b68378900502"
dependencies = [
 "winapi-util",
]

[[package]]
name = "scoped-tls"
version = "1.0.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e1cf6437eb19a8f4a6cc0f7dca544973b0b78843adbfeb3683d1a94a0024a294"

[[package]]
name = "scopeguard"
version = "1.2.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "94143f37725109f92c262ed2cf5e59bce7498c01bcc1502d7b9afe439a4e9f49"

[[package]]
name = "semver"
version = "1.0.28"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8a7852d02fc848982e0c167ef163aaff9cd91dc640ba85e263cb1ce46fae51cd"

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "9a8e94ea7f378bd32cbbd37198a4a91436180c5bb472411e48b5ec2e2124ae9e"
dependencies = [
 "serde_core",
 "serde_derive",
]

[[package]]
name = "serde_core"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "41d385c7d4ca58e59fc732af25c3983b67ac852c1a25000afe1175de458b67ad"
dependencies = [
 "serde_derive",
]

[[package]]
name = "serde_derive"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d540f220d3187173da220f885ab66608367b6574e925011a9353e4badda91d79"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "serde_json"
version = "1.0.150"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e8014e44b4736ed0538adeecded0fce2a272f22dc9578a7eb6b2d9993c74cfb9"
dependencies = [
 "itoa",
 "memchr",
 "serde",
 "serde_core",
 "zmij",
]

[[package]]
name = "sha1_smol"
version = "1.0.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bbfa15b3dddfee50a0fff136974b3e1bde555604ba463834a7eb7deb6417705d"

[[package]]
name = "sharded-slab"
version = "0.1.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f40ca3c46823713e0d4209592e8d6e826aa57e928f09752619fc696c499637f6"
dependencies = [
 "lazy_static",
]

[[package]]
name = "shlex"
version = "1.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0fda2ff0d084019ba4d7c6f371c95d8fd75ce3524c3cb8fb653a3023f6323e64"

[[package]]
name = "shlex"
version = "2.0.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f8fadd59c855ef2080decdef8ff161eb6661b86933c9d82e5ba29dc602a55aba"

[[package]]
name = "shuttle"
version = "0.8.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2ab17edba38d63047f46780cf7360acf7467fec2c048928689a5c1dd1c2b4e31"
dependencies = [
 "assoc",
 "bitvec",
 "cfg-if",
 "generator",
 "hex",
 "owo-colors",
 "rand 0.8.8",
 "rand_core 0.6.4",
 "rand_pcg",
 "scoped-tls",
 "smallvec",
 "tracing",
]

[[package]]
name = "simsimd"
version = "6.5.16"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f4fb3bc3cdce07a7d7d4caa4c54f8aa967f6be41690482b54b24100a2253fa70"
dependencies = [
 "cc",
]

[[package]]
name = "siphasher"
version = "1.0.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "33f4fe9184a62d842c9ef383018f3306d8ba224fd9d836f56d7288308847c256"

[[package]]
name = "slab"
version = "0.4.12"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0c790de23124f9ab44544d7ac05d60440adc586479ce501c1d6d7da3cd8c9cf5"

[[package]]
name = "smallvec"
version = "1.15.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "67b1b7a3b5fe4f1376887184045fcf45c69e92af734b7aaddc05fb777b6fbd03"

[[package]]
name = "softaes"
version = "0.1.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c4fdfe13e1c42e45f497416afb8dc4e783bf0e25b7be9077c1c77c5fb14cb998"

[[package]]
name = "sqlite-wasm-rs"
version = "0.5.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "dc3efc0da82635d7e1ced0053bbbfa8c7ab9645d0bf36ceb4f7127bb85315d75"
dependencies = [
 "cc",
 "js-sys",
 "rsqlite-vfs",
 "wasm-bindgen",
]

[[package]]
name = "stable_deref_trait"
version = "1.2.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "6ce2be8dc25455e1f91df71bfa12ad37d7af1092ae736f3a6cd0e37bc7810596"

[[package]]
name = "strum"
version = "0.26.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8fec0f0aef304996cf250b31b5a10dee7980c85da9d759361292b8bca5a18f06"
dependencies = [
 "strum_macros",
]

[[package]]
name = "strum_macros"
version = "0.26.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "4c6bee85a5a24955dc440386795aa378cd9cf82acd5f764469152d2270e581be"
dependencies = [
 "heck",
 "proc-macro2",
 "quote",
 "rustversion",
 "syn 2.0.117",
]

[[package]]
name = "subtle"
version = "2.6.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "13c2bddecc57b384dee18652358fb23172facb8a2c51ccc10d74c157bdea3292"

[[package]]
name = "symlink"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "a7973cce6668464ea31f176d85b13c7ab3bba2cb3b77a2ed26abd7801688010a"

[[package]]
name = "syn"
version = "2.0.117"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e665b8803e7b1d2a727f4023456bbbbe74da67099c585258af0ad9c5013b9b99"
dependencies = [
 "proc-macro2",
 "quote",
 "unicode-ident",
]

[[package]]
name = "syn"
version = "3.0.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "12df2e0110f65b775f769bb17ef989067a1d931b2eb822bd4346631eeada89f9"
dependencies = [
 "proc-macro2",
 "quote",
 "unicode-ident",
]

[[package]]
name = "synstructure"
version = "0.14.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "901704edd0dfe137f1987838ee4f259e4e063c31371bdb423f7ae38ec6f77f02"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 3.0.5",
]

[[package]]
name = "tap"
version = "1.0.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "55937e1799185b12863d447f42597ed69d9928686b8d88a1df17376a097d8369"

[[package]]
name = "tempfile"
version = "3.27.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "32497e9a4c7b38532efcdebeef879707aa9f794296a4f0244f6f69e9bc8574bd"
dependencies = [
 "fastrand",
 "getrandom 0.4.3",
 "once_cell",
 "rustix 1.1.4",
 "windows-sys 0.61.2",
]

[[package]]
name = "thiserror"
version = "2.0.20"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ec86235f5fcc2a73650310756d2ac5b138a5780bbbdfae3eeccec992c435ba4f"
dependencies = [
 "thiserror-impl",
]

[[package]]
name = "thiserror-impl"
version = "2.0.20"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bc04cd3e1236dd4a98afca4569f2deb3f120e5422a4023be2cb683f8486292af"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 3.0.5",
]

[[package]]
name = "thread_local"
version = "1.1.10"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1ad99c4c6d32803332c548b1af0540b357b3f5fc0be8f6c6bfe8b2e6ae784070"
dependencies = [
 "cfg-if",
]

[[package]]
name = "time"
version = "0.3.55"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "cdb87b95ec50ddfa440816d227a17b2ccbdda963a316a727fda0fc4334f7d134"
dependencies = [
 "deranged",
 "num-conv",
 "powerfmt",
 "serde_core",
 "time-core",
 "time-macros",
]

[[package]]
name = "time-core"
version = "0.1.9"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "9e1c906769ad99c88eaa54e728060edef082f8e358ff32030cb7c7d315e81109"

[[package]]
name = "time-macros"
version = "0.2.32"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7e689342a48d2ea927c87ea50cabf8594854bf940e9310208848d680d668ed85"
dependencies = [
 "num-conv",
 "time-core",
]

[[package]]
name = "tinystr"
version = "0.8.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b1e27c91459209c2986af3dcf603a5a74a4368754ce37414f59acc971167f643"
dependencies = [
 "displaydoc",
 "serde_core",
 "zerovec",
]

[[package]]
name = "tracing"
version = "0.1.44"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "63e71662fa4b2a2c3a26f570f037eb95bb1f85397f3cd8076caed2f026a6d100"
dependencies = [
 "pin-project-lite",
 "tracing-attributes",
 "tracing-core",
]

[[package]]
name = "tracing-appender"
version = "0.2.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "050686193eb999b4bb3bc2acfa891a13da00f79734704c4b8b4ef1a10b368a3c"
dependencies = [
 "crossbeam-channel",
 "symlink",
 "thiserror",
 "time",
 "tracing-subscriber",
]

[[package]]
name = "tracing-attributes"
version = "0.1.31"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7490cfa5ec963746568740651ac6781f701c9c5ea257c58e057f3ba8cf69e8da"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "tracing-core"
version = "0.1.36"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "db97caf9d906fbde555dd62fa95ddba9eecfd14cb388e4f491a66d74cd5fb79a"
dependencies = [
 "once_cell",
 "valuable",
]

[[package]]
name = "tracing-log"
version = "0.2.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ee855f1f400bd0e5c02d150ae5de3840039a3f54b025156404e34c23c03f47c3"
dependencies = [
 "log",
 "once_cell",
 "tracing-core",
]

[[package]]
name = "tracing-subscriber"
version = "0.3.23"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "cb7f578e5945fb242538965c2d0b04418d38ec25c79d160cd279bf0731c8d319"
dependencies = [
 "matchers",
 "nu-ansi-term",
 "once_cell",
 "regex-automata",
 "sharded-slab",
 "smallvec",
 "thread_local",
 "tracing",
 "tracing-core",
 "tracing-log",
]

[[package]]
name = "turso"
version = "0.7.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f9491d7a80312c5abe66a4409e4dce02065503a235453c94b9e4133877e39ffc"
dependencies = [
 "thiserror",
 "tracing",
 "tracing-subscriber",
 "turso_core",
 "turso_sdk_kit",
 "turso_sync_sdk_kit",
]

[[package]]
name = "turso_core"
version = "0.7.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7a833cc3bf8d4e6c101c504fa470f8ab4270c2202ff2591b61b2e373b4f20d9b"
dependencies = [
 "aegis",
 "aes",
 "aes-gcm",
 "allocator-api2",
 "antithesis_sdk",
 "arc-swap",
 "aristo",
 "bigdecimal",
 "bitflags",
 "branches",
 "bumpalo",
 "bytemuck",
 "cfg_aliases",
 "cfg_block",
 "chrono",
 "crc32c",
 "crossbeam-epoch",
 "crossbeam-utils",
 "either",
 "fallible-iterator",
 "fastbloom",
 "hex",
 "icu_collator",
 "icu_locale",
 "intrusive-collections",
 "io-uring",
 "libc",
 "libloading 0.8.9",
 "libm",
 "loom",
 "miette",
 "num-bigint",
 "num-traits",
 "pack1",
 "parking_lot",
 "pastey",
 "polling",
 "rand 0.9.5",
 "rapidhash",
 "regex",
 "regex-syntax",
 "roaring",
 "rustc-hash 2.1.2",
 "rustix 1.1.4",
 "ryu",
 "serde_json",
 "shuttle",
 "simsimd",
 "smallvec",
 "strum",
 "strum_macros",
 "tempfile",
 "thiserror",
 "tracing",
 "tracing-subscriber",
 "turso_ext",
 "turso_macros",
 "turso_parser",
 "twox-hash",
 "uncased",
 "uuid",
 "windows-sys 0.61.2",
]

[[package]]
name = "turso_ext"
version = "0.7.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e7452fe676450b6840e9eb80e512d14293265221154eba66aba3845dfc0d659f"
dependencies = [
 "chrono",
 "getrandom 0.4.3",
 "turso_macros",
]

[[package]]
name = "turso_macros"
version = "0.7.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b2825eaa6b7b893693636947f988e252c1196393fa54d4b85637c70d616d92fa"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "turso_parser"
version = "0.7.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f0e58d029f02e442df4be0b5036ce520b49c553505d9d52ef5a98fc4056e95b7"
dependencies = [
 "bitflags",
 "memchr",
 "miette",
 "strum",
 "strum_macros",
 "thiserror",
 "turso_macros",
]

[[package]]
name = "turso_sdk_kit"
version = "0.7.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "18c1dc1c0304348c39b97bc6b27cdcb1d7292454ebd0de0f30b5ee3a4c61f9bb"
dependencies = [
 "bindgen",
 "env_logger",
 "parking_lot",
 "tracing",
 "tracing-appender",
 "tracing-subscriber",
 "turso_core",
 "turso_ext",
 "turso_sdk_kit_macros",
]

[[package]]
name = "turso_sdk_kit_macros"
version = "0.7.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c4708b5fe678b8730c85964e3d378f75e18c23c4409971360b2209f4e53e4956"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "turso_sync_engine"
version = "0.7.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ce8f03e430240d590d209ac874db52773b7382c627cd604a685affde77622b09"
dependencies = [
 "base64",
 "bytes",
 "crc32c",
 "genawaiter",
 "http",
 "libc",
 "prost",
 "roaring",
 "serde",
 "serde_json",
 "thiserror",
 "tracing",
 "turso_core",
 "turso_parser",
 "uuid",
]

[[package]]
name = "turso_sync_sdk_kit"
version = "0.7.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d48cb47d056c3ec567745761fa6f832358ca30ef9eb0435fdcfb77c558e70089"
dependencies = [
 "bindgen",
 "env_logger",
 "genawaiter",
 "parking_lot",
 "tracing",
 "tracing-appender",
 "tracing-subscriber",
 "turso_core",
 "turso_sdk_kit",
 "turso_sdk_kit_macros",
 "turso_sync_engine",
]

[[package]]
name = "twox-hash"
version = "2.1.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "86a801b3cea342a06d468c8710662aa29e5e05e4f5c0d62f00bbb7f2ad7941c2"
dependencies = [
 "rand 0.10.3",
]

[[package]]
name = "typenum"
version = "1.20.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b6f5e870be6c3b371b77fe0ee0bafb859fa4964b4404c27de1d380043c4dda20"

[[package]]
name = "uncased"
version = "0.9.10"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e1b88fcfe09e89d3866a5c11019378088af2d24c3fbd4f0543f96b479ec90697"
dependencies = [
 "version_check",
]

[[package]]
name = "unicode-ident"
version = "1.0.24"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e6e4313cd5fcd3dad5cafa179702e2b244f760991f45397d14d4ebf38247da75"

[[package]]
name = "unicode-segmentation"
version = "1.13.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c6f5d3c3b1bf09027a88a6bc961fc00497d651009560b5463668dc81b0fa87a8"

[[package]]
name = "unicode-width"
version = "0.1.14"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7dd6e30e90baa6f72411720665d41d89b9a3d039dc45b8faea1ddd07f617f6af"

[[package]]
name = "universal-hash"
version = "0.5.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "fc1de2c688dc15305988b563c3854064043356019f97a4b46276fe734c4f07ea"
dependencies = [
 "crypto-common",
 "subtle",
]

[[package]]
name = "utf16_iter"
version = "1.0.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c8232dd3cdaed5356e0f716d285e4b40b932ac434100fe9b7e0e8e935b9e6246"

[[package]]
name = "utf8_iter"
version = "1.0.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b6c140620e7ffbb22c2dee59cafe6084a59b5ffc27a8859a5f0d494b5d52b6be"

[[package]]
name = "uuid"
version = "1.28.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7cc1186384beb7dd8eedea376413fd654937285ea6c9cfbb928dc3043ea4b606"
dependencies = [
 "getrandom 0.4.3",
 "js-sys",
 "sha1_smol",
 "wasm-bindgen",
]

[[package]]
name = "valuable"
version = "0.1.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ba73ea9cf16a25df0c8caa16c51acb937d5712a8429db78a3ee29d5dcacd3a65"

[[package]]
name = "vcpkg"
version = "0.2.15"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "accd4ea62f7bb7a82fe23066fb0957d48ef677f6eeb8215f372f52e48bb32426"

[[package]]
name = "version_check"
version = "0.9.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a"

[[package]]
name = "walkdir"
version = "2.5.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "29790946404f91d9c5d06f9874efddea1dc06c5efe94541a7d6863108e3a5e4b"
dependencies = [
 "same-file",
 "winapi-util",
]

[[package]]
name = "wasi"
version = "0.11.1+wasi-snapshot-preview1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ccf3ec651a847eb01de73ccad15eb7d99f80485de043efb2f370cd654f4ea44b"

[[package]]
name = "wasip2"
version = "1.0.4+wasi-0.2.12"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b67efb37e106e55ce722a510d6b5f9c17f083e5fc79afc2badeb12cc313d9487"
dependencies = [
 "wit-bindgen",
]

[[package]]
name = "wasm-bindgen"
version = "0.2.128"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aecb87a33d3b0c5e3b7aa46336eaf486cffafbd281b195e4c8b80d50df2351bf"
dependencies = [
 "cfg-if",
 "once_cell",
 "rustversion",
 "wasm-bindgen-macro",
 "wasm-bindgen-shared",
]

[[package]]
name = "wasm-bindgen-macro"
version = "0.2.128"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "a690d511e3c1a8b3a55e33511e3c2c00c78415cd23650f32b808627f5696b9ed"
dependencies = [
 "quote",
 "wasm-bindgen-macro-support",
]

[[package]]
name = "wasm-bindgen-macro-support"
version = "0.2.128"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "411e4887f0071ef2d2164a9d5fdf2d20efbef78fccd3a78b0c10a1dc5295e48a"
dependencies = [
 "bumpalo",
 "proc-macro2",
 "quote",
 "syn 3.0.5",
 "wasm-bindgen-shared",
]

[[package]]
name = "wasm-bindgen-shared"
version = "0.2.128"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "81941cd78d0c92026c33e5e01312845a4cb1e9af3407f9134b100dd03144103e"
dependencies = [
 "unicode-ident",
]

[[package]]
name = "which"
version = "4.4.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "87ba24419a2078cd2b0f2ede2691b6c66d8e47836da3b6db8265ebad47afbfc7"
dependencies = [
 "either",
 "home",
 "once_cell",
 "rustix 0.38.44",
]

[[package]]
name = "winapi-util"
version = "0.1.11"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "c2a7b1c03c876122aa43f3020e6c3c3ee5c05081c9a00739faf7503aeba10d22"
dependencies = [
 "windows-sys 0.61.2",
]

[[package]]
name = "windows-core"
version = "0.62.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b8e83a14d34d0623b51dce9581199302a221863196a1dde71a7663a4c2be9deb"
dependencies = [
 "windows-implement",
 "windows-interface",
 "windows-link",
 "windows-result",
 "windows-strings",
]

[[package]]
name = "windows-implement"
version = "0.60.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "053e2e040ab57b9dc951b72c264860db7eb3b0200ba345b4e4c3b14f67855ddf"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "windows-interface"
version = "0.59.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3f316c4a2570ba26bbec722032c4099d8c8bc095efccdc15688708623367e358"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "windows-link"
version = "0.2.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f0805222e57f7521d6a62e36fa9163bc891acd422f971defe97d64e70d0a4fe5"

[[package]]
name = "windows-result"
version = "0.4.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7781fa89eaf60850ac3d2da7af8e5242a5ea78d1a11c49bf2910bb5a73853eb5"
dependencies = [
 "windows-link",
]

[[package]]
name = "windows-strings"
version = "0.5.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "7837d08f69c77cf6b07689544538e017c1bfcf57e34b4c0ff58e6c2cd3b37091"
dependencies = [
 "windows-link",
]

[[package]]
name = "windows-sys"
version = "0.59.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1e38bc4d79ed67fd075bcc251a1c39b32a1776bbe92e5bef1f0bf1f8c531853b"
dependencies = [
 "windows-targets",
]

[[package]]
name = "windows-sys"
version = "0.61.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ae137229bcbd6cdf0f7b80a31df61766145077ddf49416a728b02cb3921ff3fc"
dependencies = [
 "windows-link",
]

[[package]]
name = "windows-targets"
version = "0.52.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "9b724f72796e036ab90c1021d4780d4d3d648aca59e491e6b98e725b84e99973"
dependencies = [
 "windows_aarch64_gnullvm",
 "windows_aarch64_msvc",
 "windows_i686_gnu",
 "windows_i686_gnullvm",
 "windows_i686_msvc",
 "windows_x86_64_gnu",
 "windows_x86_64_gnullvm",
 "windows_x86_64_msvc",
]

[[package]]
name = "windows_aarch64_gnullvm"
version = "0.52.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "32a4622180e7a0ec044bb555404c800bc9fd9ec262ec147edd5989ccd0c02cd3"

[[package]]
name = "windows_aarch64_msvc"
version = "0.52.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "09ec2a7bb152e2252b53fa7803150007879548bc709c039df7627cabbd05d469"

[[package]]
name = "windows_i686_gnu"
version = "0.52.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "8e9b5ad5ab802e97eb8e295ac6720e509ee4c243f69d781394014ebfe8bbfa0b"

[[package]]
name = "windows_i686_gnullvm"
version = "0.52.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0eee52d38c090b3caa76c563b86c3a4bd71ef1a819287c19d586d7334ae8ed66"

[[package]]
name = "windows_i686_msvc"
version = "0.52.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "240948bc05c5e7c6dabba28bf89d89ffce3e303022809e73deaefe4f6ec56c66"

[[package]]
name = "windows_x86_64_gnu"
version = "0.52.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "147a5c80aabfbf0c7d901cb5895d1de30ef2907eb21fbbab29ca94c5b08b1a78"

[[package]]
name = "windows_x86_64_gnullvm"
version = "0.52.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "24d5b23dc417412679681396f2b49f3de8c1473deb516bd34410872eff51ed0d"

[[package]]
name = "windows_x86_64_msvc"
version = "0.52.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "589f6da84c646204747d1270a2a5661ea66ed1cced2631d546fdfb155959f9ec"

[[package]]
name = "wit-bindgen"
version = "0.57.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1ebf944e87a7c253233ad6766e082e3cd714b5d03812acc24c318f549614536e"

[[package]]
name = "write16"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "d1890f4022759daae28ed4fe62859b1236caebfc61ede2f63ed4e695f3f6d936"

[[package]]
name = "writeable"
version = "0.6.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3ad82d2a33cdc9674dc7465672f271e096168fcdbe0f799d9e6db8c5892679dc"

[[package]]
name = "wyz"
version = "0.5.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "05f360fc0b24296329c78fda852a1e9ae82de9cf7b27dae4b7f62f118f77b9ed"
dependencies = [
 "tap",
]

[[package]]
name = "yoke"
version = "0.8.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "709fe23a0424b6a435d82152b1bd3fdfb0833487d5fa90d05d42762a9891fef5"
dependencies = [
 "stable_deref_trait",
 "yoke-derive",
 "zerofrom",
]

[[package]]
name = "yoke-derive"
version = "0.8.4"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ec8ebde2db3681e8c9980cc27822030e68752690ddfa9473e739aeb4dbde6d71"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 3.0.5",
 "synstructure",
]

[[package]]
name = "zerocopy"
version = "0.8.62"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "86502bf56ac7c77571a32e2647bb2a15894565e981fb2a48d7bde2d91c965a9d"
dependencies = [
 "zerocopy-derive",
]

[[package]]
name = "zerocopy-derive"
version = "0.8.62"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "5457206954b06561e2608c7e19cf58b1926586d999c246eebe4502f7e2039d1a"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 2.0.117",
]

[[package]]
name = "zerofrom"
version = "0.1.8"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0ec05a11813ea801ff6d75110ad09cd0824ddba17dfe17128ea0d5f68e6c5272"
dependencies = [
 "zerofrom-derive",
]

[[package]]
name = "zerofrom-derive"
version = "0.1.8"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f75b4683f6c7f45248d4d64056a24298c6281e0993356d7d1b4a1a962ef10d4a"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 3.0.5",
 "synstructure",
]

[[package]]
name = "zerotrie"
version = "0.2.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "4ea269c3bd32f0a32c321907a2ae912ba6f4649bb0fc764a15627e99a7095a3f"
dependencies = [
 "displaydoc",
 "yoke",
 "zerofrom",
 "zerovec",
]

[[package]]
name = "zerovec"
version = "0.11.8"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bb0464e17806c1d976d5cba29399c7f08e516e279e2ba493f63123b5fca67dd8"
dependencies = [
 "serde",
 "yoke",
 "zerofrom",
 "zerovec-derive",
]

[[package]]
name = "zerovec-derive"
version = "0.11.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "34df6fc39dbd26ddc9c10e6a2984476e13acce22e64e4487636ef494369225da"
dependencies = [
 "proc-macro2",
 "quote",
 "syn 3.0.5",
]

[[package]]
name = "zmij"
version = "1.0.21"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "b8848ee67ecc8aedbaf3e4122217aff892639231befc6a1b58d29fff4c2cabaa"

[[package]]
name = "zstd"
version = "0.13.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "e91ee311a569c327171651566e07972200e76fcfe2242a4fa446149a3881c08a"
dependencies = [
 "zstd-safe",
]

[[package]]
name = "zstd-safe"
version = "7.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "64d80649ab6db9d9f6f9c80a40becd948eda4714a0a5ac8c4d157a32231c7882"
dependencies = [
 "zstd-sys",
]

[[package]]
name = "zstd-sys"
version = "2.1.1+zstd.1.5.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aeec9eaf2dffbbd09201e23bd0ffcbaa33bb8e9266a10734fd7ed90a85eca078"
dependencies = [
 "cc",
 "pkg-config",
]
````

## Task M2.10: complete replay and consumer hand-off

RED/GREEN: `store_import_parser_accepts_source_and_rejects_missing_source`. Install the foreground controllers below. `real_turso_v2_import_rebuilds_and_preserves_source_sidecars` tests the library; the process `import` case invokes the actual `kfs store import` command against a full turso-generated v2 schema, compares source hashes and verifies 200,001 rebuilt entries. `cli_scope` exercises strict admission through CLI flags.

Xross moves `xross_search_exports` into its own DB, replaces its direct turso reader with the shared v3 reader, and builds `SearchScope` from host-authorized export root IDs. Its host chooses a stable consumer ID; peers never supply that ID or use index policy as authorization. Kunkun builds/signs the `kfs daemon` helper as `kuntud`, gives that helper TCC grants, and loads `openReadOnly` in app-server. Root registration and rescan requests use a separate writer/inbox client; the app-server does not crawl. Both products pin the owner-approved v0.6.0 engine commit after implementation and installed-product acceptance. Their worktrees are outside this write set; no consumer pin, signature, push, merge or tag is performed by this replay.

<!-- apply {"task": "M2.10", "file": "scripts/m2-previous-tag.ts", "mode": "file"} -->
````typescript
/** Actual previous-tag epoch isolation; no checkout, branch or tag mutation. */
import {spawnSync} from 'node:child_process';
import {mkdirSync,rmSync,readFileSync,writeFileSync,existsSync} from 'node:fs';
import {resolve,join} from 'node:path';
import {strict as assert} from 'node:assert';
const base='/Volumes/Portable2TB/tmp/kuntu-m2/previous-tag';
const commit='a5df3559671b09f2af75fdd28ed5a4a06ab7dc56';
const src=join(base,'source');mkdirSync(src,{recursive:true});
function call(args:string[],cwd?:string,env:Record<string,string>={}){
 const r=spawnSync(args[0]!,args.slice(1),{cwd,env:{...process.env,...env},encoding:'utf8',maxBuffer:64*1024*1024});
 return {status:r.status,text:r.stdout+r.stderr};
}
function ok(args:string[],cwd?:string,env:Record<string,string>={}){const r=call(args,cwd,env);assert.equal(r.status,0,r.text);return r.text.trim();}
const archive=join(base,'previous.tar');
ok(['git','archive','--format=tar','--output',archive,commit]);ok(['tar','-xf',archive,'-C',src]);
const build=call(['cargo','build','--release','--locked','-p','kuntu-cli'],src,{CARGO_TARGET_DIR:join(base,'target')});writeFileSync(join(base,'build.log'),build.text);assert.equal(build.status,0,build.text);
const old=join(base,'target/release/kfs');
const target=process.env.CARGO_TARGET_DIR??'target';const now=resolve(target,'release/kfs'),probe=resolve(target,'release/examples/m2_probe');
const root=join(base,'files');mkdirSync(root,{recursive:true});writeFileSync(join(root,'retained-file.md'),'x');
const home=join(base,'home');rmSync(home,{recursive:true,force:true});ok([probe,'init',home,root]);
const db=join(home,'index.v2.db');ok([old,'index','rebuild','--root',root,'--db',db]);
const original=readFileSync(db);
const cells:Record<string,string>={previous_tag:commit,previous_epoch:ok([old,'index','status','--db',db])};
cells.current_private_v2=ok([now,'index','status','--db',db]);
assert.deepEqual(readFileSync(db),original,'new private compatibility must not migrate v2 main file');
cells.current_epoch=ok([probe,'scan',home,root]);
const v3=join(home,'index.v3.db');const before=readFileSync(v3);
const oldOnV3=call([old,'index','status','--db',v3]);assert.notEqual(oldOnV3.status,0);assert.match(oldOnV3.text,/schema|version|supported/i);assert.deepEqual(readFileSync(v3),before,'previous tag refusal must preserve epoch3 main bytes');cells.previous_refuses_epoch3=oldOnV3.text.trim();
cells.gc=ok([now,'store','gc'],undefined,{KUNTU_HOME:home});assert.ok(existsSync(db));assert.deepEqual(readFileSync(db),original,'unregistered previous-tag epoch remains preserved');
cells.other_epoch='epoch2 and epoch3 remain distinct; previous tag refuses user_version3; current private-v2 reader preserves source';
writeFileSync(join(base,'matrix.json'),JSON.stringify(cells,null,2));console.log(JSON.stringify(cells,null,2));console.log("PASS previous_tag_epoch_isolation");
````

<!-- apply {"task": "M2.10", "file": "scripts/m2-evidence.ts", "mode": "file"} -->
````typescript
/** Process crash/stress evidence. All children are awaited and killed on failure. */
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { mkdirSync, writeFileSync, existsSync, readFileSync, rmSync, readdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join, resolve } from 'node:path';
import { strict as assert } from 'node:assert';
const base='/Volumes/Portable2TB/tmp/kuntu-m2/evidence';
const target=process.env.CARGO_TARGET_DIR??'target';
const bin=resolve(target,'release/examples/m2_probe');
mkdirSync(base,{recursive:true});
const root=join(base,'root-200k');mkdirSync(root,{recursive:true});
const live=new Set<ChildProcess>();const orphaned=new Set<number>();
function start(op:string,home:string,extra:Record<string,string>={},minor='1') {
 const child=spawn(bin,[op,home,extra.KUNTU_EVIDENCE_ROOT??root,minor],{env:{...process.env,...extra},stdio:['ignore','pipe','pipe']});live.add(child);
 let output='';child.stdout?.on('data',(b:Buffer)=>{output+=b.toString()});child.stderr?.on('data',(b:Buffer)=>{output+=b.toString()});
 const done=new Promise<{code:number|null,text:string}>(r=>child.on('close',(code)=>{live.delete(child);r({code,text:output})}));
 return {child,done};
}
async function ok(op:string,home:string,extra:Record<string,string>={},minor='1') {const r=await start(op,home,extra,minor).done;assert.equal(r.code,0,r.text);return r.text.trim();}
async function marker(path:string,proc:ReturnType<typeof start>) {
 const deadline=Date.now()+600_000;while(!existsSync(path)){assert.equal(proc.child.exitCode,null,'child exited before handshake');assert.ok(Date.now()<deadline,'handshake timeout');await Bun.sleep(10);}
}
async function killed(op:string,home:string,point:string,rows?:number) {
 const m=join(base,`marker-${point}-${rows??0}`);rmSync(m,{force:true});
 const p=start(op,home,{KUNTU_FAULT:point,KUNTU_FAULT_MARKER:m,...(rows===undefined?{}:{KUNTU_FAULT_ROWS:String(rows)})});
 await marker(m,p);p.child.kill('SIGKILL');const r=await p.done;assert.equal(r.code,null);return readFileSync(m,'utf8');
}
function db(home:string,sql:string){const r=spawnSync('python3',['-c','import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); print(c.execute(sys.argv[2]).fetchall()); c.commit()',join(home,'index.v3.db'),sql],{encoding:'utf8',maxBuffer:256*1024*1024});assert.equal(r.status,0,r.stderr);return r.stdout.trim();}
const mode=process.argv[2]??'crash';
try {
 if(mode==='crash') {
  for(let n=0;n<200_000;n++){const p=join(root,`file-${String(n).padStart(6,'0')}.md`);if(!existsSync(p))writeFileSync(p,'x');}
  const home=join(base,'crash-home');rmSync(home,{recursive:true,force:true});
  await ok('init',home);const outcomes:Record<string,string>={};
  for(const percent of [30,70]){
   rmSync(home,{recursive:true,force:true});await ok('init',home);await killed('scan',home,'batch_committed',200_000*percent/100);
   const partialCount=Number(/\d+/.exec(db(home,"SELECT count(*) FROM entries"))?.[0]);assert.ok(partialCount>=200_000*percent/100&&partialCount<200_000*percent/100+5000);
   const partial=db(home,"SELECT status,active_scan_gen,(SELECT value FROM meta WHERE key='generation'),(SELECT count(*) FROM entries) FROM root_state");
   const resumed=await ok('scan',home);assert.equal(db(home,'SELECT count(*) FROM entries'),'[(200001,)]');assert.equal(db(home,"SELECT count(*) FROM entries WHERE scan_gen<>(SELECT active_scan_gen FROM root_state)"),'[(0,)]');outcomes[`kill_${percent}`]=`${partial}; ${resumed}`;
  }
  for(const point of ['building_committed','before_ready','ready_committed']){await killed('scan',home,point);outcomes[point]=await ok('scan',home);}
  const expected=db(home,"SELECT path,kind,size,mtime,flags FROM entries ORDER BY path");
  const fresh=join(base,'fresh-home');rmSync(fresh,{recursive:true,force:true});await ok('init',fresh);await ok('scan',fresh);assert.equal(db(fresh,"SELECT path,kind,size,mtime,flags FROM entries ORDER BY path"),expected);
  const mig=join(base,'migration-home');rmSync(mig,{recursive:true,force:true});await ok('init',mig);await killed('scan',mig,'migration_ddl');assert.equal(db(mig,"SELECT value FROM meta WHERE key='schema_minor'"),'[(0,)]');outcomes.migration=await ok('scan',mig);
  await killed('quarantine',home,'quarantine_renamed');outcomes.quarantine=await ok('init',home);assert.equal(db(home,'SELECT count(*) FROM entries'),'[(0,)]');
  const first=join(base,'first-home');rmSync(first,{recursive:true,force:true});await killed('identity',first,'identity_synced');const x=await ok('identity',first);assert.equal(x.length,32);
  const simultaneous=join(base,'simultaneous');rmSync(simultaneous,{recursive:true,force:true});mkdirSync(simultaneous);for(let n=0;n<100;n++){const pair=join(simultaneous,`pair-${n}`);const a=start('identity',pair),b=start('identity',pair);const ar=await a.done,br=await b.done;assert.equal(ar.code,0,ar.text);assert.equal(br.code,0,br.text);assert.equal(ar.text,br.text);}outcomes.concurrent_identity='100 concurrent fresh-home pairs: one identity per pair';
  const heldMarker=join(base,'held');rmSync(heldMarker,{force:true});const held=start('hold',fresh,{KUNTU_FAULT_MARKER:heldMarker});await marker(heldMarker,held);
  const denied=await start('quarantine',fresh).done;assert.notEqual(denied.code,0);held.child.kill('SIGKILL');await held.done;
  const held2Marker=join(base,'held2');rmSync(held2Marker,{force:true});const held2=start('hold',fresh,{KUNTU_FAULT_MARKER:held2Marker});await marker(held2Marker,held2);
  const poll=start('poll',fresh);await Bun.sleep(500);const released=Date.now();held2.child.kill('SIGKILL');await held2.done;const takeover=await poll.done;assert.equal(takeover.code,0,takeover.text);const measured=Date.now()-released;assert.ok(measured<=6000);outcomes.takeover=`${measured} ms after kill; ${takeover.text.trim()}`;
  const execText=await ok('exec-holder',fresh);const pid=Number(/child_pid=(\d+)/.exec(execText)?.[1]);assert.ok(pid>0);orphaned.add(pid);process.kill(pid,0);assert.match(await ok('skew-writer',fresh),/acquired=true/);process.kill(pid,0);process.kill(pid,'SIGKILL');orphaned.delete(pid);outcomes.exec='child alive before and after a second process reacquired the writer lease';
  const source=join(base,'v2-source');rmSync(source,{recursive:true,force:true});await ok('create-v2',source);
  const files=['source.v2.db','source.v2.db-wal','source.v2.db-shm'];const hashes=()=>files.map(f=>{const p=join(source,f);return existsSync(p)?createHash('sha256').update(readFileSync(p)).digest('hex'):null});const before=hashes();
  const imported=join(base,'imported');rmSync(imported,{recursive:true,force:true});const importedResult=spawnSync(resolve(target,'release/kfs'),['store','import',join(source,'source.v2.db')],{env:{...process.env,KUNTU_HOME:imported},encoding:'utf8'});assert.equal(importedResult.status,0,importedResult.stderr);assert.deepEqual(hashes(),before);assert.equal(db(imported,'SELECT count(*) FROM entries'),'[(200001,)]');outcomes.import=importedResult.stdout.trim();
  const scopedRoot=join(base,'scoped-files');mkdirSync(join(scopedRoot,'node_modules'),{recursive:true});writeFileSync(join(scopedRoot,'visible-file.md'),'x');writeFileSync(join(scopedRoot,'.hidden-file.md'),'x');writeFileSync(join(scopedRoot,'node_modules','ignored-file.md'),'x');const scopedHome=join(base,'scoped-home');rmSync(scopedHome,{recursive:true,force:true});await ok('init-permissive',scopedHome,{KUNTU_EVIDENCE_ROOT:scopedRoot});
  const scopedEnv={...process.env,KUNTU_HOME:scopedHome};const rebuild=spawnSync(resolve(target,'release/kfs'),['index','rebuild','--root',scopedRoot],{env:scopedEnv,encoding:'utf8'});assert.equal(rebuild.status,0,rebuild.stderr);const searched=spawnSync(resolve(target,'release/kfs'),['search','file','--root',scopedRoot,'--provider','sqlite','--include-hidden','--include-ignored','--json'],{env:scopedEnv,encoding:'utf8'});assert.equal(searched.status,0,searched.stderr);assert.ok(searched.stdout.includes('visible-file'));assert.ok(!searched.stdout.includes('.hidden-file')&&!searched.stdout.includes('node_modules'));outcomes.cli_scope='strict CLI excludes union rows and supplied query flags';
  writeFileSync(join(base,'crash.json'),JSON.stringify(outcomes,null,2));console.log(JSON.stringify(outcomes,null,2));
 } else if(mode==='stress') {
  const stressRoot=join(base,'root-1m');mkdirSync(stressRoot,{recursive:true});
  for(let n=0;n<1_000_000;n++){const p=join(stressRoot,`file-${String(n).padStart(6,'0')}.md`);if(!existsSync(p))writeFileSync(p,'x');}
  const extra={KUNTU_EVIDENCE_ROOT:stressRoot};
  const home=join(base,'stress-home');rmSync(home,{recursive:true,force:true});await ok('init',home,extra);
  const started=Date.now();const writers=[start('writer',home,extra),start('writer',home,extra)];const readers=Array.from({length:8},()=>start('reader',home,extra));
  const results=await Promise.all([...writers,...readers].map(p=>p.done));for(const r of results)assert.equal(r.code,0,r.text);
  const metrics=results.slice(2).map(r=>JSON.parse(r.text) as {queries:number,errors:number,p99_us:number,max_us:number});
  const report={root_entries:1_000_001,probe:'alternating entry-rowid point and 100-row id range',seconds:(Date.now()-started)/1000,writers:results.slice(0,2).map(r=>r.text.trim()),readers:metrics,integrity:db(home,'PRAGMA integrity_check')};assert.equal(report.integrity,"[('ok',)]");writeFileSync(join(base,'stress.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report,null,2));
  assert.equal(db(home,'SELECT count(*) FROM entries'),'[(1000001,)]');
  for(const m of metrics){assert.equal(m.errors,0);assert.ok(m.p99_us<=5000,JSON.stringify(m));assert.ok(m.max_us<=100000,JSON.stringify(m));}
 } else if(mode==='skew') {
  const cells:Record<string,string>={};
  const h=join(base,'skew');rmSync(h,{recursive:true,force:true});await ok('init',h,{},'0');
  for(const minor of ['0','1']){cells[`same_reader_${minor}`]=await ok('skew-reader',h,{},minor);cells[`same_writer_${minor}`]=await ok('skew-writer',h,{},minor);}
  cells.current_reader_new_minor=await ok('skew-reader',h,{},'1');cells.current_writer_new_minor=await ok('skew-writer',h,{},'1');
  cells.old_reader_new_minor=await ok('skew-reader',h,{},'0');const old=await start('skew-writer',h,{},'0').done;assert.notEqual(old.code,0);cells.old_writer_new_minor=old.text.trim();
  db(h,"UPDATE meta SET value=2 WHERE key='min_reader_minor'");const refusedBytes=readFileSync(join(h,'index.v3.db'));for(const minor of ['0','1']){const r=await start('skew-reader',h,{},minor).done;assert.notEqual(r.code,0);cells[`new_reader_floor_${minor}`]=r.text.trim();assert.deepEqual(readFileSync(join(h,'index.v3.db')),refusedBytes,'refused reader preserves database bytes');}
  const epoch4=join(h,'index.v4.db');writeFileSync(epoch4,'future epoch sentinel');const original=readFileSync(epoch4);const cli=spawnSync(resolve(target,'release/kfs'),['store','gc'],{env:{...process.env,KUNTU_HOME:h},encoding:'utf8'});assert.equal(cli.status,0,cli.stderr);assert.deepEqual(readFileSync(epoch4),original);cells.other_epoch='future sentinel unchanged';
  writeFileSync(join(base,'skew.json'),JSON.stringify(cells,null,2));console.log(JSON.stringify(cells,null,2));
 } else throw new Error(`Unknown mode ${mode}`);
} finally {
 for(const pid of orphaned){try{process.kill(pid,'SIGKILL')}catch{}}
 const remaining=[...live];
 await Promise.all(remaining.map(child=>new Promise<void>(resolve=>{child.once('close',()=>resolve());child.kill('SIGKILL');})));
}
````

<!-- apply {"task": "M2.10", "file": "scripts/m2-red.ts", "mode": "file"} -->
````typescript
/** Exact defect replacements. RED must be a named test failure, then GREEN. */
import {readFileSync,writeFileSync,mkdirSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {strict as assert} from 'node:assert';
type Case={task:string,file:string,old:string,bad:string,crate:string,test:string};
const cases:Case[]=[
 {task:'M2.4-client-version',file:'crates/kuntu-index/src/shared.rs',old:'if !(0..=BUILD_MINOR).contains(&minor) {',bad:'if minor < 0 {',crate:'kuntu-index',test:'m2_tests::caller_cannot_claim_a_future_build_minor_to_bypass_writer_floor'},
 {"task": "M2.2-exec", "file": "crates/kuntu-store/src/lib.rs", "old": "    let mut fd = create_lock_file(&locks, || {\n      openat(\n        &locks,\n        name,\n        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,\n        Mode::from_bits_truncate(0o600),\n      )\n    })?;\n", "bad": "    let mut fd = create_lock_file(&locks, || {\n      openat(\n        &locks,\n        name,\n        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NOFOLLOW,\n        Mode::from_bits_truncate(0o600),\n      )\n    })?;\n", "crate": "kuntu-store", "test": "tests::executed_child_does_not_inherit_the_permanent_writer_lease"},
 {task:'M2.1-lock-create',file:'crates/kuntu-store/src/lib.rs',old:'attempts < 100',bad:'attempts < 0',crate:'kuntu-store',test:'tests::transient_lock_creation_retries_only_not_found_under_the_same_directory_fd'},
 {task:'M2.5-cleanup',file:'crates/kuntu-index/src/shared.rs',old:'rows >= 5000 || started.elapsed() >= std::time::Duration::from_millis(100)',bad:'rows >= 5000',crate:'kuntu-index',test:'m2_tests::slow_delete_triggers_force_the_100_ms_cleanup_boundary'},
 {task:'M2.1',file:'crates/kuntu-store/src/lib.rs',old:'let flags = OFlag::O_RDWR\n      | OFlag::O_NOFOLLOW',bad:'let flags = OFlag::O_RDWR',crate:'kuntu-store',test:'tests::descriptor_open_rejects_symlink_and_private_modes_are_exact'},
 {task:'M2.2',file:'crates/kuntu-store/src/lib.rs',old:'else if !fd.try_lock_exclusive()? {',bad:'else if false && !fd.try_lock_exclusive()? {',crate:'kuntu-store',test:'tests::permanent_lease_inode_is_exclusive_and_reusable'},
 {task:'M2.3',file:'crates/kuntu-index/src/lib.rs',old:'SELECT entry_id FROM {} WHERE term >= ?1 AND term < ?2 LIMIT ?3',bad:'SELECT entry_id FROM {} WHERE term >= ?1 AND term < ?2 LIMIT ?1',crate:'kuntu-index',test:'m2_tests::production_builders_pin_both_postings_reverse_and_rowid'},
 {task:'M2.4',file:'crates/kuntu-index/src/shared.rs',old:'if read_min > minor {',bad:'if false && read_min > minor {',crate:'kuntu-index',test:'m2_tests::actual_opens_enforce_skew_before_wal_and_old_writer_steps_down'},
 {task:'M2.5',file:'crates/kuntu-index/src/engine.rs',old:"UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='generation'",bad:"UPDATE meta SET value=CAST(value AS INTEGER)+0 WHERE key='generation'",crate:'kuntu-index',test:'m2_tests::batch_generation_is_atomic_and_readers_observe_data_version'},
 {task:'M2.6',file:'crates/kuntu-index/src/shared.rs',old:"status='building' AND active_scan_gen=?2 AND scan_policy_revision=policy_revision",bad:"status='building' AND ?2=?2 AND scan_policy_revision=policy_revision",crate:'kuntu-index',test:'m2_tests::resume_keeps_generation_stale_completion_is_rejected_and_building_is_reported'},
 {task:'M2.7',file:'crates/kuntu-index/src/shared.rs',old:'let _ = t.join();',bad:'drop(t);',crate:'kuntu-index',test:'m2_tests::joined_takeover_service_releases_lease_only_after_worker_stops'},
 {task:'M2.8',file:'crates/kuntu-napi/src/lib.rs',old:'read_only: true,',bad:'read_only: false,',crate:'node',test:'openReadOnly enforces access on rebuild refresh repair and registration'},
 {task:'M2.9',file:'crates/kuntu-daemon/src/lib.rs',old:'if index.is_shared() {',bad:'if false && index.is_shared() {',crate:'kuntu-daemon',test:'tests::non_holder_bulk_endpoint_queues_and_empty_search_is_rejected'},
 {task:'M2.10',file:'crates/kuntu-cli/src/args.rs',old:'"store" => {',bad:'"store-disabled" => {',crate:'kuntu-cli',test:'args::tests::store_import_parser_accepts_source_and_rejects_missing_source'},
];
const base='/Volumes/Portable2TB/tmp/kuntu-m2/replay';mkdirSync(base,{recursive:true});
function command(args:string[],cwd?:string){const r=spawnSync(args[0]!,args.slice(1),{cwd,env:{...process.env,...(args[0]==='cargo'?{CARGO_PROFILE_RELEASE_LTO:'false',CARGO_PROFILE_RELEASE_CODEGEN_UNITS:'16'}:{})},encoding:'utf8',maxBuffer:16*1024*1024});return {status:r.status,text:r.stdout+r.stderr};}
function test(c:Case){
 if(c.crate==='node'){
  const b=command(['pnpm','run','build:debug'],'crates/kuntu-napi');assert.equal(b.status,0,b.text);
  return command(['node','--test','--test-name-pattern',c.test,'test/index.test.cjs'],'crates/kuntu-napi');
 }
 return command(['cargo','test','--release','-p',c.crate,c.test,'--','--exact','--test-threads=1','--nocapture']);
}
const chosen=process.argv[2];
for(const c of cases.filter(c=>!chosen||c.task===chosen)){
 const original=readFileSync(c.file,'utf8');
 // M2.9 has the same guard for refresh. Replace exactly the first occurrence,
 // at the unique rebuild arm; it is an exact bounded anchor replacement.
 const n=original.split(c.old).length-1;assert.equal(n,c.task==='M2.9'?2:1,`anchor ${c.task} count=${n}`);
 try {
  writeFileSync(c.file,original.replace(c.old,c.bad));const red=test(c);writeFileSync(`${base}/${c.task}-RED.log`,red.text);
  assert.notEqual(red.status,0,`${c.task} RED unexpectedly passed`);assert.ok(red.text.includes(c.test),red.text);assert.match(red.text,/FAILED|not ok|✖/);assert.doesNotMatch(red.text,/could not compile|error\[E\d+\]/);
 } finally {writeFileSync(c.file,original);}
 const green=test(c);writeFileSync(`${base}/${c.task}-GREEN.log`,green.text);assert.equal(green.status,0,green.text);
 if(c.crate!=='node')assert.match(green.text,/1 passed; 0 failed/);
 console.log(`${c.task}: RED named failure, GREEN passed (${c.test})`);
}
````

<!-- apply {"task": "M2.10", "file": "scripts/m2-replay.ts", "mode": "file"} -->
````typescript
/** Applies only the committed complete-file payload, then replays every task. */
import {readFileSync,writeFileSync,mkdirSync} from 'node:fs';
import {dirname,resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
import {strict as assert} from 'node:assert';
import {createHash} from 'node:crypto';
const planPath=process.argv[2];assert.ok(planPath,'pass the git-show committed plan file');
const plan=readFileSync(planPath,'utf8');
const base='/Volumes/Portable2TB/tmp/kuntu-m2/replay';mkdirSync(base,{recursive:true});
process.env.CARGO_TARGET_DIR='/Volumes/Portable2TB/tmp/kuntu-m2/target';
process.env.KUNTU_TEST_DIR='/Volumes/Portable2TB/tmp/kuntu-m2/contracts';
process.env.TMPDIR='/Volumes/Portable2TB/tmp/kuntu-m2/testtmp';mkdirSync(process.env.TMPDIR,{recursive:true});
process.env.KUNTU_BENCH_DIR='/Volumes/Portable2TB/tmp/kuntu-m2/bench-final';process.env.KUNTU_BENCH_ROWS='1000000';
const pattern=/<!-- apply ({[^\n]+}) -->\n(`{3,})[^\n]*\n([\s\S]*?)\n\2/g;
const files=new Map<string,string>();
for(const match of plan.matchAll(pattern)){
 const meta:unknown=JSON.parse(match[1]!);
 assert.ok(typeof meta==='object'&&meta!==null&&'file'in meta&&'mode'in meta&&'task'in meta);
 assert.ok(typeof meta.file==='string'&&typeof meta.task==='string'&&meta.mode==='file');
 assert.ok(!meta.file.startsWith('/')&&!meta.file.split('/').includes('..'));
 assert.ok(!files.has(meta.file),`duplicate complete payload ${meta.file}`);
 files.set(meta.file,match[3]!+'\n');
}
const setSection=plan.split('## Exact write set\n')[1]?.split('\n## ')[0];assert.ok(setSection);
const declared=[...setSection.matchAll(/^- `([^`]+)`/gm)].map(m=>m[1]!);
assert.deepEqual([...files.keys()].sort(),declared.sort(),'plan_apply_write_set_matches_complete_payloads');
for(const [file,contents]of files){mkdirSync(dirname(file),{recursive:true});writeFileSync(file,contents);}
const hashes=Object.fromEntries([...files].map(([f,s])=>[f,createHash('sha256').update(s).digest('hex')]));
writeFileSync(`${base}/payload-sha256.json`,JSON.stringify(hashes,null,2));
function run(label:string,args:string[],cwd?:string,extra:Record<string,string>={}){
 console.log(`RUN ${label}`);const r=spawnSync(args[0]!,args.slice(1),{cwd,env:{...process.env,...extra},encoding:'utf8',maxBuffer:64*1024*1024});const text=r.stdout+r.stderr;writeFileSync(`${base}/${label}.log`,text);assert.equal(r.status,0,`${label}\n${text.slice(-12000)}`);return text;
}
run('fmt',['cargo','fmt','--all','--','--check']);
run('clippy',['cargo','clippy','--workspace','--all-targets','--locked','--','-D','warnings']);
run('napi-install',['pnpm','install','--frozen-lockfile'],'crates/kuntu-napi');
run('reds-greens',['bun','scripts/m2-red.ts']);
run('workspace',['cargo','test','--workspace','--locked','--','--test-threads=1']);
const js=readFileSync('crates/kuntu-napi/index.js');const dts=readFileSync('crates/kuntu-napi/index.d.ts');
run('napi-build',['pnpm','run','build:debug'],'crates/kuntu-napi');
assert.deepEqual(readFileSync('crates/kuntu-napi/index.js'),js,'generated JS differs');assert.deepEqual(readFileSync('crates/kuntu-napi/index.d.ts'),dts,'generated declarations differ');
run('node',['node','--test','test/index.test.cjs'],'crates/kuntu-napi');run('types',['pnpm','run','typecheck'],'crates/kuntu-napi');
run('m1-gate',['cargo','test','--release','-p','kuntu-index','realistic_fixture_latency_gate','--','--ignored','--nocapture']);
run('m1-recall',['cargo','test','--release','-p','kuntu-index','rarest_token_drives_the_search_and_recall_is_exact','--','--ignored','--nocapture']);
run('m1-budget',['cargo','test','--release','-p','kuntu-index','default_budget_bounds_a_broad_single_token','--','--ignored','--nocapture']);
run('sqlite-version',['cargo','test','--release','-p','kuntu-index','bundled_runtime_includes_wal_reset_fix','--','--nocapture']);
run('size-delete',['cargo','test','--release','-p','kuntu-index','realistic_fixture_reverse_delete_cost_and_size_gate','--','--ignored','--nocapture']);
run('probe-build',['cargo','build','--release','-p','kuntu-index','--example','m2_probe']);run('cli-build',['cargo','build','--release','-p','kuntu-cli']);
run('crash',['bun','scripts/m2-evidence.ts','crash']);run('stress',['bun','scripts/m2-evidence.ts','stress']);run('skew',['bun','scripts/m2-evidence.ts','skew']);
run('previous-tag',['bun','scripts/m2-previous-tag.ts']);
run('fmt-final',['cargo','fmt','--all','--','--check']);
const protectedPaths=['crates/kuntu-crawler','crates/kuntu-watcher','crates/kuntu-scan','.github'];
assert.equal(run('protected',['git','diff','--name-only','e164d67','--',...protectedPaths]).trim(),'');assert.equal(run('protected-untracked',['git','ls-files','--others','--exclude-standard','--',...protectedPaths]).trim(),'');
for(const [f,hash]of Object.entries(hashes))assert.equal(createHash('sha256').update(readFileSync(f)).digest('hex'),hash,`payload changed ${f}`);
console.log(`PASS: plan_apply_write_set_matches_complete_payloads; ${files.size} files; all tasks and evidence commands completed`);
````

## Crash and state checklist

| Question | Behavior and named test |
|---|---|
| Two concurrent first opens | Bootstrap publishes one identity/schema. `two_concurrent_first_opens_publish_one_random_identity`; process `concurrent_identity`, plus `identity_synced` kill. |
| Two writers on the same store | Capability refuses the second bulk writer; small inbox writes remain guarded. `lease_capability_blocks_second_writer_and_payload_is_not_authority`, `private_open_cannot_bypass_a_shared_store_lease`; 60 s `stress` during a physical 1M-file rebuild. |
| Kill between batch commit and generation bump | There is no separate bump interval: both are one commit. `batch_generation_is_atomic_and_readers_observe_data_version`; process `kill_30` / `kill_70` kill immediately after commit. |
| Kill between building and ready | Building resumes its generation, stale completion cannot delete newer data. `resume_keeps_generation_stale_completion_is_rejected_and_building_is_reported`; `building_committed`, `before_ready`, `ready_committed`. |
| Kill during quarantine | A synced marker names the whole DB/WAL/SHM group; startup finishes it before reuse. `live_readers_prevent_quarantine_and_gc_never_touches_future_epochs`; process `quarantine`. |
| Kill during migration | DDL and metadata roll back together; next holder reruns the additive step. `actual_opens_enforce_skew_before_wal_and_old_writer_steps_down`; process `migration`. |
| Disable/remove consumer during scan | Policy revision invalidates the active generation; no stale completion. `consumer_removal_and_disable_invalidate_inflight_generation`. |
| Restart recovery while a live holder exists | No recovery writes until lease acquisition. `restart_recovery_waits_for_live_holder_then_resumes_persisted_generation`. |
| Is supplied state trusted as truth? | No. Membership, allowance, policy revision, minor floors and generation come from the opened store. The supported build minor comes from compiled code; a caller cannot claim a future version. `caller_cannot_claim_a_future_build_minor_to_bypass_writer_floor`, `strict_consumer_never_inherits_union_or_client_flags`, Node strict-consumer test, daemon strict-admission test and `cli_scope`. |
| Re-check after blocking steps? | Minor after transaction lock; generation/revision after crawl and each transaction wait; metadata after migration lock; lifetime under epoch lock. `every_write_transaction_rechecks_minor_and_rolls_back`, consumer-in-flight test, restart-recovery test and process skew cases. |

## Finding closure map

| Finding | Tasks | Named regression/evidence |
|---|---|---|
| 1 Mechanical compile | M2.1, M2.3, M2.8–10 | `plan_apply_write_set_matches_complete_payloads`, fmt/clippy/locked workspace build |
| 2 Complete engine/fixture port | M2.3 | `realistic_fixture_latency_gate`, all preserved M1 recall/shape tests |
| 3 Lease gates/lifetime/hosts | M2.2, M2.7–9 | `lease_capability_blocks_second_writer_and_payload_is_not_authority`, `private_open_cannot_bypass_a_shared_store_lease`, `executed_child_does_not_inherit_the_permanent_writer_lease`, joined-service test, daemon queue test, exec/takeover process cases |
| 4 Actual skew/migration/guards | M2.4 | `actual_opens_enforce_skew_before_wal_and_old_writer_steps_down`, `caller_cannot_claim_a_future_build_minor_to_bypass_writer_floor`, `every_write_transaction_rechecks_minor_and_rolls_back`, migration kill, minor skew controller and `previous_tag_epoch_isolation` (v0.4.1 archive) |
| 5 Batches/recovery/notifications | M2.5–6 | `slow_scan_rows_force_the_100_ms_batch_boundary`, `slow_delete_triggers_force_the_100_ms_cleanup_boundary`, atomic-generation/data-version test, resume/building test, 30%/70% kills |
| 6 Indexed reverse deletes/lossy recall | M2.3, M2.6 | `production_builders_pin_both_postings_reverse_and_rowid`, `entry_terms_fallback_uses_the_entry_id_index`, both M1 lossy-path tests; `realistic_fixture_reverse_delete_cost_and_size_gate` (measured size and 5,000-row delete) |
| 7 Consumer scope | M2.3, M2.8–9 | `strict_consumer_never_inherits_union_or_client_flags`, `requested_scope_cannot_widen_config_even_on_an_early_return`, Node/daemon strict tests, `cli_scope` |
| 8 FD opens/permissions | M2.1, M2.3 | `descriptor_open_rejects_symlink_and_private_modes_are_exact`, `opened_directory_survives_path_swap_and_sidecars_are_private`, `fd_vfs_preserves_the_unix_open_abi_for_private_connections` |
| 9 Atomic random identity | M2.1 | `atomic_identity_is_128_bits_and_existing_document_is_preserved`, concurrent-open test, `transient_lock_creation_retries_only_not_found_under_the_same_directory_fd`, 100 process pairs and publication kill |
| 10 Reader-safe quarantine/GC | M2.4, M2.9 | `live_readers_prevent_quarantine_and_gc_never_touches_future_epochs`, quarantine kill; unregistered private epochs preserved |
| 11 NAPI actual access/compat | M2.8 | Node read-only operation test, `old_constructor_opens_exact_v2_path_and_refuses_unrelated_database` |
| 12 Import end to end | M2.9–10 | `store_import_parser_accepts_source_and_rejects_missing_source`, real-turso library import test, process `import` |
| 13 Production SQL pins/binds | M2.3 | `production_builders_pin_both_postings_reverse_and_rowid`, preserved `entry_fetch_is_a_rowid_seek_not_an_index_scan` |
| 14 Real RED/GREEN execution | M2.1–10 | `m2-red.ts` runs fourteen pairs across all ten tasks, requires the named failure and rejects compilation errors/zero-test passes |
| 15 Complete write set | M2.1–10 | `plan_apply_write_set_matches_complete_payloads`, protected-file checks |
| 16 Patched WAL engine | M2.3 | `bundled_runtime_includes_wal_reset_fix`, stress and query gates |
| 17 Honest complete evidence | M2.10 | Full foreground replay logs, crash/stress/skew artifacts and the measured table below |

## Replay invocation

From a detached scratch worktree at `e164d67` under `/Volumes/Portable2TB/ExtDev/`, save `git show HEAD:docs/plans/M2-shared-store.md` from the plan worktree into `/Volumes/Portable2TB/tmp/kuntu-m2/committed-plan.md`. Extract only the `scripts/m2-replay.ts` complete-file block to `/Volumes/Portable2TB/tmp/kuntu-m2/replay-entry.ts`, then run `bun /Volumes/Portable2TB/tmp/kuntu-m2/replay-entry.ts /Volumes/Portable2TB/tmp/kuntu-m2/committed-plan.md` in that scratch worktree. The script itself installs all payload files. Remove only that scratch worktree when done. No task asks an agent to push, merge, tag or commit implementation files.

Sensitivity tests use release optimization with LTO disabled and 16 codegen units to keep repeated fault builds practical. The M1 gate and process acceptance use the repository's default release profile. Cached fixtures are v3-specific and separate from M1/v2 caches. A successful M1 gate is not a claim that the tighter M2 ceilings are met.

## Replay results

Pending the committed-blob replay. Pilot compilation/tests are not acceptance evidence. The final revision must replace this paragraph with measured results and retain all failed or not-run targets as explicit deviations.
