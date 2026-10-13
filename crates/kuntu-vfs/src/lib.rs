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
