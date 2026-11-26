//! Real on-disk fixtures: attributes and reparse points written to NTFS, then
//! read back by the scanner exactly as it reads a provider's placeholders.

use super::super::{scan_local_directory, LocalScanNode, LocalScanReport};
use super::*;
use crate::{IgnoredMode, ScanOptions};
use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicU64, Ordering};
use windows_sys::Wdk::Storage::FileSystem::RtlQueryThreadPlaceholderCompatibilityMode;
use windows_sys::Win32::System::IO::DeviceIoControl;

const CLOUD_TAG: u32 = 0x9000_601A;
const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
const FSCTL_DELETE_REPARSE_POINT: u32 = 0x0009_00AC;

/// The folder, and the reparse points to remove before it: the cloud filter
/// refuses ordinary deletion of a hand-written Cloud Files entry.
struct Fixture(PathBuf, std::cell::RefCell<Vec<(PathBuf, u32)>>);
impl Fixture {
  fn new() -> Self {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
      "kuntu-win-{}-{}",
      std::process::id(),
      NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    Self(path, Default::default())
  }
  fn reparse_point(&self, path: &Path, tag: u32) {
    reparse_control(path, FSCTL_SET_REPARSE_POINT, tag, &[0u8; 4]).unwrap();
    self.1.borrow_mut().push((path.to_path_buf(), tag));
  }
  fn file(&self, name: &str, size: usize) -> PathBuf {
    let path = self.0.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, vec![b'x'; size]).unwrap();
    path
  }
  fn dir(&self, name: &str) -> PathBuf {
    let path = self.0.join(name);
    fs::create_dir_all(&path).unwrap();
    path
  }
  fn scan(&self, path: &Path) -> LocalScanReport {
    scan_local_directory(ScanOptions {
      directories: vec![path.to_path_buf()],
      ignore_hidden: false,
      full_path: false,
      respect_gitignore: true,
      ignored_mode: IgnoredMode::Summarize,
      follow_symlinks: false,
    })
    .unwrap()
  }
}
impl Drop for Fixture {
  fn drop(&mut self) {
    for (path, tag) in self.1.borrow().iter() {
      if reparse_control(path, FSCTL_DELETE_REPARSE_POINT, *tag, &[]).is_err() {
        // A delete-on-close open removes the entry even when the filter then
        // reports the open as failed.
        let _ = fs::OpenOptions::new()
          .access_mode(DELETE)
          .custom_flags(
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_DELETE_ON_CLOSE,
          )
          .open(path);
      }
    }
    if let Err(error) = fs::remove_dir_all(&self.0) {
      eprintln!("could not remove the fixture {:?}: {error}", self.0);
    }
  }
}

fn child<'a>(node: &'a LocalScanNode, name: &str) -> &'a LocalScanNode {
  node
    .children
    .iter()
    .find(|child| child.name == name)
    .unwrap_or_else(|| panic!("no child {name} in {:?}", node.path))
}

fn assert_skipped(node: &LocalScanNode, reason: &str) {
  assert_eq!(node.scan_state, "skipped", "{:?}", node.path);
  assert_eq!(node.skip_reason.as_deref(), Some(reason), "{:?}", node.path);
  assert_eq!((node.size, node.logical_size), (0, 0), "{:?}", node.path);
  assert!(node.children.is_empty(), "{:?}", node.path);
}

fn set_offline(path: &Path) {
  let wide = terminated(path.as_os_str());
  // SAFETY: a terminated string that outlives the call.
  let ok = unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_OFFLINE) };
  assert_ne!(ok, 0, "{}", io::Error::last_os_error());
}

/// Sets or deletes a reparse point. Microsoft tags take a bare header;
/// third-party tags also carry an owner GUID.
fn reparse_control(path: &Path, control: u32, tag: u32, data: &[u8]) -> io::Result<()> {
  let file = fs::OpenOptions::new()
    .write(true)
    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
    .open(path)?;
  let mut buffer = Vec::new();
  buffer.extend_from_slice(&tag.to_le_bytes());
  buffer.extend_from_slice(&(data.len() as u16).to_le_bytes());
  buffer.extend_from_slice(&0u16.to_le_bytes());
  if tag & 0x8000_0000 == 0 {
    buffer.extend_from_slice(&[7u8; 16]);
  }
  buffer.extend_from_slice(data);
  let mut returned = 0;
  // SAFETY: the input buffer outlives the synchronous call.
  let ok = unsafe {
    DeviceIoControl(
      file.as_raw_handle() as HANDLE,
      control,
      buffer.as_ptr().cast(),
      buffer.len() as u32,
      std::ptr::null_mut(),
      0,
      &mut returned,
      std::ptr::null_mut(),
    )
  };
  if ok == 0 {
    return Err(io::Error::last_os_error());
  }
  Ok(())
}

fn junction(link: &Path, target: &Path) {
  let output = std::process::Command::new("cmd")
    .args(["/C", "mklink", "/J"])
    .arg(link)
    .arg(target)
    .output()
    .unwrap();
  assert!(
    output.status.success(),
    "{}",
    String::from_utf8_lossy(&output.stderr)
  );
}

#[test]
fn a_local_folder_is_measured_completely() {
  let fixture = Fixture::new();
  fixture.file("a", 8192);
  fixture.file("nested/deeper/b", 20_000);
  let report = fixture.scan(&fixture.0);
  assert!(
    report.coverage.issues.is_empty(),
    "{:?}",
    report.coverage.issues
  );
  assert_eq!(report.coverage.mode, "local-only");
  assert_eq!(report.coverage.files, 2);
  assert_eq!(report.coverage.directories, 3);
  assert_eq!(report.coverage.logical_bytes, 28_192);
  let root = &report.nodes[0];
  assert_eq!(root.scan_state, "complete");
  assert!(root.size >= 28_192, "allocated {}", root.size);
  assert_eq!(child(root, "a").logical_size, 8192);
  assert!(report.volumes[0].is_local && report.volumes[0].total_bytes > 0);
}

#[test]
fn a_listing_longer_than_one_buffer_is_read_in_full() {
  let fixture = Fixture::new();
  for index in 0..1500 {
    fixture.file(&format!("a-reasonably-long-file-name-number-{index:05}"), 1);
  }
  let report = fixture.scan(&fixture.0);
  assert_eq!(report.coverage.files, 1500);
  assert_eq!(report.coverage.logical_bytes, 1500);
}

#[test]
fn hard_links_count_their_allocation_once() {
  let fixture = Fixture::new();
  let file = fixture.file("file", 64 * 1024);
  let single = fixture.scan(&fixture.0).nodes[0].size;
  fs::hard_link(&file, fixture.0.join("link")).unwrap();
  let report = fixture.scan(&fixture.0);
  assert_eq!(report.coverage.logical_bytes, 128 * 1024);
  assert_eq!(report.nodes[0].size, single);
}

#[test]
fn offline_attribute_marks_files_online_only_and_folders_cloud() {
  let fixture = Fixture::new();
  fixture.file("kept", 8192);
  set_offline(&fixture.file("placeholder", 8192));
  fixture.file("remote/inside", 8192);
  set_offline(&fixture.0.join("remote"));
  let report = fixture.scan(&fixture.0);
  let root = &report.nodes[0];
  assert_skipped(child(root, "placeholder"), "dataless");
  assert_skipped(child(root, "remote"), "cloud-root");
  assert_eq!(report.coverage.logical_bytes, 8192);
  assert_eq!(root.scan_state, "partial");
  // Named as a root, or reached through one, it stays closed.
  for path in [
    fixture.0.join("remote"),
    fixture.0.join("remote").join("inside"),
  ] {
    let report = fixture.scan(&path);
    assert_skipped(&report.nodes[0], "cloud-root");
    assert_eq!(report.coverage.files, 0);
  }
}

#[test]
fn cloud_reparse_tags_are_excluded_without_any_cloud_name() {
  let fixture = Fixture::new();
  fixture.file("kept", 8192);
  fixture.reparse_point(&fixture.file("hydrated", 0), CLOUD_TAG);
  fixture.reparse_point(&fixture.dir("synced"), CLOUD_TAG);
  let report = fixture.scan(&fixture.0);
  let root = &report.nodes[0];
  assert_skipped(child(root, "hydrated"), "cloud-root");
  assert_skipped(child(root, "synced"), "cloud-root");
  assert!(child(root, "synced").is_directory);
  assert_eq!(report.coverage.files, 1);
  // As a root. The cloud filter refuses to open a hand-written Cloud Files tag
  // (it carries no provider data), which still leaves the folder closed.
  let report = fixture.scan(&fixture.0.join("synced"));
  assert_eq!(report.nodes[0].scan_state, "skipped");
  assert_eq!(report.coverage.files, 0);
  // A provider tag the filter leaves alone is recognised from the handle.
  fixture.reparse_point(&fixture.dir("legacy"), 0x8000_0021);
  let report = fixture.scan(&fixture.0.join("legacy"));
  assert_skipped(&report.nodes[0], "cloud-root");
}

#[test]
fn unknown_reparse_points_fail_closed() {
  let fixture = Fixture::new();
  fixture.reparse_point(&fixture.file("odd", 0), 0x0000_1234);
  fixture.reparse_point(&fixture.dir("odd-folder"), 0x0000_1234);
  let report = fixture.scan(&fixture.0);
  assert_skipped(child(&report.nodes[0], "odd"), "reparse-point");
  assert_skipped(child(&report.nodes[0], "odd-folder"), "reparse-point");
}

#[test]
fn junctions_are_never_followed() {
  let fixture = Fixture::new();
  fixture.file("actual/file", 8192);
  junction(&fixture.0.join("link"), &fixture.0.join("actual"));
  let report = fixture.scan(&fixture.0);
  assert_skipped(child(&report.nodes[0], "link"), "symlink");
  assert_eq!(report.coverage.files, 1);
  // Like a symlink, a junction does not make its parent incomplete.
  assert_eq!(report.nodes[0].scan_state, "complete");
  for path in [fixture.0.join("link"), fixture.0.join("link").join("file")] {
    let report = fixture.scan(&path);
    assert_skipped(&report.nodes[0], "symlink");
  }
}

#[test]
fn cloud_named_folders_are_excluded_before_they_are_opened() {
  let fixture = Fixture::new();
  for name in ["OneDrive", "OneDrive - Contoso", "Dropbox", "iCloudDrive"] {
    fixture.file(&format!("{name}/file"), 8192);
  }
  fixture.file("Google Drive/file", 8192);
  let report = fixture.scan(&fixture.0);
  assert_eq!(report.coverage.files, 0);
  assert_eq!(
    report.coverage.skipped_by_reason.get("cloud-root"),
    Some(&5)
  );
}

#[test]
fn placeholder_ignore_rules_are_not_read() {
  let fixture = Fixture::new();
  fixture.file("ignored", 8192);
  let rules = fixture.0.join(".gitignore");
  fs::write(&rules, "ignored\n").unwrap();
  assert!(child(&fixture.scan(&fixture.0).nodes[0], "ignored").ignored);
  set_offline(&rules);
  let report = fixture.scan(&fixture.0);
  assert!(!child(&report.nodes[0], "ignored").ignored);
  assert_skipped(child(&report.nodes[0], ".gitignore"), "dataless");
  assert_eq!(report.nodes[0].scan_state, "partial");
}

#[test]
fn network_paths_and_missing_entries_are_reported_by_reason() {
  let fixture = Fixture::new();
  for path in [r"\\localhost\C$\Users", r"\\?\UNC\server\share\folder"] {
    let report = fixture.scan(Path::new(path));
    assert_skipped(&report.nodes[0], "non-local-volume");
  }
  let report = fixture.scan(&fixture.0.join("absent"));
  assert_skipped(&report.nodes[0], "missing");
  let verbatim = fs::canonicalize(&fixture.0).unwrap();
  assert_eq!(fixture.scan(&verbatim).nodes[0].scan_state, "complete");
}

#[test]
fn a_directory_replaced_after_its_listing_is_refused() {
  let fixture = Fixture::new();
  fixture.file("target/original", 1);
  fixture.file("elsewhere/foreign", 1);
  let guard = super::super::PolicyGuard::enter().unwrap();
  let (root, _, _) = open_root(&fixture.0, &|| Ok(())).unwrap();
  entries(&fixture.0, &root, &|| Ok(())).unwrap();
  let path = fixture.0.join("target");
  let listed = metadata(&path, &root).unwrap();
  fs::rename(&path, fixture.0.join("held")).unwrap();
  junction(&path, &fixture.0.join("elsewhere"));
  let error = directory(&path, &root, listed).err().unwrap();
  assert_eq!(super::super::error_reason(&error), "symlink");
  fs::remove_dir(&path).unwrap();
  fs::rename(fixture.0.join("elsewhere"), &path).unwrap();
  let error = directory(&path, &root, listed).err().unwrap();
  assert_eq!(super::super::error_reason(&error), "changed-identity");
  guard.finish(Ok(())).unwrap();
}

#[test]
fn placeholders_are_exposed_during_a_scan_and_the_thread_mode_is_restored() {
  // SAFETY: reads a per-thread setting.
  let mode = || unsafe { RtlQueryThreadPlaceholderCompatibilityMode() };
  let before = mode();
  let guard = super::super::PolicyGuard::enter().unwrap();
  assert_eq!(mode(), EXPOSE_PLACEHOLDERS);
  guard.finish(Ok(())).unwrap();
  assert_eq!(mode(), before);
}

#[test]
fn names_that_are_not_unicode_do_not_break_the_report() {
  let fixture = Fixture::new();
  let name = OsString::from_wide(&[u16::from(b'n'), 0xD800]);
  fs::create_dir(fixture.0.join(&name)).unwrap();
  fs::write(fixture.0.join(&name).join("node_modules"), b"x").unwrap();
  let report = fixture.scan(&fixture.0);
  assert_eq!(report.coverage.files, 1);
  // The scan keeps the exact name; only its serialized spelling is lossy.
  assert_eq!(report.nodes[0].children[0].path, fixture.0.join(&name));
  let json = serde_json::to_value(&report).unwrap();
  assert!(json["nodes"][0]["children"][0]["path"]
    .as_str()
    .unwrap()
    .ends_with("n\u{fffd}"));
  let candidates = crate::find_candidates(crate::CandidateOptions {
    roots: vec![fixture.0.clone()],
    presets: vec![],
    ignore_hidden: false,
    follow_symlinks: false,
  });
  serde_json::to_string(&crate::build_removal_plan(candidates)).unwrap();
}
