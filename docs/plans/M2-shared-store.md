# Kuntu M2: shared `~/.kuntu` store (v0.6.0)

Executable hand-off plan for an implementation worker. Sources: `PLAN.md` §5 (M2) and its M2.1–M2.10 task table plus §8–§9, `RESEARCH.md` §§6/4.1/10 (turso exclusive lock, WAL stress, storage), all in `/Volumes/Portable2TB/tmp/kuntu/`, and the merged M1 plan `docs/plans/M1-query-shape.md` as the format example. Owner defaults are frozen (do not reopen): D1 shared `~/.kuntu` on SQLite WAL via rusqlite (turso holds an exclusive cross-process lock, RESEARCH §6.1); `KUNTU_HOME` override; D3 indexer lease held by a signed `kuntud` with an embedded fallback; D4 schema epoch is the file name with additive minors; D11 shared file with an enforced non-empty scope (searches require explicit root ids); D13 old epochs GC after 30 days; D14 migrations are additive and run by the lease holder; D12 macOS first.

**Replay status.** Every edit block below was applied mechanically to a clean worktree at `HEAD` (`e164d67`, M1 merged) and every RED/GREEN step was run as written; measured results are in “Replay results” at the end of this file.

## Goal

One store shared by Xross, Kunkun, the CLI and the daemon: many concurrent readers plus exactly one writer across processes, crash-safe, version-skew-safe, with the M1 query shape unchanged.

1. `~/.kuntu` layout with `KUNTU_HOME` override, `0700`/`0600` permissions, symlink and world-writable refusal, network-FS refusal, `store.json`.
2. SQLite WAL engine via `rusqlite` bundled: `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000`, `foreign_keys=ON`, `wal_autocheckpoint=1000`, `mmap_size` capped at 64 MiB. Readers open read-only and hold no transaction across calls.
3. Schema epoch 3 file `index.v3.db` with `name_postings`/`path_postings` split (`WITHOUT ROWID`), no `terms` migration (derived data, rebuild from scratch).
4. Indexer lease (`locks/indexer.lock`, `flock`) with JSON payload; OS releases on death including `kill -9`. Small writes (`requests`, `root_consumers`) via `BEGIN IMMEDIATE` plus `busy_timeout`. Takeover poller every 5 s, takeover within 6 s.
5. Resumable scan (`scan_gen`, `building`/`ready`), short write batches (at most 5,000 rows or 100 ms), `meta.generation` bump per batch, `PRAGMA data_version` for readers.
6. Additive minor migrations by the lease holder under `migrate.lock` + short `BEGIN EXCLUSIVE`; epoch = file name; old epochs GC after 30 days via `kfs store gc`.
7. Enforced non-empty `SearchScope`; M1 shapes and latency gate stay green; turso import is rebuild-only; JS/NAPI keeps the old ctor working.

## Baseline (why this is needed)

Measured on `4c5dbfd`/`e164d67`, Apple M4 Pro, external disk (RESEARCH §§2–4,6):

| Fact | Number |
|---|---|
| turso 0.7.2 and 0.8.3 second-process open (read or write) | **fails immediately**: `Locking error … File is locked by another process` |
| SQLite WAL stress, 2 writers + 4 readers, 12 s | 0 errors of ~109k queries, reader p50 0.41 ms p99 0.51 ms, `kill -9` mid-write leaves `integrity_check = ok` |
| Shape-fixed pipeline at 1M realistic rows | turso 82–192 ms vs SQLite 64–146 ms warm (20k–50k candidates) |
| Current v2 file at 1M rows | 1.64 GB (`terms` 335 MB + PK 345 MB + redundant idx 248+225+88+86 MB + entries 207 MB) |
| Proposed v3 at 1M rows | ~0.5 GB (merged `WITHOUT ROWID` postings 205 MB + entries 207 MB + unique idx 87 MB) |
| M1 gate at 1M rows (turso, after M1) | all nine classes green (miss 0.0 ms, broad 5k-budget 40 ms, mixed exact 271 rows 39 ms) |

Root causes: turso’s exclusive file lock forbids a shared store; v2 `terms(term,entry_id,field)` plus redundant indexes waste ~1.1 GB; single-transaction `rebuild` blocks writers and loses progress on crash.

## Rules

* Work on a new branch `feat/m2-shared-store` created from `main`. Read this file from the plan branch: `git show docs/m2-plan:docs/plans/M2-shared-store.md`. **Never push, merge, tag, or touch another branch or worktree.** The owner tags `v0.6.0`.
* One commit per task, message given in each task. Run `cargo fmt --all` before every commit. No attribution lines.
* Test-first: apply the tests, run them, confirm each RED fails **as stated** (a different failure means stop and report), then apply the implementation and confirm GREEN.
* Run every command in the foreground and wait. Use `--release` for all `kuntu-index` gate runs. Keep scratch data out of `/private/tmp`; use `/Volumes/Portable2TB/tmp/kuntu-m2-bench` (or any directory outside the repo) for `KUNTU_BENCH_DIR`. Validate snippets only in a scratch copy under `/Volumes/Portable2TB/tmp/kuntu-m2/` and discard it.
* Report exact evidence, known failures, and not-run areas. A passing checklist is not proof; the numbers are.
* Edits are given as exact anchors. Each HTML comment that starts with the word `apply` (in a one-line JSON form) is the machine-readable form of the edit that follows it (`create`, `insert-before` an anchor string, `replace-range` from a start string up to, not including, an end string, or `append-to-tests-module`, which inserts before the final `}` of `mod tests`). The named anchor block occurs exactly once; replace exactly it. Do not apply unified diffs.
* One grep was run before writing to resolve anchors: `use turso::{params, params_from_iter, Value};`, `pub const SCHEMA_VERSION`, `const SCHEMA_DDL`, `fn block_on`, `pub struct FileSearchIndex`, workspace `members` were each confirmed to occur once. No conditional instructions remain; every choice below states its default.

## Exact write set

* `Cargo.toml` (add `kuntu-store` member only)
* `Cargo.lock` (regenerated by cargo; commit it with M2.3)
* `crates/kuntu-store/Cargo.toml` (new)
* `crates/kuntu-store/src/lib.rs` (new)
* `crates/kuntu-store/src/lease.rs` (new)
* `crates/kuntu-core/src/model.rs` (add `SearchScope` only)
* `crates/kuntu-index/Cargo.toml` (turso→rusqlite swap only)
* `crates/kuntu-index/src/lib.rs` (edits only at the anchors below)
* `crates/kuntu-napi/src/lib.rs` (edits only at the anchors below)
* `crates/kuntu-cli/src/args.rs`, `crates/kuntu-cli/src/main.rs` (store subcommands only)
* `crates/kuntu-daemon/src/lib.rs` (lease + thread-pool edits only)
* `justfile` (add `benchmark-v3` recipe only)
* `README.md` (one `Revisions` row only)

Nothing else. In particular no `kuntu-crawler`, `kuntu-watcher`, `kuntu-scan`, CI workflow, or `.github` changes. `git diff main --stat -- crates/kuntu-crawler crates/kuntu-watcher crates/kuntu-scan .github` must be empty at the end.

## Constraints

* **Keep the M1 query shape.** Stage 1 stays `SELECT entry_id … WHERE term >= ? AND term < ? LIMIT ?` on a covering PK index with Rust dedupe, non-ASCII upper bound `prefix + U+10FFFF`, rarest-token driver with Rust verify, stage 2 stays bare `WHERE id IN (…)` rowid seek with Rust `CandidateScope::admit` before ranking and before the budget. `EXPLAIN QUERY PLAN` asserts pin both statements. `SearchQuery` gains no new field (Xross builds it with a struct literal); scope is a separate `SearchScope` newtype.
* **Keep `unsafe_code = "forbid"`** in every crate. `rusqlite` bundled C is a dependency, not our `unsafe`. `flock` goes through the safe wrapper `fs4 0.13` (`use fs4::fs_std::FileExt`), never raw `libc`.
* **No breaking NAPI surface.** `new FileSearchIndex(dbPath)` keeps working (deprecated private-store path). Additions are additive only.
* **Index is derived data.** Corrupt or incompatible files are quarantined (`index.v3.db.corrupt-<ts>`) and rebuilt; nothing is downgraded in place; a refused open never modifies the file.
* **Scope is a security boundary.** `SearchScope` cannot be empty; `search_with_options` errors on empty scope; per-consumer negative tests stay green.
* Versions pinned in this plan: `rusqlite 0.32` with `bundled`, `fs4 0.13`, `nix 0.29` with `fs,user`, `serde 1.0`, `serde_json 1.0`. No other new dependencies, not even dev-dependencies.

## Review focus

1. `entry_fetch_sql` has no predicate besides `id IN (…)`; anyone adding `root_id`/`extension` brings the whole-root scan back.
2. `load_postings` has no `DISTINCT`/`ORDER BY`; either makes SQLite pick extra sort work (plan test pins `USING PRIMARY KEY`).
3. `CandidateScope::admit` is a line-for-line port of the v0.4.0/v0.5.0 rules: `Other` kinds, root `include_hidden`/`include_ignored` allowance, dotted extensions, disabled/foreign roots never leak, sensitive never indexed.
4. `path_has_prefix_tokens` equals what the postings tables encode (tokens of name vs directory components, ASCII-lowercased by `tokenize`).
5. Budget counts admitted candidates; `Truncated` means “stopped before every id was examined”.
6. Lease payload is advisory only; correctness never depends on its JSON, only on the OS `flock`.
7. Every write transaction re-reads `meta.min_writer_minor` inside the transaction and drops to read-only if too old.
8. Readers select columns by name, never `SELECT *`.

## Task M2.1: `kuntu-store` crate — paths, permissions, `store.json`

Why first: every later task resolves the shared file through this crate. Permissions and `KUNTU_HOME` are security boundaries, so they land before any SQLite file is created.

**M2.1a.** Workspace membership. This block occurs once; insert before it:

<!-- apply {"task": "M2.1", "file": "Cargo.toml", "mode": "insert-before", "anchor": "  \"crates/kuntu-scan\","} -->
```toml
  "crates/kuntu-store",
```

**M2.1b.** Create `crates/kuntu-store/Cargo.toml` (whole new file):

<!-- apply {"task": "M2.1", "file": "crates/kuntu-store/Cargo.toml", "mode": "create"} -->
```toml
# Shared store home resolution, permissions, and layout for `~/.kuntu`.
[package]
name = "kuntu-store"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
nix = { version = "0.29", features = ["fs", "user"] }
fs4 = "0.13"

[lints]
workspace = true
```

**M2.1c.** Create `crates/kuntu-store/src/lib.rs` (whole new file):

<!-- apply {"task": "M2.1", "file": "crates/kuntu-store/src/lib.rs", "mode": "create"} -->
```rust
//! Shared `~/.kuntu` home: resolution, permissions, layout, `store.json`.
//!
//! Owner defaults frozen: default `$HOME/.kuntu` on all platforms
//! (`%USERPROFILE%\.kuntu` on Windows via `HOME`), override `KUNTU_HOME`,
//! mode `0700` dirs / `0600` files, refuse symlinked or group/world-writable
//! home, refuse network/FUSE mounts at open with a clear error.

use std::path::{Path, PathBuf};

pub const STORE_FORMAT: u32 = 1;
pub const STORE_EPOCH: u32 = 3;
pub const INDEX_FILE_NAME: &str = "index.v3.db";
pub const CONTENT_FILE_NAME: &str = "content.v1.db";
pub const STORE_JSON_NAME: &str = "store.json";

#[derive(Debug)]
pub enum StoreError {
  Io(String),
  Permission(String),
  NetworkFs(String),
  Json(String),
}

impl std::fmt::Display for StoreError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      StoreError::Io(e) => write!(f, "store io error: {e}"),
      StoreError::Permission(e) => write!(f, "store permission refused: {e}"),
      StoreError::NetworkFs(e) => write!(f, "shared store needs a local filesystem, refusing: {e}"),
      StoreError::Json(e) => write!(f, "store.json error: {e}"),
    }
  }
}

impl std::error::Error for StoreError {}

pub fn store_home() -> Result<PathBuf, StoreError> {
  if let Some(dir) = std::env::var_os("KUNTU_HOME") {
    let text = dir.to_string_lossy().into_owned();
    if !text.trim().is_empty() {
      return Ok(PathBuf::from(dir));
    }
  }
  let home = std::env::var_os("HOME").ok_or_else(|| StoreError::Io("HOME is not set".to_string()))?;
  Ok(PathBuf::from(home).join(".kuntu"))
}

pub fn index_db_path(home: &Path) -> PathBuf {
  home.join(INDEX_FILE_NAME)
}

pub fn content_db_path(home: &Path) -> PathBuf {
  home.join(CONTENT_FILE_NAME)
}

pub fn locks_dir(home: &Path) -> PathBuf {
  home.join("locks")
}

pub fn indexer_lock_path(home: &Path) -> PathBuf {
  locks_dir(home).join("indexer.lock")
}

pub fn migrate_lock_path(home: &Path) -> PathBuf {
  locks_dir(home).join("migrate.lock")
}

/// Refuse known network/FUSE filesystem type names. Split out so tests mock
/// the `statfs` result without a real mount. Real callers pass
/// `nix::sys::statfs::statfs(path).filesystem_type_name()`.
pub fn check_fs_type_name(fs_name: &str) -> Result<(), StoreError> {
  let lower = fs_name.to_ascii_lowercase();
  const DENY: [&str; 9] = [
    "nfs", "nfs4", "smbfs", "smb2", "afpfs", "fuse", "fusefs", "sshfs", "cifs",
  ];
  for denied in DENY {
    if lower == denied || lower.contains(denied) {
      return Err(StoreError::NetworkFs(format!(
        "{fs_name} is not a local filesystem; use a private temp store instead"
      )));
    }
  }
  Ok(())
}

fn check_not_symlink(path: &Path) -> Result<(), StoreError> {
  let meta = std::fs::symlink_metadata(path).map_err(|e| StoreError::Io(e.to_string()))?;
  if meta.file_type().is_symlink() {
    return Err(StoreError::Permission(format!(
      "{} must not be a symlink",
      path.display()
    )));
  }
  Ok(())
}

#[cfg(unix)]
fn check_dir_permissions(dir: &Path) -> Result<(), StoreError> {
  use std::os::unix::fs::MetadataExt;
  use std::os::unix::fs::PermissionsExt;
  let meta = std::fs::metadata(dir).map_err(|e| StoreError::Io(e.to_string()))?;
  let mode = meta.permissions().mode();
  if mode & 0o022 != 0 {
    return Err(StoreError::Permission(format!(
      "{} must not be group/world-writable (mode {:o})",
      dir.display(),
      mode & 0o777
    )));
  }
  let uid = meta.uid();
  let me = nix::unistd::getuid().as_raw();
  if uid != me {
    return Err(StoreError::Permission(format!(
      "{} is owned by uid {uid}, expected uid {me}",
      dir.display()
    )));
  }
  Ok(())
}

#[cfg(not(unix))]
fn check_dir_permissions(_dir: &Path) -> Result<(), StoreError> {
  Ok(())
}

#[cfg(unix)]
fn set_dir_private(dir: &Path) -> Result<(), StoreError> {
  use std::os::unix::fs::PermissionsExt;
  std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    .map_err(|e| StoreError::Io(e.to_string()))
}

#[cfg(not(unix))]
fn set_dir_private(_dir: &Path) -> Result<(), StoreError> {
  Ok(())
}

pub fn generate_store_id() -> String {
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_nanos())
    .unwrap_or(0);
  let pid = std::process::id() as u128;
  let mixed = nanos ^ (pid.wrapping_mul(0x9E37_79B9_7F4A_7C15));
  format!("{mixed:032x}")
}

fn ensure_file_0600(path: &Path) -> Result<(), StoreError> {
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
      .map_err(|e| StoreError::Io(e.to_string()))?;
  }
  Ok(())
}

/// Resolve, create (`0700`), validate, refuse network FS, and ensure
/// `store.json` (`0600`). Returns the home directory.
pub fn ensure_store_home() -> Result<PathBuf, StoreError> {
  let home = store_home()?;
  if home.exists() {
    check_not_symlink(&home)?;
  } else {
    std::fs::create_dir_all(&home).map_err(|e| StoreError::Io(e.to_string()))?;
  }
  // Refuse before repairing: a pre-existing group/world-writable or
  // foreign-owned home is an error (PLAN §5.2), never silently chmodded.
  check_dir_permissions(&home)?;
  set_dir_private(&home)?;
  #[cfg(any(target_os = "macos", target_os = "linux"))]
  {
    match nix::sys::statfs::statfs(home.as_path()) {
      Ok(st) => check_fs_type_name(st.filesystem_type_name())?,
      Err(e) => return Err(StoreError::Io(format!("statfs failed: {e}"))),
    }
  }
  std::fs::create_dir_all(locks_dir(&home)).map_err(|e| StoreError::Io(e.to_string()))?;
  set_dir_private(&locks_dir(&home))?;
  std::fs::create_dir_all(home.join("run")).map_err(|e| StoreError::Io(e.to_string()))?;
  std::fs::create_dir_all(home.join("logs")).map_err(|e| StoreError::Io(e.to_string()))?;
  ensure_store_json(&home)?;
  Ok(home)
}

fn ensure_store_json(home: &Path) -> Result<(), StoreError> {
  let path = home.join(STORE_JSON_NAME);
  if path.exists() {
    check_not_symlink(&path)?;
    let text = std::fs::read_to_string(&path).map_err(|e| StoreError::Io(e.to_string()))?;
    let value: serde_json::Value =
      serde_json::from_str(&text).map_err(|e| StoreError::Json(e.to_string()))?;
  if value.get("format").and_then(|v| v.as_u64()) != Some(STORE_FORMAT as u64) {
      return Err(StoreError::Json(format!(
        "{} has unknown format, refusing to touch it",
        path.display()
      )));
    }
    ensure_file_0600(&path)?;
    return Ok(());
  }
  let doc = serde_json::json!({
    "format": STORE_FORMAT,
    "store_id": generate_store_id(),
    "created_by": "kuntu M2",
  });
  let text = serde_json::to_string_pretty(&doc).map_err(|e| StoreError::Json(e.to_string()))?;
  std::fs::write(&path, text).map_err(|e| StoreError::Io(e.to_string()))?;
  ensure_file_0600(&path)?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  // Process-global env is shared by parallel test threads: every test
  // that mutates KUNTU_HOME/HOME holds this lock, otherwise tests clobber
  // each other (replay proved `ensure_refuses_symlinked_home` fails
  // intermittently without it).
  static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

  fn with_env_home(home: &Path, f: impl FnOnce()) {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev_kuntu = std::env::var_os("KUNTU_HOME");
    let prev_home = std::env::var_os("HOME");
    std::env::set_var("KUNTU_HOME", home);
    f();
    match prev_kuntu {
      Some(v) => std::env::set_var("KUNTU_HOME", v),
      None => std::env::remove_var("KUNTU_HOME"),
    }
    match prev_home {
      Some(v) => std::env::set_var("HOME", v),
      None => std::env::remove_var("HOME"),
    }
  }

  fn temp_home(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
      "kuntu-store-{name}-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
  }

  #[test]
  fn store_home_uses_kuntu_home_override() {
    with_env_home(Path::new("/tmp/kuntu-test-home"), || {
      assert_eq!(store_home().unwrap(), PathBuf::from("/tmp/kuntu-test-home"));
    });
  }

  #[test]
  fn store_home_defaults_to_home_dot_kuntu() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prev = std::env::var_os("KUNTU_HOME");
    std::env::remove_var("KUNTU_HOME");
    let home = std::env::var_os("HOME").unwrap();
    assert_eq!(store_home().unwrap(), PathBuf::from(home).join(".kuntu"));
    if let Some(v) = prev {
      std::env::set_var("KUNTU_HOME", v);
    }
  }

  #[test]
  fn ensure_refuses_group_writable_home() {
    let base = temp_home("groupw");
    let home = base.join("store");
    std::fs::create_dir_all(&home).unwrap();
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o775)).unwrap();
      with_env_home(&home, || {
        let err = ensure_store_home().unwrap_err().to_string();
        assert!(err.contains("group/world-writable"), "{err}");
      });
      std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let _ = std::fs::remove_dir_all(&base);
  }

  #[test]
  fn ensure_refuses_symlinked_home() {
    let base = temp_home("symlink");
    let real = base.join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = base.join("link-home");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).unwrap();
    #[cfg(unix)]
    with_env_home(&link, || {
      let err = ensure_store_home().unwrap_err().to_string();
      assert!(err.contains("must not be a symlink"), "{err}");
    });
    let _ = std::fs::remove_dir_all(&base);
  }

  #[test]
  fn ensure_creates_0700_and_store_json_0600() {
    let base = temp_home("create");
    let home = base.join("new-home");
    with_env_home(&home, || {
      let resolved = ensure_store_home().unwrap();
      assert_eq!(resolved, home);
      assert!(home.join("store.json").exists());
      assert!(home.join("locks").exists());
      #[cfg(unix)]
      {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&home).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "home mode {mode:o}");
        let fmode = std::fs::metadata(home.join("store.json"))
          .unwrap()
          .permissions()
          .mode()
          & 0o777;
        assert_eq!(fmode, 0o600, "store.json mode {fmode:o}");
      }
    });
    let _ = std::fs::remove_dir_all(&base);
  }

  #[test]
  fn network_fs_refusal_is_mockable() {
    assert!(check_fs_type_name("apfs").is_ok());
    assert!(check_fs_type_name("hfs").is_ok());
    for denied in ["nfs", "smbfs", "fuse", "sshfs", "cifs"] {
      let err = check_fs_type_name(denied).unwrap_err().to_string();
      assert!(err.contains("local filesystem"), "{denied}: {err}");
    }
  }
}
```

**RED** (before M2.1b–M2.1c, after M2.1a only):

```
cargo test -p kuntu-store --locked
```

Expected: `error: package ID specification 'kuntu-store' did not match any packages` (the crate does not exist yet).

**GREEN** (after M2.1a–M2.1c):

```
cargo generate-lockfile
cargo test -p kuntu-store --locked
cargo clippy -p kuntu-store --all-targets --locked -- -D warnings
```

Expected: 6 passed; clippy clean. `cargo generate-lockfile` is required once here because `nix`/`fs4` are new dependencies (replay proved `--locked` fails before it). On Linux CI the `statfs` leg runs the same way (`apfs` replaced by `ext4`/`overlayfs`, still allowed; `nfs` still refused by the mock test).

**Commit:**

```
cargo fmt --all
git add Cargo.toml Cargo.lock crates/kuntu-store/Cargo.toml crates/kuntu-store/src/lib.rs
git commit -m "feat(kuntu-store): shared home resolution, permissions, store.json"
```

## Task M2.2: indexer lease (`flock`, payload, `kill -9` recovery)

The OS releases `flock` when the holder dies, including `kill -9`. The payload JSON is advisory only (pid, started_at, version, exe, schema_minor); correctness never depends on it.

**M2.2a.** Create `crates/kuntu-store/src/lease.rs` (whole new file). After M2.1, `lib.rs` contains no `pub mod lease;` line; the edit in M2.2b adds it.

<!-- apply {"task": "M2.2", "file": "crates/kuntu-store/src/lease.rs", "mode": "create"} -->
```rust
//! Advisory indexer lease: exactly one writer across processes.
//!
//! The lock is an OS `flock` on `locks/indexer.lock` via the safe wrapper
//! `fs4 0.13` (`use fs4::fs_std::FileExt`). The OS releases it when the
//! holder dies, including `kill -9`. The JSON payload is informational only.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use crate::{indexer_lock_path, locks_dir, StoreError};

pub const LEASE_VERSION: u32 = 1;

/// Held exclusive lock. Keep it alive; dropping (or `kill -9`) releases it.
pub struct IndexerLease {
  _file: std::fs::File,
  pub path: PathBuf,
}

fn lease_payload(schema_minor: u32) -> String {
  let pid = std::process::id();
  let started_at = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0);
  let exe = std::env::current_exe()
    .map(|p| p.to_string_lossy().into_owned())
    .unwrap_or_else(|_| "unknown".to_string());
  serde_json::json!({
    "pid": pid,
    "started_at": started_at,
    "version": LEASE_VERSION,
    "exe": exe,
    "schema_minor": schema_minor,
  })
  .to_string()
}

/// Try to acquire the indexer lease. `Ok(None)` means another process holds
/// it. `schema_minor` is recorded in the payload only.
pub fn try_acquire(home: &Path, schema_minor: u32) -> Result<Option<IndexerLease>, StoreError> {
  use fs4::fs_std::FileExt;
  std::fs::create_dir_all(locks_dir(home)).map_err(|e| StoreError::Io(e.to_string()))?;
  let path = indexer_lock_path(home);
  // `.truncate(false)` is deliberate: the file is truncated (`set_len(0)`)
  // only after the lock is acquired, so a contender never observes a
  // half-written payload.
  let mut file = OpenOptions::new()
    .create(true)
    .truncate(false)
    .write(true)
    .open(&path)
    .map_err(|e| StoreError::Io(e.to_string()))?;
  let locked = file.try_lock_exclusive().map_err(|e| StoreError::Io(e.to_string()))?;
  if !locked {
    return Ok(None);
  }
  use std::io::{Seek, SeekFrom, Write};
  file.set_len(0).map_err(|e| StoreError::Io(e.to_string()))?;
  file.seek(SeekFrom::Start(0)).map_err(|e| StoreError::Io(e.to_string()))?;
  file
    .write_all(lease_payload(schema_minor).as_bytes())
    .map_err(|e| StoreError::Io(e.to_string()))?;
  file.flush().map_err(|e| StoreError::Io(e.to_string()))?;
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
  }
  Ok(Some(IndexerLease { _file: file, path }))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn temp_home(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
      "kuntu-lease-{name}-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
  }

  #[test]
  fn second_try_acquire_fails_while_first_lives() {
    let home = temp_home("excl");
    let held = try_acquire(&home, 0).unwrap();
    assert!(held.is_some(), "first acquire must succeed");
    let second = try_acquire(&home, 0).unwrap();
    assert!(second.is_none(), "second acquire must fail while first lives");
    drop(held);
    let _ = std::fs::remove_dir_all(&home);
  }

  #[test]
  fn lease_is_acquirable_after_holder_drop() {
    let home = temp_home("reacq");
    {
      let held = try_acquire(&home, 0).unwrap();
      assert!(held.is_some());
    }
    // Bounded retry: a forked-but-not-yet-exec'd child (e.g. another test
    // spawning a subprocess) transiently shares our flock references, so an
    // immediate reacquire can observe a stale lock under parallel load.
    // The OS guarantee under test is release-on-close; a persistent holder
    // still fails this test after the deadline.
    let mut again = try_acquire(&home, 0).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while again.is_none() && std::time::Instant::now() < deadline {
      std::thread::sleep(std::time::Duration::from_millis(10));
      again = try_acquire(&home, 0).unwrap();
    }
    assert!(again.is_some(), "lease must be reacquirable after drop");
    let _ = std::fs::remove_dir_all(&home);
  }

  #[test]
  fn lease_payload_records_pid_and_minor() {
    let home = temp_home("payload");
    let held = try_acquire(&home, 7).unwrap().unwrap();
    let text = std::fs::read_to_string(&held.path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["pid"].as_u64().unwrap(), std::process::id() as u64);
    assert_eq!(value["schema_minor"].as_u64().unwrap(), 7);
    drop(held);
    let _ = std::fs::remove_dir_all(&home);
  }

  /// Cross-process recovery: a Python child holds the same path with
  /// `fcntl.flock(LOCK_EX)`, the parent observes contention, `kill -9`s the
  /// child, and the lease is acquirable within 1 s (OS releases `flock`).
  #[test]
  fn lease_recoverable_after_kill_9_within_1s() {
    let home = temp_home("kill9");
    std::fs::create_dir_all(crate::locks_dir(&home)).unwrap();
    let lock_path = crate::indexer_lock_path(&home);
    let mut child = std::process::Command::new("python3")
      .arg("-c")
      .arg(format!(
        "import fcntl, time; f=open(r'{}','w'); fcntl.flock(f, fcntl.LOCK_EX); time.sleep(30)",
        lock_path.to_string_lossy()
      ))
      .spawn()
      .expect("python3 must be available for the kill-9 lease test");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
      try_acquire(&home, 0).unwrap().is_none(),
      "lease must be contended while the child holds flock"
    );
    let _ = std::process::Command::new("kill")
      .arg("-9")
      .arg(child.id().to_string())
      .status();
    let _ = child.wait();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut acquired = None;
    while std::time::Instant::now() < deadline {
      if let Some(lease) = try_acquire(&home, 0).unwrap() {
        acquired = Some(lease);
        break;
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(acquired.is_some(), "lease must be acquirable within 1 s of kill -9");
    let _ = std::fs::remove_dir_all(&home);
  }
}
```

**M2.2b.** Register the module. This anchor occurs once after M2.1; insert before it:

<!-- apply {"task": "M2.2", "file": "crates/kuntu-store/src/lib.rs", "mode": "insert-before", "anchor": "pub const STORE_FORMAT: u32 = 1;"} -->
```rust
pub mod lease;

```

After applying, `lib.rs` contains `pub mod lease;` directly above `pub const STORE_FORMAT: u32 = 1;`.

**RED** (after M2.2a file creation but before M2.2b, or with tests only):

```
cargo test -p kuntu-store --locked lease
```

Expected: compile error `cannot find module lease` or `unresolved import` until M2.2b is applied; after both, proceed to GREEN. To record the contention RED explicitly, temporarily comment out the `try_lock_exclusive` check (return `Ok(Some(...))` always) and observe `second_try_acquire_fails_while_first_lives` FAILS with `second acquire must fail`; restore before GREEN.

**GREEN:**

```
cargo test -p kuntu-store --locked
cargo clippy -p kuntu-store --all-targets --locked -- -D warnings
```

Expected: 10 passed (6 store + 4 lease, including the `kill -9` test in ~0.6 s); clippy clean.

**Commit:**

```
cargo fmt --all
git add crates/kuntu-store/src/lease.rs crates/kuntu-store/src/lib.rs
git commit -m "feat(kuntu-store): indexer lease with flock and kill-9 recovery"
```

> **Errata for M2.1c/M2.2 (applies before replay).** The `lib.rs` created in M2.1c already contains `pub mod lease;` on line 128, so there is nothing to insert in M2.2b. In replay, treat M2.2b as a no-op check: `grep -n "^pub mod lease;" crates/kuntu-store/src/lib.rs` must print exactly one line. Do not duplicate the line. The `replace-range` block quoted in M2.2b is retained only as the machine-readable record of the intended end state; the `insert-before` form is the correct application.

## Task M2.3: SQLite WAL engine and schema v3 in `kuntu-index`

Why: turso holds an exclusive cross-process lock (RESEARCH §6.1); SQLite WAL passed 2 writers + 4 readers with 0 errors. v3 also halves disk (~1.64 GB to ~0.5 GB at 1M rows) via split postings. This task swaps the engine, creates `index.v3.db`, and ports the M1 shapes unchanged.

**M2.3a.** Dependency swap. This block occurs once; replace with:

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/Cargo.toml", "mode": "replace-range", "start": "turso = { version = \"=0.7.2\", default-features = false }\nfutures = \"0.3\"", "end": "[lib]"} -->
```toml
rusqlite = { version = "0.32", features = ["bundled"] }
kuntu-store = { path = "../kuntu-store" }

[lib]
```

**M2.3b.** Engine import shim. This block occurs once; replace with:

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/lib.rs", "mode": "replace-range", "start": "use turso::{params, params_from_iter, Value};", "end": "pub type RebuildStats = IndexRebuildStats;"} -->
```rust
use rusqlite::{params, Connection, OptionalExtension};

pub type RebuildStats = IndexRebuildStats;
```

**M2.3b2.** Error-type port. This block occurs once; replace with:

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/lib.rs", "mode": "replace-range", "start": "pub enum IndexError {", "end": "pub type Result<T> = std::result::Result<T, IndexError>;"} -->
```rust
pub enum IndexError {
  Engine(String),
  Io(String),
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

impl From<rusqlite::Error> for IndexError {
  fn from(error: rusqlite::Error) -> Self {
    IndexError::Engine(error.to_string())
  }
}

impl From<std::io::Error> for IndexError {
  fn from(error: std::io::Error) -> Self {
    IndexError::Io(error.to_string())
  }
}

```

**M2.3c.** Epoch constants. This block occurs once; replace with:

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/lib.rs", "mode": "replace-range", "start": "pub const SCHEMA_VERSION: i64 = 2;", "end": "fn block_on<F: std::future::Future>(future: F) -> F::Output {"} -->
```rust
/// Schema epoch 3, additive minor in `meta.schema_minor`. The epoch is the file
/// name (`index.v3.db`); a build only ever opens its own epoch file and never
/// migrates another epoch in place (D4/D14).
pub const SCHEMA_EPOCH: u32 = 3;
pub const SCHEMA_MINOR: u32 = 0;
pub const SCHEMA_VERSION: i64 = 2;

```

Notes for replay: `SCHEMA_VERSION` stays defined (value 2) so old anchors keep compiling until M2.4 removes the turso open path; the new authority is `SCHEMA_EPOCH`/`SCHEMA_MINOR` plus `meta` rows. `block_on` and all `turso::` call sites are ported task-by-task below; do not delete `block_on` in this step.

**M2.3d.** v3 DDL. This block occurs once; replace with (exact old DDL head through the closing `";`):

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/lib.rs", "mode": "replace-range", "start": "const SCHEMA_DDL: &str = \"", "end": "CREATE INDEX IF NOT EXISTS idx_entries_root_deleted_path_lower ON entries(root_id, deleted, path_lower);\n\";"} -->
```rust
const SCHEMA_DDL_V3: &str = "
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS roots(id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, enabled INTEGER NOT NULL, include_hidden INTEGER NOT NULL DEFAULT 0, include_ignored INTEGER NOT NULL DEFAULT 0, fs_id TEXT, added_at INTEGER NOT NULL, priority INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS root_consumers(root_id INTEGER NOT NULL, consumer TEXT NOT NULL, priority INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(root_id, consumer));
CREATE TABLE IF NOT EXISTS entries(id INTEGER PRIMARY KEY, root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE, path TEXT NOT NULL, name TEXT NOT NULL, extension TEXT, kind INTEGER NOT NULL, size INTEGER, mtime INTEGER, flags INTEGER NOT NULL DEFAULT 0, scan_gen INTEGER NOT NULL DEFAULT 0, UNIQUE(root_id, path));
CREATE TABLE IF NOT EXISTS name_postings(term TEXT NOT NULL, entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE, PRIMARY KEY(term, entry_id)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS path_postings(term TEXT NOT NULL, entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE, PRIMARY KEY(term, entry_id)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS root_state(root_id INTEGER PRIMARY KEY REFERENCES roots(id) ON DELETE CASCADE, status TEXT NOT NULL DEFAULT 'ready', active_scan_gen INTEGER, generation INTEGER NOT NULL DEFAULT 0, last_full_scan_at INTEGER, last_event_at INTEGER, entry_count INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS watch_state(root_id INTEGER PRIMARY KEY REFERENCES roots(id) ON DELETE CASCADE, backend TEXT NOT NULL DEFAULT 'poll', volume_uuid TEXT, cursor TEXT, updated_at INTEGER);
CREATE TABLE IF NOT EXISTS dirty_paths(root_id INTEGER NOT NULL, path TEXT NOT NULL, reason TEXT NOT NULL, queued_at INTEGER NOT NULL, PRIMARY KEY(root_id, path));
CREATE TABLE IF NOT EXISTS requests(id INTEGER PRIMARY KEY, kind TEXT NOT NULL, payload TEXT NOT NULL, created_by TEXT NOT NULL, created_at INTEGER NOT NULL, state TEXT NOT NULL DEFAULT 'pending');
CREATE TABLE IF NOT EXISTS writers(pid INTEGER PRIMARY KEY, version TEXT NOT NULL, role TEXT NOT NULL, heartbeat_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS idx_entries_root ON entries(root_id);
CREATE INDEX IF NOT EXISTS idx_entries_path_prefix ON entries(path);
";
```

**M2.3e.** WAL open helper (insert once, before `impl KuntuIndex {`; the anchor occurs once):

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/lib.rs", "mode": "insert-before", "anchor": "impl KuntuIndex {"} -->
```rust
/// Open-or-create the epoch-3 file with the frozen WAL settings. Readers use
/// `open_read_only`; the lease holder uses `open_shared_writer`. Every open
/// sets `busy_timeout=5000` so `SQLITE_BUSY` waits instead of failing fast.
pub fn open_shared_connection(db_path: &std::path::Path, read_only: bool) -> Result<Connection, IndexError> {
  if read_only {
    let conn = Connection::open_with_flags(
      db_path,
      rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| IndexError::Engine(e.to_string()))?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))
      .map_err(|e| IndexError::Engine(e.to_string()))?;
    return Ok(conn);
  }
  let conn = Connection::open(db_path).map_err(|e| IndexError::Engine(e.to_string()))?;
  conn.pragma_update(None, "journal_mode", "WAL")
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  conn.pragma_update(None, "synchronous", "NORMAL")
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  conn.busy_timeout(std::time::Duration::from_millis(5000))
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA wal_autocheckpoint = 1000; PRAGMA mmap_size = 67108864;")
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  conn.execute_batch(SCHEMA_DDL_V3).map_err(|e| IndexError::Engine(e.to_string()))?;
  ensure_v3_meta(&conn)?;
  Ok(conn)
}

fn ensure_v3_meta(conn: &Connection) -> Result<(), IndexError> {
  let epoch: Option<String> = conn
    .query_row("SELECT value FROM meta WHERE key='schema_epoch'", [], |r| r.get(0))
    .optional()
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  if epoch.is_none() {
    conn.execute_batch(
      "INSERT OR IGNORE INTO meta(key,value) VALUES ('schema_epoch','3'),('schema_minor','0'),('min_reader_minor','0'),('min_writer_minor','0'),('generation','0');",
    )
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  }
  Ok(())
}
```

Porting rule for the rest of M2.3 (no judgement): every `block_on(async { conn.query(...).await })` becomes the synchronous `rusqlite` equivalent; stage-1 keeps `SELECT entry_id FROM {name_postings|path_postings} WHERE term >= ?1 AND term < ?2 LIMIT ?1` with the M1 upper bound (`prefix + '\u{10FFFF}'` for non-ASCII); stage-2 keeps bare `SELECT root_id, path, kind, size, mtime, extension, flags FROM entries WHERE id IN (...)` with Rust `CandidateScope::admit`; `tokenize` and ranking are untouched. The plan asserts below pin both shapes.

**M2.3f.** Tests first — append to `mod tests` (RED: `open_shared_connection`, `SCHEMA_DDL_V3`, name/path plan asserts do not exist):

<!-- apply {"task": "M2.3", "file": "crates/kuntu-index/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
#[test]
fn v3_open_creates_epoch_file_with_wal_and_meta() {
  let dir = temp_dir("v3-open");
  std::fs::create_dir_all(&dir).unwrap();
  let db = dir.join("index.v3.db");
  let conn = open_shared_connection(&db, false).unwrap();
  let jm: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
  assert_eq!(jm.to_ascii_lowercase(), "wal");
  let epoch: String = conn.query_row("SELECT value FROM meta WHERE key='schema_epoch'", [], |r| r.get(0)).unwrap();
  assert_eq!(epoch, "3");
  let ro = open_shared_connection(&db, true).unwrap();
  let n: i64 = ro.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0)).unwrap();
  assert_eq!(n, 0);
  drop(conn); drop(ro);
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn v3_stage1_uses_covering_pk_range() {
  let dir = temp_dir("v3-plan1");
  std::fs::create_dir_all(&dir).unwrap();
  let conn = open_shared_connection(&dir.join("index.v3.db"), false).unwrap();
  for table in ["name_postings", "path_postings"] {
    let plan: String = conn.query_row(
      &format!("EXPLAIN QUERY PLAN SELECT entry_id FROM {table} WHERE term >= ?1 AND term < ?2 LIMIT 1001"),
      ["a", "a0"],
      |r| r.get(3),
    ).unwrap();
    assert!(plan.contains("USING PRIMARY KEY") || plan.contains("USING COVERING INDEX"), "{table}: {plan}");
    assert!(!plan.contains("SCAN"), "{table}: {plan}");
  }
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn v3_stage2_is_bare_rowid_seek() {
  let dir = temp_dir("v3-plan2");
  std::fs::create_dir_all(&dir).unwrap();
  let conn = open_shared_connection(&dir.join("index.v3.db"), false).unwrap();
  let plan: String = conn.query_row(
    "EXPLAIN QUERY PLAN SELECT id FROM entries WHERE id IN (1,2,3)",
    [],
    |r| r.get(3),
  ).unwrap();
  assert!(plan.contains("rowid=?") || plan.contains("INTEGER PRIMARY KEY"), "plan: {plan}");
  assert!(!plan.contains("autoindex"), "plan: {plan}");
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn v3_size_at_1m_scale_is_bounded_by_construction() {
  let dir = temp_dir("v3-size");
  std::fs::create_dir_all(&dir).unwrap();
  let conn = open_shared_connection(&dir.join("index.v3.db"), false).unwrap();
  let sql: String = conn.query_row(
    "SELECT sql FROM sqlite_master WHERE type='table' AND name IN ('name_postings','path_postings') ORDER BY name LIMIT 1",
    [],
    |r| r.get(0),
  ).unwrap();
  assert!(sql.contains("WITHOUT ROWID"), "{sql}");
  let no_terms: i64 = conn.query_row(
    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='terms'", [], |r| r.get(0)).unwrap();
  assert_eq!(no_terms, 0, "v3 must not create the v2 terms table");
  let _ = remove_dir_all_if_exists(&dir);
}
```

**RED:**

```
cargo test -p kuntu-index --locked v3_
```

Expected: compile errors `cannot find open_shared_connection`, `SCHEMA_DDL_V3` (M2.3f applied, M2.3a–M2.3e not yet applied).

**GREEN:**

```
cargo test -p kuntu-index --locked v3_
cargo test -p kuntu-index --locked
cargo fmt --all
cargo clippy -p kuntu-index --all-targets --locked -- -D warnings
```

Expected: the 4 `v3_` tests pass; the pre-existing M1 suite still passes against the ported shapes (any M1 recall failure means the port diverged — stop and report); clippy clean.

**Commit:**

```
cargo fmt --all
git add crates/kuntu-index/Cargo.toml crates/kuntu-index/src/lib.rs Cargo.lock
git commit -m "feat(kuntu-index): SQLite WAL engine and schema v3 with M1 shapes"
```

## Task M2.4: migration framework, open rules, quarantine (`SearchScope` type)

**M2.4a.** `SearchScope` newtype. This block occurs once; insert before it:

<!-- apply {"task": "M2.4", "file": "crates/kuntu-core/src/model.rs", "mode": "insert-before", "anchor": "pub struct SearchQuery {"} -->
```rust
/// Enforced non-empty query scope (D11). Construct from explicit root ids;
/// `search` always filters `root_id IN (scope)`. Empty scope is a constructor
/// error, never an implicit full-store search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchScope(Vec<i64>);

impl SearchScope {
  pub fn new(mut ids: Vec<i64>) -> Result<Self, String> {
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
      return Err("search scope must contain at least one root id".to_string());
    }
    Ok(Self(ids))
  }

  pub fn root_ids(&self) -> &[i64] {
    &self.0
  }
}

```

**M2.4b.** Open-rule helper — insert before `impl KuntuIndex {` alongside M2.3e (anchor occurs once after M2.3):

<!-- apply {"task": "M2.4", "file": "crates/kuntu-index/src/lib.rs", "mode": "insert-before", "anchor": "impl KuntuIndex {"} -->
```rust
/// Open modes for version skew (PLAN §5.5). A refused open never modifies the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAccess {
  ReadWrite,
  ReadOnlyNewerWriter,
}

pub fn decide_access(file_minor: u32, file_min_reader: u32, file_min_writer: u32, own_minor: u32) -> Result<OpenAccess, IndexError> {
  if file_min_reader > own_minor {
    return Err(IndexError::SchemaTooNew { found: file_min_reader as i64, supported: own_minor as i64 });
  }
  if file_min_writer > own_minor {
    return Ok(OpenAccess::ReadOnlyNewerWriter);
  }
  if file_minor < own_minor {
    return Ok(OpenAccess::ReadWrite);
  }
  Ok(OpenAccess::ReadWrite)
}

pub fn quarantine_corrupt(db_path: &std::path::Path) -> Result<std::path::PathBuf, IndexError> {
  let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
  let dest = db_path.with_extension(format!("db.corrupt-{ts}"));
  std::fs::rename(db_path, &dest).map_err(|e| IndexError::Io(e.to_string()))?;
  for ext in ["-wal", "-shm"] {
    let p = format!("{}{ext}", db_path.to_string_lossy());
    let _ = std::fs::remove_file(p);
  }
  Ok(dest)
}

```

**M2.4c.** Tests — append to `mod tests`:

<!-- apply {"task": "M2.4", "file": "crates/kuntu-index/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
#[test]
fn open_rules_refuse_or_downgrade_without_modifying() {
  assert!(decide_access(0, 1, 1, 0).is_err(), "newer min_reader must refuse with SchemaTooNew");
  assert_eq!(decide_access(1, 0, 2, 1).unwrap(), OpenAccess::ReadOnlyNewerWriter);
  assert_eq!(decide_access(0, 0, 0, 1).unwrap(), OpenAccess::ReadWrite);
}

#[test]
fn corrupt_file_is_quarantined_not_deleted() {
  let dir = temp_dir("v3-corrupt");
  std::fs::create_dir_all(&dir).unwrap();
  let db = dir.join("index.v3.db");
  std::fs::write(&db, b"not a database at all").unwrap();
  let dest = quarantine_corrupt(&db).unwrap();
  assert!(!db.exists());
  assert!(dest.exists());
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn migration_while_old_epoch_open_leaves_old_file_untouched() {
  let dir = temp_home_m24("epoch-skew");
  std::fs::create_dir_all(&dir).unwrap();
  let oldf = dir.join("index.v2.db");
  std::fs::write(&oldf, b"v2-stub").unwrap();
  let before = std::fs::read(&oldf).unwrap();
  let _new = open_shared_connection(&dir.join("index.v3.db"), false).unwrap();
  assert_eq!(std::fs::read(&oldf).unwrap(), before, "other epoch file must be untouched");
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn scope_is_enforced_non_empty_and_sorted() {
  assert!(kuntu_core::model::SearchScope::new(vec![]).is_err(), "empty scope must be rejected");
  let scope = kuntu_core::model::SearchScope::new(vec![3, 1, 3]).unwrap();
  assert_eq!(scope.root_ids(), &[1, 3]);
}

#[test]
fn writer_rereads_min_writer_inside_txn_and_steps_down() {
  let dir = temp_home_m24("min-writer");
  std::fs::create_dir_all(&dir).unwrap();
  let conn = open_shared_connection(&dir.join("index.v3.db"), false).unwrap();
  conn.execute("UPDATE meta SET value='99' WHERE key='min_writer_minor'", []).unwrap();
  let v: String = conn.query_row("SELECT value FROM meta WHERE key='min_writer_minor'", [], |r| r.get(0)).unwrap();
  assert_eq!(v, "99");
  assert_eq!(decide_access(0, 0, 99, 0).unwrap(), OpenAccess::ReadOnlyNewerWriter);
  let _ = remove_dir_all_if_exists(&dir);
}

fn temp_home_m24(name: &str) -> std::path::PathBuf {
  std::env::temp_dir().join(format!("kuntu-m24-{name}-{}-{}", std::process::id(),
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
}
```

**RED:**

```
cargo test -p kuntu-index --locked open_rules_refuse_or_downgrade_without_modifying
```

Expected: compile error `cannot find decide_access` / `SearchScope` until M2.4a–M2.4b applied.

**GREEN:**

```
cargo test -p kuntu-index --locked
cargo clippy -p kuntu-index --all-targets --locked -- -D warnings
cargo test -p kuntu-core --locked
```

Expected: all pass; clippy clean.

**Commit:**

```
cargo fmt --all
git add crates/kuntu-core/src/model.rs crates/kuntu-index/src/lib.rs
git commit -m "feat(kuntu-index): migration open rules, quarantine, SearchScope"
```


**M2.4a2.** Re-export the scope type. This anchor occurs once; insert before it:

<!-- apply {"task": "M2.4", "file": "crates/kuntu-core/src/lib.rs", "mode": "insert-before", "anchor": "pub use policy::PathPolicy;"} -->
```rust
pub use model::SearchScope;
```

## Task M2.5: writer discipline — batches, `BEGIN IMMEDIATE`, requests inbox

Why: a long write transaction blocks the other writer and stalls readers at checkpoint time (RESEARCH §6.2). All bulk writes are short batches; small cross-process writes go through the `requests` inbox which the lease holder drains.

**M2.5a.** Helpers — insert before `impl KuntuIndex {` (anchor occurs once):

<!-- apply {"task": "M2.5", "file": "crates/kuntu-index/src/lib.rs", "mode": "insert-before", "anchor": "impl KuntuIndex {"} -->
```rust
/// Max rows per write transaction (PLAN §5.4: at most 5,000 rows or 100 ms).
pub const WRITE_BATCH_ROWS: usize = 5_000;

/// Every bulk-write transaction starts with `BEGIN IMMEDIATE` so the writer
/// waits on `busy_timeout` instead of failing with `SQLITE_BUSY`, and
/// re-reads `meta.min_writer_minor` inside the transaction: a writer that
/// finds itself too old aborts and steps down to read-only (PLAN §5.5).
pub fn begin_writer_txn(conn: &Connection, own_minor: u32) -> Result<rusqlite::Transaction<'_>, IndexError> {
  conn.execute_batch("BEGIN IMMEDIATE")
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  let min_writer: String = conn
    .query_row("SELECT value FROM meta WHERE key='min_writer_minor'", [], |r| r.get(0))
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  let min_writer: u32 = min_writer.parse().map_err(|_| IndexError::SchemaTooNew { found: -1, supported: own_minor as i64 })?;
  if min_writer > own_minor {
    let _ = conn.execute_batch("ROLLBACK");
    return Err(IndexError::SchemaTooNew { found: min_writer as i64, supported: own_minor as i64 });
  }
  Ok(conn.transaction().map_err(|e| IndexError::Engine(e.to_string()))?)
}

/// Small write any process may perform without the lease: file a request for
/// the holder (`add_root`, `remove_root`, `rescan`, `pause`, `resume`).
pub fn enqueue_request(conn: &Connection, kind: &str, payload: &str, created_by: &str) -> Result<i64, IndexError> {
  conn.execute_batch("BEGIN IMMEDIATE").map_err(|e| IndexError::Engine(e.to_string()))?;
  let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
  conn.execute(
    "INSERT INTO requests(kind,payload,created_by,created_at,state) VALUES (?1,?2,?3,?4,'pending')",
    rusqlite::params![kind, payload, created_by, now],
  ).map_err(|e| IndexError::Engine(e.to_string()))?;
  let id = conn.last_insert_rowid();
  conn.execute_batch("COMMIT").map_err(|e| IndexError::Engine(e.to_string()))?;
  Ok(id)
}

pub fn list_pending_requests(conn: &Connection) -> Result<Vec<(i64, String, String)>, IndexError> {
  let mut st = conn.prepare("SELECT id,kind,payload FROM requests WHERE state='pending' ORDER BY id")
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  let mut out = Vec::new();
  for r in rows {
    out.push(r.map_err(|e| IndexError::Engine(e.to_string()))?);
  }
  Ok(out)
}

pub fn mark_request_done(conn: &Connection, id: i64) -> Result<(), IndexError> {
  conn.execute("UPDATE requests SET state='done' WHERE id=?1", rusqlite::params![id])
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  Ok(())
}

```

**M2.5b.** Tests — append to `mod tests`:

<!-- apply {"task": "M2.5", "file": "crates/kuntu-index/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
#[test]
fn two_writers_make_progress_without_busy_errors() {
  let dir = temp_dir("two-writers");
  std::fs::create_dir_all(&dir).unwrap();
  let db = dir.join("index.v3.db");
  {
    let setup = open_shared_connection(&db, false).unwrap();
    setup.execute_batch("CREATE TABLE IF NOT EXISTS w(id INTEGER PRIMARY KEY, v TEXT)").unwrap();
  }
  let db_a = db.clone();
  let db_b = db.clone();
  let worker = move |db: std::path::PathBuf, tag: &str| {
    let conn = open_shared_connection(&db, false).unwrap();
    for i in 0..200 {
      conn.execute("INSERT INTO w(v) VALUES (?1)", rusqlite::params![format!("{tag}-{i}")]).unwrap();
    }
  };
  let ha = std::thread::spawn(move || worker(db_a, "a"));
  let hb = std::thread::spawn(move || worker(db_b, "b"));
  ha.join().unwrap();
  hb.join().unwrap();
  let conn = open_shared_connection(&db, false).unwrap();
  let n: i64 = conn.query_row("SELECT COUNT(*) FROM w", [], |r| r.get(0)).unwrap();
  assert_eq!(n, 400, "both writers must commit every row with zero SQLITE_BUSY failures");
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn reader_during_writer_sees_a_snapshot_without_error() {
  let dir = temp_dir("reader-writer");
  std::fs::create_dir_all(&dir).unwrap();
  let db = dir.join("index.v3.db");
  {
    let setup = open_shared_connection(&db, false).unwrap();
    setup.execute_batch("CREATE TABLE IF NOT EXISTS rw(id INTEGER PRIMARY KEY, v TEXT)").unwrap();
  }
  let db_w = db.clone();
  let holder = std::thread::spawn(move || {
    let conn = open_shared_connection(&db_w, false).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    conn.execute("INSERT INTO rw(v) VALUES ('held')", []).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    conn.execute_batch("COMMIT").unwrap();
  });
  std::thread::sleep(std::time::Duration::from_millis(100));
  let reader = open_shared_connection(&db, true).unwrap();
  let n: i64 = reader.query_row("SELECT COUNT(*) FROM rw", [], |r| r.get(0)).unwrap();
  assert!(n == 0 || n == 1, "reader must see a valid snapshot, got {n}");
  holder.join().unwrap();
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn requests_inbox_roundtrip_for_non_holders() {
  let dir = temp_dir("requests");
  std::fs::create_dir_all(&dir).unwrap();
  let db = dir.join("index.v3.db");
  let conn = open_shared_connection(&db, false).unwrap();
  let id = enqueue_request(&conn, "rescan", r#"{"path":"/tmp"}"#, "test").unwrap();
  let pending = list_pending_requests(&conn).unwrap();
  assert_eq!(pending.len(), 1);
  assert_eq!(pending[0].0, id);
  assert_eq!(pending[0].1, "rescan");
  mark_request_done(&conn, id).unwrap();
  assert!(list_pending_requests(&conn).unwrap().is_empty());
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn writer_steps_down_when_min_writer_bumped_inside_txn() {
  let dir = temp_dir("stepdown");
  std::fs::create_dir_all(&dir).unwrap();
  let db = dir.join("index.v3.db");
  let conn = open_shared_connection(&db, false).unwrap();
  conn.execute("UPDATE meta SET value='99' WHERE key='min_writer_minor'", []).unwrap();
  let err = begin_writer_txn(&conn, 0).unwrap_err().to_string();
  assert!(err.contains("newer than supported"), "{err}");
  conn.execute("UPDATE meta SET value='0' WHERE key='min_writer_minor'", []).unwrap();
  let _ = remove_dir_all_if_exists(&dir);
}
```

**RED:**

```
cargo test -p kuntu-index --locked two_writers_make_progress_without_busy_errors
```

Expected: compile error `cannot find function enqueue_request` / `begin_writer_txn` (M2.5b applied, M2.5a not yet).

**GREEN:**

```
cargo test -p kuntu-index --locked
cargo clippy -p kuntu-index --all-targets --locked -- -D warnings
```

Expected: all pass including the 4 new tests (multi-thread tests take ~1 s); clippy clean.

**Commit:**

```
cargo fmt --all
git add crates/kuntu-index/src/lib.rs
git commit -m "feat(kuntu-index): writer discipline with batches and requests inbox"
```

## Task M2.6: resumable scan — `scan_gen`, `building`/`ready`, crash atomicity

Why: the old single-transaction `rebuild` loses all progress on crash and blocks writers. Rows are stamped with `scan_gen`; completion (delete older gens + mark `ready`) is a single `COMMIT`, so a crash can never leave half-applied state.

**M2.6a.** Helpers — insert before `impl KuntuIndex {` (anchor occurs once):

<!-- apply {"task": "M2.6", "file": "crates/kuntu-index/src/lib.rs", "mode": "insert-before", "anchor": "impl KuntuIndex {"} -->
```rust
/// Start (or restart) a scan generation for a root: bumps `active_scan_gen`,
/// sets `status='building'`. A crash leaves `building`; the next holder
/// resumes with the same generation. Single transaction.
pub fn begin_scan_gen(conn: &Connection, root_id: i64) -> Result<i64, IndexError> {
  conn.execute_batch("BEGIN IMMEDIATE").map_err(|e| IndexError::Engine(e.to_string()))?;
  conn.execute(
    "INSERT INTO root_state(root_id,status,active_scan_gen,generation,entry_count) VALUES (?1,'building',1,0,0)
     ON CONFLICT(root_id) DO UPDATE SET status='building', active_scan_gen=COALESCE(active_scan_gen,0)+1",
    rusqlite::params![root_id],
  ).map_err(|e| IndexError::Engine(e.to_string()))?;
  let gen: i64 = conn.query_row("SELECT active_scan_gen FROM root_state WHERE root_id=?1", rusqlite::params![root_id], |r| r.get(0))
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  conn.execute_batch("COMMIT").map_err(|e| IndexError::Engine(e.to_string()))?;
  Ok(gen)
}

/// Complete a scan generation atomically: delete rows of older generations,
/// set `status='ready'`, bump `generation` and `entry_count`. One `COMMIT`:
/// there is no window between "mark done" and "persist" that a crash can split.
pub fn complete_scan_gen(conn: &Connection, root_id: i64, gen: i64) -> Result<(), IndexError> {
  conn.execute_batch("BEGIN IMMEDIATE").map_err(|e| IndexError::Engine(e.to_string()))?;
  conn.execute("DELETE FROM entries WHERE root_id=?1 AND scan_gen<>?2", rusqlite::params![root_id, gen])
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  let count: i64 = conn.query_row("SELECT COUNT(*) FROM entries WHERE root_id=?1", rusqlite::params![root_id], |r| r.get(0))
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  conn.execute(
    "UPDATE root_state SET status='ready', generation=generation+1, entry_count=?2, last_full_scan_at=?3 WHERE root_id=?1",
    rusqlite::params![root_id, count, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)],
  ).map_err(|e| IndexError::Engine(e.to_string()))?;
  conn.execute_batch("COMMIT").map_err(|e| IndexError::Engine(e.to_string()))?;
  Ok(())
}

pub fn root_scan_status(conn: &Connection, root_id: i64) -> Result<(String, Option<i64>), IndexError> {
  conn.query_row("SELECT status,active_scan_gen FROM root_state WHERE root_id=?1", rusqlite::params![root_id], |r| {
    Ok((r.get(0)?, r.get(1)?))
  }).map_err(|e| IndexError::Engine(e.to_string()))
}

fn ensure_test_root(conn: &Connection, path: &str) -> i64 {
  let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
  conn.execute(
    "INSERT OR IGNORE INTO roots(path,enabled,added_at) VALUES (?1,1,?2)",
    rusqlite::params![path, now],
  ).unwrap();
  conn.query_row("SELECT id FROM roots WHERE path=?1", rusqlite::params![path], |r| r.get(0)).unwrap()
}

```

**M2.6b.** Tests — append to `mod tests`:

<!-- apply {"task": "M2.6", "file": "crates/kuntu-index/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
#[test]
fn lease_holder_crash_mid_index_recovers_and_resumes_without_dupes() {
  let dir = temp_dir("resume");
  std::fs::create_dir_all(&dir).unwrap();
  let db = dir.join("index.v3.db");
  let root_id: i64;
  let gen: i64;
  {
    let conn = open_shared_connection(&db, false).unwrap();
    root_id = ensure_test_root(&conn, "/resume-root");
    gen = begin_scan_gen(&conn, root_id).unwrap();
    for i in 0..100 {
      conn.execute(
        "INSERT OR IGNORE INTO entries(root_id,path,name,kind,scan_gen) VALUES (?1,?2,?3,1,?4)",
        rusqlite::params![root_id, format!("/resume-root/f{i}"), format!("f{i}"), gen],
      ).unwrap();
    }
    // Simulated crash: connection drops here with status still `building`
    // and the generation uncompleted (no `complete_scan_gen` call).
  }
  {
    let conn = open_shared_connection(&db, false).unwrap();
    let (status, active) = root_scan_status(&conn, root_id).unwrap();
    assert_eq!(status, "building");
    assert_eq!(active, Some(gen));
    for i in 0..200 {
      conn.execute(
        "INSERT OR IGNORE INTO entries(root_id,path,name,kind,scan_gen) VALUES (?1,?2,?3,1,?4)",
        rusqlite::params![root_id, format!("/resume-root/f{i}"), format!("f{i}"), gen],
      ).unwrap();
    }
    complete_scan_gen(&conn, root_id, gen).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM entries WHERE root_id=?1", rusqlite::params![root_id], |r| r.get(0)).unwrap();
    assert_eq!(n, 200, "resume must converge exactly, no duplicates, no loss");
    let (status, _) = root_scan_status(&conn, root_id).unwrap();
    assert_eq!(status, "ready");
  }
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn crash_between_mark_done_and_persist_is_impossible_single_commit() {
  let dir = temp_dir("atomic-complete");
  std::fs::create_dir_all(&dir).unwrap();
  let db = dir.join("index.v3.db");
  let conn = open_shared_connection(&db, false).unwrap();
  let root_id = ensure_test_root(&conn, "/atomic-root");
  let gen = begin_scan_gen(&conn, root_id).unwrap();
  for i in 0..10 {
    conn.execute(
      "INSERT INTO entries(root_id,path,name,kind,scan_gen) VALUES (?1,?2,?3,1,?4)",
      rusqlite::params![root_id, format!("/atomic-root/n{i}"), format!("n{i}"), gen],
    ).unwrap();
  }
  conn.execute(
    "INSERT INTO entries(root_id,path,name,scan_gen) VALUES (?1,'/atomic-root/stale','stale',?2)",
    rusqlite::params![root_id, gen - 1],
  ).unwrap();
  complete_scan_gen(&conn, root_id, gen).unwrap();
  let stale: i64 = conn.query_row(
    "SELECT COUNT(*) FROM entries WHERE root_id=?1 AND scan_gen<>?2",
    rusqlite::params![root_id, gen], |r| r.get(0)).unwrap();
  let (status, _) = root_scan_status(&conn, root_id).unwrap();
  assert_eq!(stale, 0, "ready must never coexist with older-generation rows");
  assert_eq!(status, "ready");
  let _ = remove_dir_all_if_exists(&dir);
}
```

**RED:**

```
cargo test -p kuntu-index --locked lease_holder_crash_mid_index_recovers_and_resumes_without_dupes
```

Expected: compile error `cannot find function begin_scan_gen` (M2.6b applied, M2.6a not yet).

**GREEN:**

```
cargo test -p kuntu-index --locked
cargo clippy -p kuntu-index --all-targets --locked -- -D warnings
```

Expected: all pass including the 2 new tests; clippy clean.

**Commit:**

```
cargo fmt --all
git add crates/kuntu-index/src/lib.rs
git commit -m "feat(kuntu-index): resumable scan generations with atomic completion"
```

## Task M2.7: lease takeover poller (second app continues within 6 s)

Why: when the holder quits, another embedding app must take the lease and resume from `watch_state` with no full walk (PLAN §5.4). Every embedding app runs the poller; the recommended production cadence is try-lock every 5 s, takeover deadline 6 s.

**M2.7a.** Poller — insert before `#[cfg(test)]` in `crates/kuntu-store/src/lease.rs` (anchor occurs once in that file):

<!-- apply {"task": "M2.7", "file": "crates/kuntu-store/src/lease.rs", "mode": "insert-before", "anchor": "#[cfg(test)]"} -->
```rust
/// Production takeover cadence (frozen): poll every 5 s; a quit holder is
/// replaced within 6 s. Tests pass shorter intervals; the constants below are
/// what shipped apps use.
pub const TAKEOVER_POLL_MS: u64 = 5_000;
pub const TAKEOVER_TIMEOUT_MS: u64 = 6_000;

/// Poll until the lease is acquired or the deadline passes. Returns `None` on
/// timeout (another process still holds it). The caller that wins reads
/// `watch_state` and resumes — never a full walk (M3 owns the resume path).
pub fn poll_takeover(
  home: &Path,
  schema_minor: u32,
  interval_ms: u64,
  timeout_ms: u64,
) -> Result<Option<IndexerLease>, StoreError> {
  let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
  loop {
    if let Some(lease) = try_acquire(home, schema_minor)? {
      return Ok(Some(lease));
    }
    if std::time::Instant::now() >= deadline {
      return Ok(None);
    }
    std::thread::sleep(std::time::Duration::from_millis(interval_ms.min(500)));
  }
}

```

**M2.7b.** Tests — append to `mod tests` in `crates/kuntu-store/src/lease.rs`:

<!-- apply {"task": "M2.7", "file": "crates/kuntu-store/src/lease.rs", "mode": "append-to-tests-module"} -->
```rust
#[test]
fn takeover_acquires_within_deadline_after_holder_quits() {
  let home = temp_home("takeover");
  {
    let held = try_acquire(&home, 0).unwrap();
    assert!(held.is_some());
    // Holder quits here: lease drops, OS releases flock.
  }
  let won = poll_takeover(&home, 0, 50, 6_000).unwrap();
  assert!(won.is_some(), "second app must take over within the deadline with no duplicate rows");
  let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn takeover_times_out_while_holder_lives() {
  let home = temp_home("takeover-busy");
  let _held = try_acquire(&home, 0).unwrap().unwrap();
  let won = poll_takeover(&home, 0, 50, 300).unwrap();
  assert!(won.is_none(), "takeover must not steal a live lease");
  let _ = std::fs::remove_dir_all(&home);
}
```

**RED:**

```
cargo test -p kuntu-store --locked takeover_
```

Expected: compile error `cannot find function poll_takeover` (M2.7b applied, M2.7a not yet).

**GREEN:**

```
cargo test -p kuntu-store --locked
cargo clippy -p kuntu-store --all-targets --locked -- -D warnings
```

Expected: 12 passed; clippy clean.

**Commit:**

```
cargo fmt --all
git add crates/kuntu-store/src/lease.rs
git commit -m "feat(kuntu-store): lease takeover poller for second-app resume"
```

## Task M2.8: NAPI compatibility — old ctor keeps working, shared-store factory added

Decision (frozen, no judgement): `new FileSearchIndex(dbPath)` keeps working unchanged (deprecated private-store path, opens exactly that file). New additive API: `FileSearchIndex.withHome(home?)` resolves `KUNTU_HOME` or `~/.kuntu` via `kuntu-store` and opens `index.v3.db`; `openReadOnly()` opens the shared file read-only (works while another process holds the lease); `mode()` returns `"shared:read-write"`, `"shared:read-only"`, or `"private"`. The wire DTOs are unchanged. Regenerated `index.js`/`index.d.ts` must be diff-clean in CI (the worker regenerates with `npm run build` and commits the result).

**M2.8a.** Dependency. This block occurs once; replace with:

<!-- apply {"task": "M2.8", "file": "crates/kuntu-napi/Cargo.toml", "mode": "replace-range", "start": "kuntu-index = { path = \"../kuntu-index\" }", "end": "napi = \"3\""} -->
```toml
kuntu-index = { path = "../kuntu-index" }
kuntu-store = { path = "../kuntu-store" }
napi = "3"
```

**M2.8b.** Path resolution helper. This anchor occurs once; insert before it:

<!-- apply {"task": "M2.8", "file": "crates/kuntu-napi/src/lib.rs", "mode": "insert-before", "anchor": "fn open_index(db_path: &str) -> Result<KuntuIndex> {"} -->
```rust
/// Shared-store path resolution (pure, node-free, unit-tested below).
/// `db_path` (old ctor) wins when non-empty; otherwise the shared
/// `index.v3.db` under `home` or `KUNTU_HOME`/`~/.kuntu` is used.
pub fn resolve_db_path(home: Option<String>, db_path: Option<String>) -> String {
  if let Some(p) = db_path {
    if !p.trim().is_empty() {
      return p;
    }
  }
  let home = home.map(std::path::PathBuf::from).unwrap_or_else(| | {
    kuntu_store::store_home().unwrap_or_else(|_| std::env::temp_dir().join(".kuntu"))
  });
  kuntu_store::index_db_path(&home).to_string_lossy().into_owned()
}

```

**M2.8c.** Struct field. This block occurs once; replace with:

<!-- apply {"task": "M2.8", "file": "crates/kuntu-napi/src/lib.rs", "mode": "replace-range", "start": "pub struct FileSearchIndex {", "end": "  #[napi(constructor)]"} -->
```rust
pub struct FileSearchIndex {
  db_path: String,
  read_only: bool,
}

  #[napi(constructor)]
```

**M2.8d.** Constructor plus factory and mode. This block occurs once; replace with:

<!-- apply {"task": "M2.8", "file": "crates/kuntu-napi/src/lib.rs", "mode": "replace-range", "start": "  pub fn new(db_path: String) -> Self {", "end": "  #[napi(ts_return_type = \"Promise<IndexRebuildStatsDto>\")]"} -->
```rust
  pub fn new(db_path: String) -> Self {
    Self { db_path, read_only: false }
  }

  #[napi(factory)]
  pub fn with_home(home: Option<String>) -> Self {
    Self { db_path: resolve_db_path(home, None), read_only: false }
  }

  #[napi]
  pub fn open_read_only(&self) -> Self {
    Self { db_path: self.db_path.clone(), read_only: true }
  }

  #[napi]
  pub fn mode(&self) -> String {
    if self.read_only {
      "shared:read-only".to_string()
    } else if self.db_path.ends_with(kuntu_store::INDEX_FILE_NAME) {
      "shared:read-write".to_string()
    } else {
      "private".to_string()
    }
  }

  #[napi(ts_return_type = "Promise<IndexRebuildStatsDto>")]
```

**M2.8e.** Tests — append to `mod tests` in `crates/kuntu-napi/src/lib.rs`:

<!-- apply {"task": "M2.8", "file": "crates/kuntu-napi/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
#[test]
fn old_ctor_path_wins_over_home_resolution() {
  let p = resolve_db_path(Some("/home/x".to_string()), Some("/tmp/private.sqlite".to_string()));
  assert_eq!(p, "/tmp/private.sqlite");
}

#[test]
fn empty_db_path_falls_back_to_shared_store() {
  let p = resolve_db_path(Some("/home/x".to_string()), Some("  ".to_string()));
  assert!(p.ends_with(kuntu_store::INDEX_FILE_NAME), "{p}");
  assert!(p.starts_with("/home/x"), "{p}");
}

#[test]
fn mode_reports_private_vs_shared() {
  let priv_idx = FileSearchIndex::new("/tmp/a.sqlite".to_string());
  assert_eq!(priv_idx.mode(), "private");
  let shared = FileSearchIndex::with_home(Some("/home/x".to_string()));
  assert_eq!(shared.mode(), "shared:read-write");
  assert_eq!(shared.open_read_only().mode(), "shared:read-only");
}
```

Requires: `node` 18+ on `PATH` for the `napi-build` build script (`node --version` must succeed before running the commands below).

**RED:**

```
cargo test -p kuntu-napi --locked resolve_db_path
```

Expected: compile error `cannot find function resolve_db_path` (M2.8e applied, M2.8b not yet).

**GREEN:**

```
cargo test -p kuntu-napi --locked
cargo clippy -p kuntu-napi --all-targets --locked -- -D warnings
```

Expected: all pass including the 3 new tests; clippy clean. Then regenerate the JS glue (`npm run build` in `crates/kuntu-napi`) and confirm `git status --short -- crates/kuntu-napi/index.js crates/kuntu-napi/index.d.ts` shows only the intended regeneration diff, which is committed with this task.

**Commit:**

```
cargo fmt --all
git add crates/kuntu-napi/Cargo.toml crates/kuntu-napi/src/lib.rs crates/kuntu-napi/index.js crates/kuntu-napi/index.d.ts Cargo.lock
git commit -m "feat(kuntu-napi): shared-store factory with old ctor preserved"
```

## Task M2.9: CLI store commands and lease-holding daemon

Why: operators need `kfs store info|gc|doctor`; `kfs daemon` holds the lease while serving, and reads work with or without it (PLAN M2.9 table).

**M2.9a.** Deps. Two one-line inserts (each anchor occurs once in its file):

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/Cargo.toml", "mode": "insert-before", "anchor": "kuntu-index = { path = \"../kuntu-index\" }"} -->
```toml
kuntu-store = { path = "../kuntu-store" }
```

<!-- apply {"task": "M2.9", "file": "crates/kuntu-daemon/Cargo.toml", "mode": "insert-before", "anchor": "kuntu-index = { path = \"../kuntu-index\" }"} -->
```toml
kuntu-store = { path = "../kuntu-store" }
```

**M2.9b.** Args: `Store` command. Three edits in `crates/kuntu-cli/src/args.rs`:

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/src/args.rs", "mode": "replace-range", "start": "  Index(IndexCommand),", "end": "  Watch {"} -->
```rust
  Index(IndexCommand),
  Store(StoreCommand),
  Watch {
```

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/src/args.rs", "mode": "replace-range", "start": "pub enum IndexCommand {", "end": "fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {"} -->
```rust
pub enum IndexCommand {
  Rebuild,
  Refresh,
  Repair,
  Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreCommand {
  Info,
  Gc { days: u64 },
  Doctor,
}

pub fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
```

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/src/args.rs", "mode": "replace-range", "start": "    \"index\" => parse_index(&args[1..]),", "end": "    \"watch\" => parse_watch(&args[1..]),"} -->
```rust
    "index" => parse_index(&args[1..]),
    "store" => parse_store(&args[1..]),
    "watch" => parse_watch(&args[1..]),
```

**M2.9c.** `parse_store` — insert before `fn parse_watch(` in `crates/kuntu-cli/src/args.rs` (anchor occurs once):

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/src/args.rs", "mode": "insert-before", "anchor": "fn parse_watch("} -->
```rust
fn parse_store(args: &[String]) -> Result<ParsedArgs, String> {
  let Some(sub) = args.first() else {
    return Err("store requires info, gc, or doctor".to_string());
  };
  let command = match sub.as_str() {
    "info" => StoreCommand::Info,
    "gc" => {
      let days = args.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(30);
      StoreCommand::Gc { days }
    }
    "doctor" => StoreCommand::Doctor,
    unknown => return Err(format!("unknown store command: {unknown}")),
  };
  Ok(ParsedArgs {
    roots: Vec::new(),
    db_path: None,
    provider: SearchProvider::Auto,
    command: CommandSpec::Store(command),
  })
}

```

**M2.9d.** Dispatch plus runner — two edits in `crates/kuntu-cli/src/main.rs`:

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/src/main.rs", "mode": "replace-range", "start": "    CommandSpec::Index(command) => run_index_command(&config, spec.db_path.as_deref(), command),", "end": "    CommandSpec::Watch { duration_ms } => {"} -->
```rust
    CommandSpec::Index(command) => run_index_command(&config, spec.db_path.as_deref(), command),
    CommandSpec::Store(command) => run_store_command(command),
    CommandSpec::Watch { duration_ms } => {
```

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/src/main.rs", "mode": "insert-before", "anchor": "fn run_watch_command("} -->
```rust
fn run_store_command(command: StoreCommand) -> Result<String, String> {
  use args::StoreCommand as SC;
  let home = kuntu_store::ensure_store_home().map_err(|e| e.to_string())?;
  match command {
    SC::Info => {
      let db = kuntu_store::index_db_path(&home);
      let size = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
      let lease_free = match kuntu_store::lease::try_acquire(&home, 0).map_err(|e| e.to_string())? {
        Some(lease) => {
          drop(lease);
          "free"
        }
        None => "held",
      };
      Ok(format!("home={} epoch=3 db={} bytes={size} lease={lease_free}", home.display(), db.display()))
    }
    SC::Gc { days } => {
      let removed = kuntu_index::store_gc(&home, days).map_err(|e| e.to_string())?;
      Ok(format!("gc removed {} old epoch file(s)", removed.len()))
    }
    SC::Doctor => {
      let db = kuntu_store::index_db_path(&home);
      match kuntu_index::doctor_check(&db) {
        Ok(report) => Ok(report),
        Err(e) => Err(e.to_string()),
      }
    }
  }
}

```

The `args::StoreCommand` import: change `use args::{parse_args, CommandSpec, IndexCommand, SearchProvider};` to include `StoreCommand` (anchor occurs once):

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/src/main.rs", "mode": "replace-range", "start": "use args::{parse_args, CommandSpec, IndexCommand, SearchProvider};", "end": "use kuntu_core::{"} -->
```rust
use args::{parse_args, CommandSpec, IndexCommand, SearchProvider, StoreCommand};
use kuntu_core::{
```

**M2.9d2.** Store helpers (`store_gc`, `doctor_check`) — insert before `impl KuntuIndex {` in `crates/kuntu-index/src/lib.rs` (anchor occurs once):

<!-- apply {"task": "M2.9", "file": "crates/kuntu-index/src/lib.rs", "mode": "insert-before", "anchor": "impl KuntuIndex {"} -->
```rust
/// Epoch GC (D13): delete `index.v*.db` files (plus `-wal`/`-shm`) in `home`
/// that are not the current `index.v3.db` and whose mtime is older than
/// `max_age_days`. Returns removed file names. Never touches the current
/// epoch file. Default retention is 30 days.
pub fn store_gc(home: &std::path::Path, max_age_days: u64) -> Result<Vec<String>, IndexError> {
  let current = kuntu_store::index_db_path(home);
  let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(max_age_days * 86400);
  let mut removed = Vec::new();
  let entries = std::fs::read_dir(home).map_err(|e| IndexError::Io(e.to_string()))?;
  for entry in entries {
    let entry = entry.map_err(|e| IndexError::Io(e.to_string()))?;
    let name = entry.file_name().to_string_lossy().into_owned();
    if !name.starts_with("index.v") || !name.ends_with(".db") {
      continue;
    }
    let path = entry.path();
    if path == current {
      continue;
    }
    let mtime = entry.metadata().map_err(|e| IndexError::Io(e.to_string()))?.modified()
      .map_err(|e| IndexError::Io(e.to_string()))?;
    if mtime < cutoff {
      std::fs::remove_file(&path).map_err(|e| IndexError::Io(e.to_string()))?;
      for ext in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", path.to_string_lossy()));
      }
      removed.push(name);
    }
  }
  Ok(removed)
}

/// `kfs store doctor`: `quick_check` plus a read probe. Returns a one-line
/// report. On corruption the caller quarantines and rebuilds (M2.4).
pub fn doctor_check(db_path: &std::path::Path) -> Result<String, IndexError> {
  let conn = open_shared_connection(db_path, true)?;
  let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  let entries: i64 = conn.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  Ok(format!("{}: quick_check={check} entries={entries}", db_path.display()))
}


```

**M2.9e.** Daemon holds the lease while serving; reads never require it. Two edits in `crates/kuntu-daemon/src/lib.rs`:

<!-- apply {"task": "M2.9", "file": "crates/kuntu-daemon/src/lib.rs", "mode": "replace-range", "start": "  let started = Instant::now();", "end": "  let mut requests = 0;"} -->
```rust
  let started = Instant::now();
  let home = config.db_path.parent().map(|p| p.to_path_buf());
  let _lease = match home {
    Some(h) if h.join("locks").exists() || h.join(kuntu_store::STORE_JSON_NAME).exists() => {
      kuntu_store::lease::try_acquire(&h, 0).map_err(|err| BackendError::new(err.to_string()))?
    }
    _ => None,
  };
  let mut requests = 0;
```

<!-- apply {"task": "M2.9", "file": "crates/kuntu-daemon/src/lib.rs", "mode": "replace-range", "start": "    match listener.accept() {", "end": "  Ok(DaemonRunStats {"} -->
```rust
    match listener.accept() {
      Ok((stream, _addr)) => {
        let db_path = config.db_path.clone();
        let allowed = config.allowed_roots.clone();
        std::thread::spawn(move || {
          let mut conn_index = match KuntuIndex::open(&db_path) {
            Ok(index) => index,
            Err(_) => return,
          };
          let _ = handle_connection(&mut conn_index, stream, &allowed);
        });
        requests += 1;
      }
      Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
        thread::sleep(Duration::from_millis(10));
      }
      Err(err) => return Err(BackendError::from(err)),
    }
  }

  Ok(DaemonRunStats {
```

Note: `handle_connection` takes `&mut KuntuIndex` and `TcpStream: Send`; each thread opens its own read-capable connection, so concurrent readers never share a `Connection`. The original single `index` binding becomes unused — delete the two lines `let mut index =` / `KuntuIndex::open(&config.db_path)...` as part of applying M2.9e (they sit between the two replaced blocks; remove them).

**M2.9f.** Tests — append to `mod tests` in `crates/kuntu-cli/src/args.rs`:

<!-- apply {"task": "M2.9", "file": "crates/kuntu-cli/src/args.rs", "mode": "append-to-tests-module"} -->
```rust
#[test]
fn store_info_parses_without_roots_or_db() {
  let parsed = parse_args(&strings(&["store", "info"])).unwrap();
  assert_eq!(parsed.command, CommandSpec::Store(StoreCommand::Info));
}

#[test]
fn store_gc_defaults_to_30_days() {
  let parsed = parse_args(&strings(&["store", "gc"])).unwrap();
  assert_eq!(parsed.command, CommandSpec::Store(StoreCommand::Gc { days: 30 }));
}

#[test]
fn store_unknown_subcommand_errors() {
  assert!(parse_args(&strings(&["store", "vacuum"])).is_err());
}
```

Append to `mod tests` in `crates/kuntu-daemon/src/lib.rs`:

<!-- apply {"task": "M2.9", "file": "crates/kuntu-daemon/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
fn daemon_test_port() -> u16 {
  15400 + (std::process::id() % 20000) as u16
}

fn http_get(port: u16, path: &str) -> String {
  use std::io::{Read, Write};
  let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
  stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
  stream.write_all(format!("GET {path} HTTP/1.1\r\nContent-Length: 0\r\n\r\n").as_bytes()).unwrap();
  let mut buf = Vec::new();
  stream.read_to_end(&mut buf).unwrap();
  String::from_utf8_lossy(&buf).into_owned()
}

/// Reads work without the lease while another process holds it: the lease is
/// held in this test process, the daemon serves with `hold_lease` semantics
/// off, and 4 concurrent `/health` readers all get 200.
#[test]
fn reads_work_without_lease_while_another_process_holds_it() {
  let dir = temp_dir("daemon-nolease");
  std::fs::create_dir_all(&dir).unwrap();
  let _holder = kuntu_store::lease::try_acquire(&dir, 0).unwrap().unwrap();
  let port = daemon_test_port();
  let config = DaemonConfig {
    db_path: dir.join("index.v3.db"),
    addr: format!("127.0.0.1:{port}"),
    duration: Some(Duration::from_secs(10)),
    max_requests: Some(4),
    allowed_roots: Vec::new(),
    allow_non_loopback: false,
  };
  let server = std::thread::spawn(|| serve(config).unwrap());
  std::thread::sleep(std::time::Duration::from_millis(300));
  let bodies: Vec<_> = (0..4).map(|_| {
    let port = port;
    std::thread::spawn(move || http_get(port, "/health"))
  }).collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap()).collect();
  assert_eq!(bodies.len(), 4);
  for b in &bodies {
    assert!(b.contains("200"), "{b}");
  }
  let stats = server.join().unwrap();
  assert_eq!(stats.requests, 4);
  let _ = std::fs::remove_dir_all(&dir);
}
```

**RED:**

```
cargo test -p kuntu-cli --locked store_info_parses_without_roots_or_db
```

Expected: compile error `cannot find StoreCommand` / `parse_store` (tests applied, M2.9b–M2.9d not yet).

**GREEN:**

```
cargo test -p kuntu-cli --locked
cargo test -p kuntu-daemon --locked
cargo fmt --all
cargo clippy -p kuntu-cli -p kuntu-daemon --all-targets --locked -- -D warnings
```

Expected: all pass including the 4 new tests (daemon test takes ~1 s with real TCP readers); clippy clean.

**Commit:**

```
cargo fmt --all
git add crates/kuntu-cli/Cargo.toml crates/kuntu-cli/src/args.rs crates/kuntu-cli/src/main.rs crates/kuntu-daemon/Cargo.toml crates/kuntu-daemon/src/lib.rs crates/kuntu-index/src/lib.rs Cargo.lock
git commit -m "feat(kfs): store commands and lease-holding daemon"
```

## Task M2.10: turso import path, epoch GC, consumers, bench recipe, release note

Decisions (frozen): the `terms`-era data is **not** migrated — the index is derived, so a v3 build creates `index.v3.db` and re-indexes from source roots. `kfs store import --db <v2-file>` copies only the root list (read-only open of the v2 file) and rebuilds; the v2 file is never modified. Old epochs are removed only by `kfs store gc` after 30 days unused (D13). Xross moves its `xross_search_exports` mapping table to its own DB (D5); Kunkun reads via `openReadOnly()` and writes via the `requests` inbox.

**M2.10a.** Import path helper (GC/doctor landed in M2.9d2) — insert before `impl KuntuIndex {` in `crates/kuntu-index/src/lib.rs` (anchor occurs once):

<!-- apply {"task": "M2.10", "file": "crates/kuntu-index/src/lib.rs", "mode": "insert-before", "anchor": "impl KuntuIndex {"} -->
```rust
/// Import path from a turso v2 store (PLAN §5.3): read the root list with a
/// read-only open and return it. The v2 file is never written to and no
/// `terms` data is carried over; the caller re-indexes from these roots into
/// `index.v3.db`. v2 files are plain SQLite files readable by rusqlite.
pub fn import_v2_roots(v2_path: &std::path::Path) -> Result<Vec<String>, IndexError> {
  let conn = Connection::open_with_flags(v2_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  let mut st = conn.prepare("SELECT path FROM roots ORDER BY path")
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  let rows = st.query_map([], |r| r.get::<_, String>(0))
    .map_err(|e| IndexError::Engine(e.to_string()))?;
  let mut roots = Vec::new();
  for r in rows {
    roots.push(r.map_err(|e| IndexError::Engine(e.to_string()))?);
  }
  Ok(roots)
}

```

**M2.10b.** Tests — append to `mod tests` in `crates/kuntu-index/src/lib.rs`:

<!-- apply {"task": "M2.10", "file": "crates/kuntu-index/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
#[test]
fn import_reads_v2_roots_without_touching_source() {
  let dir = temp_dir("import");
  std::fs::create_dir_all(&dir).unwrap();
  let v2 = dir.join("old.sqlite");
  {
    let conn = Connection::open(&v2).unwrap();
    conn.execute_batch(
      "CREATE TABLE roots(id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, enabled INTEGER NOT NULL, priority INTEGER NOT NULL DEFAULT 0, include_hidden INTEGER NOT NULL DEFAULT 0, include_ignored INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL);
       INSERT INTO roots(path, enabled, updated_at) VALUES ('/a', 1, 0), ('/b', 1, 0);",
    ).unwrap();
  }
  let before = std::fs::read(&v2).unwrap();
  let roots = import_v2_roots(&v2).unwrap();
  assert_eq!(roots, vec!["/a".to_string(), "/b".to_string()]);
  assert_eq!(std::fs::read(&v2).unwrap(), before, "import must never modify the v2 file");
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn two_processes_write_concurrently_across_processes() {
  let dir = temp_dir("xproc-write");
  std::fs::create_dir_all(&dir).unwrap();
  let db = dir.join("index.v3.db");
  {
    let setup = open_shared_connection(&db, false).unwrap();
    setup.execute_batch("CREATE TABLE IF NOT EXISTS xp(id INTEGER PRIMARY KEY, v TEXT)").unwrap();
  }
  let child = std::process::Command::new("python3")
    .arg("-c")
    .arg(format!(
      "import sqlite3, time; c=sqlite3.connect(r'{}'); c.execute('PRAGMA journal_mode=WAL'); c.execute('PRAGMA busy_timeout=5000'); [c.execute(\"INSERT INTO xp(v) VALUES (?)\", (f'p{{}}',)) for _ in [0]]; c.commit(); [c.execute(\"INSERT INTO xp(v) VALUES (?)\", (f'py-{{i}}',)) or c.commit() for i in range(50)]; c.close()",
      db.to_string_lossy()
    ))
    .spawn()
    .expect("python3 with sqlite3 is required for the cross-process write test");
  let conn = open_shared_connection(&db, false).unwrap();
  for i in 0..50 {
    conn.execute("INSERT INTO xp(v) VALUES (?1)", [format!("rs-{i}")]).unwrap();
  }
  let status = child.wait_with_output().unwrap();
  assert!(status.status.success(), "python writer must exit 0");
  let n: i64 = conn.query_row("SELECT COUNT(*) FROM xp", [], |r| r.get(0)).unwrap();
  assert_eq!(n, 101, "both processes must commit every row, zero SQLITE_BUSY losses");
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn gc_removes_only_old_epochs() {
  let dir = temp_dir("gc");
  std::fs::create_dir_all(&dir).unwrap();
  let current = dir.join("index.v3.db");
  let old = dir.join("index.v2.db");
  std::fs::write(&current, b"v3").unwrap();
  std::fs::write(&old, b"v2").unwrap();
  let kept = store_gc(&dir, 30_000).unwrap();
  assert!(kept.is_empty(), "fresh old epoch must be kept: {kept:?}");
  let removed = store_gc(&dir, 0).unwrap();
  assert_eq!(removed, vec!["index.v2.db".to_string()]);
  assert!(current.exists());
  assert!(!old.exists());
  let _ = remove_dir_all_if_exists(&dir);
}

#[test]
fn busy_timeout_turns_busy_into_wait() {
  let dir = temp_dir("busy");
  std::fs::create_dir_all(&dir).unwrap();
  let db = dir.join("index.v3.db");
  let a = open_shared_connection(&db, false).unwrap();
  a.execute_batch("CREATE TABLE IF NOT EXISTS b(id INTEGER PRIMARY KEY)").unwrap();
  a.execute("BEGIN IMMEDIATE", []).unwrap();
  let db2 = db.clone();
  let writer = std::thread::spawn(move || {
    let b = open_shared_connection(&db2, false).unwrap();
    b.execute("INSERT INTO b DEFAULT VALUES", []).unwrap();
  });
  std::thread::sleep(std::time::Duration::from_millis(200));
  a.execute("COMMIT", []).unwrap();
  writer.join().unwrap();
  let _ = remove_dir_all_if_exists(&dir);
}
```

**M2.10c.** Bench recipe — insert before the `benchmark-1m:` line in `justfile` (anchor occurs once):

<!-- apply {"task": "M2.10", "file": "justfile", "mode": "insert-before", "anchor": "benchmark-1m:"} -->
```make
# The shared-store (v3) latency gate on the generated realistic fixture.
benchmark-v3:
    KUNTU_BENCH_ROWS=100000 cargo test --release -p kuntu-index --locked realistic_fixture_latency_gate -- --ignored --nocapture

```

**M2.10d.** Release note — insert before the `| \`v0.5.0\` |` row in `README.md` (anchor occurs once):

<!-- apply {"task": "M2.10", "file": "README.md", "mode": "insert-before", "anchor": "| `v0.5.0` |"} -->
```markdown
| `v0.6.0` | Shared `~/.kuntu` store (`KUNTU_HOME` override): SQLite WAL engine via rusqlite bundled (`index.v3.db`, schema epoch 3, `name_postings`/`path_postings` split, ~0.5 GB at 1M rows); many readers plus one writer across processes (`busy_timeout=5000`, short batches, `requests` inbox); indexer `flock` lease with takeover; additive minor migrations by the lease holder; enforced non-empty `SearchScope`; old epochs GC after 30 days. No `terms` migration — v3 rebuilds from source roots (`kfs store import --db <v2-file>` copies the root list only). JS/NAPI: `new FileSearchIndex(dbPath)` keeps working; new `withHome(home?)`, `openReadOnly()`, `mode()`. M1 query shapes and latency gate unchanged. |
```

**M2.10e.** Consumer steps (docs-only, no code): Xross moves `open_mapping_connection` (second turso connection on the same file) to its own small DB in its state dir and opens the shared store through `KuntuStore`; export root ids stay the only search scope. Kunkun's app server opens read-only (`openReadOnly()` / `FileSearchIndex.openReadOnly()`); write requests go through the `requests` inbox or helper socket. No consumer pin moves in this milestone (owner runs them per CONTRIBUTING.md).

**RED:**

```
cargo test -p kuntu-index --locked import_reads_v2_roots_without_touching_source
```

Expected: compile error `cannot find function import_v2_roots` (M2.10b applied, M2.10a not yet).

**GREEN:**

```
cargo test -p kuntu-index --locked
cargo test -p kuntu-store --locked
cargo fmt --all
cargo clippy -p kuntu-index --all-targets --locked -- -D warnings
```

Expected: all pass including the 4 new tests (cross-process test takes ~2 s); clippy clean.

**Commit:**

```
cargo fmt --all
git add crates/kuntu-index/src/lib.rs justfile README.md
git commit -m "feat(kuntu-index): turso import path, epoch GC, release note"
```

## Concurrency and crash checklist (each item has a named test)

| # | Requirement | Named test(s) | Task |
|---|---|---|---|
| C1 | Two processes writing: both commit everything, zero `SQLITE_BUSY` losses | `two_writers_make_progress_without_busy_errors` (threads, separate conns) and `two_processes_write_concurrently_across_processes` (python3 + Rust, real processes) | M2.5, M2.10 |
| C2 | Reader during writer: read-only opens succeed mid-batch, snapshot valid, zero errors | `reader_during_writer_sees_a_snapshot_without_error` | M2.5 |
| C3 | Lease holder crash mid-index: OS releases `flock` (`kill -9` proven), next holder resumes the same `scan_gen` with no dupes or loss | `lease_recoverable_after_kill_9_within_1s`, `lease_holder_crash_mid_index_recovers_and_resumes_without_dupes` | M2.2, M2.6 |
| C4 | Crash between "mark done" and "persist": impossible by construction — completion is one `COMMIT` (stale-row delete + `ready` + counts); test asserts `ready` never coexists with older-gen rows | `crash_between_mark_done_and_persist_is_impossible_single_commit` | M2.6 |
| C5 | Migration while another process has the old epoch open: other-epoch file untouched; refused opens modify nothing | `migration_while_old_epoch_open_leaves_old_file_untouched`, `open_rules_refuse_or_downgrade_without_modifying`, `corrupt_file_is_quarantined_not_deleted` | M2.4 |
| C6 | `KUNTU_HOME` permissions: `0700` dirs / `0600` files created; group/world-writable refused; symlinked home refused; network FS refused | `ensure_creates_0700_and_store_json_0600`, `ensure_refuses_group_writable_home`, `ensure_refuses_symlinked_home`, `network_fs_refusal_is_mockable` | M2.1 |
| C7 | Client trusting another process's root ids: empty scope rejected at construction; search filters `root_id IN (scope)`; foreign roots never leak | `scope_is_enforced_non_empty_and_sorted` plus the `CandidateScope::admit` port review in M2.3 (not a test function) | M2.4 |
| C8 | `busy_timeout` and `SQLITE_BUSY`: 5 s timeout set on every open; contention waits then succeeds; never fails fast | `v3_open_creates_epoch_file_with_wal_and_meta`, `busy_timeout_turns_busy_into_wait`, `writer_steps_down_when_min_writer_bumped_inside_txn` | M2.3, M2.10, M2.5 |

Note: every test named above is defined exactly once in this file (verified by grep for `fn <name>()` during writing). The M2.3 port review (`CandidateScope::admit` line-for-line port) is a review item, not a test function.

## Turso store migration and JS/NAPI API compatibility (decided)

- **Import path (rebuild-only):** `kfs store import --db <v2-turso-file> [--root <path>...]` opens the v2 file read-only, reads `roots(path)`, creates `index.v3.db` if needed, and re-indexes from those roots (plus any `--root` flags). No `terms`/`entries` rows are copied. The v2 file is byte-identical afterwards (asserted by test). A v3 build never opens, migrates, or deletes a v2 file except `kfs store gc` after 30 days.
- **JS/NAPI compatibility:** `new FileSearchIndex(dbPath)` — unchanged, opens exactly that file (private store, mode `"private"`). `FileSearchIndex.withHome(home?)` — shared store (`KUNTU_HOME` or `~/.kuntu` `/index.v3.db`). `openReadOnly()` — same path, read-only (works while another process holds the lease). `mode()` — `"private"` / `"shared:read-write"` / `"shared:read-only"`. All search/rebuild/refresh/repair/status DTOs unchanged. Regenerated `index.js`/`index.d.ts` committed in M2.8.

## Owner decisions beyond D1–D14 (recommended defaults)

| # | Decision | Recommended default |
|---|---|---|
| O1 | `rusqlite` version for the shared store | **0.32 with `bundled`** (validates FTS5 availability for M4/M5 later; no system-sqlite variance) |
| O2 | `flock` wrapper | **`fs4 0.13` via `fs4::fs_std::FileExt`** (safe wrapper; workspace keeps `unsafe_code = "forbid"`) |
| O3 | `busy_timeout` value | **5000 ms on every open** (RESEARCH stress: longest write-lock wait 43 ms; 5 s absorbs rebuild batches without masking deadlocks) |
| O4 | Writer batch cap | **5,000 rows or 100 ms** per transaction (PLAN §5.4) |
| O5 | Takeover cadence | **poll every 5 s, deadline 6 s** (`TAKEOVER_POLL_MS`/`TAKEOVER_TIMEOUT_MS`; tests use shorter intervals) |
| O6 | Old-epoch retention | **30 days unused, `kfs store gc` only** (D13); never automatic on open |
| O7 | `mmap_size` cap | **64 MiB** (`PRAGMA mmap_size = 67108864`; bounds reader RSS on 1M-entry stores) |
| O8 | NAPI surface shape | **old ctor preserved + `withHome` factory + `openReadOnly`/`mode`** (no breaking change; Xross/Kunkun adopt at their pace) |
| O9 | Import semantics | **root-list-only rebuild; v2 file never modified** (index is derived data) |
| O10 | `store_id` generation | **32-hex-char mixed time+pid value in `store.json`** (no `uuid` dependency; uniqueness per creation, not RFC 4122) |
| O11 | Network-FS deny list | **`nfs/nfs4/smbfs/smb2/afpfs/fuse/fusefs/sshfs/cifs`** substring match on `statfs` type name; WAL needs local FS |
| O12 | Minor-version plumbing | **`meta` keys `schema_minor`/`min_reader_minor`/`min_writer_minor` + `decide_access` + per-txn re-read** (PLAN §5.5 verbatim) |

## Final verification (no new files)

Run in this order and paste the real output lines into the hand-back report:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p kuntu-store -p kuntu-core -p kuntu-index -p kuntu-daemon -p kuntu-cli --locked
cargo test --release -p kuntu-index --locked -- --ignored --nocapture --test-threads=1
KUNTU_BENCH_ROWS=100000 KUNTU_BENCH_DIR=/Volumes/Portable2TB/tmp/kuntu-m2-bench \
  cargo test --release -p kuntu-index --locked realistic_fixture_latency_gate -- --ignored --nocapture
git status --short
git diff main --stat
```

Expected: fmt and clippy clean; the debug suite passes (all crates above); the ignored suite passes; the 100k gate passes all nine classes on the new engine (same ceilings as M1 — this is the "M1 shape kept green" proof; the owner runs the 1M gate on the internal disk); `git status` clean; the diff touches only the write set (`git diff main --stat -- crates/kuntu-crawler crates/kuntu-watcher crates/kuntu-scan .github` is empty).

Not run by this plan (state in the report): `kuntu-napi` Node smoke beyond `cargo test -p kuntu-napi` (needs the JS toolchain from M2.8), Windows/Linux CI legs, installed-product acceptance, real-disk 1M numbers, Xross/Kunkun consumer tests. Consumer pin moves are separate owner steps.

## Replay results

PENDING — filled by the replay pass below (do not edit this section by hand; the replay loop updates it and re-commits until every RED/GREEN reproduces).
