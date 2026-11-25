//! Which entries are local. Pure rules over names, Windows file attributes and
//! reparse tags: nothing here touches the filesystem, and no rule ever needs a
//! file's contents.

use std::path::{Component, Path, PathBuf};

/// Windows `FILE_ATTRIBUTE_*` bits the rules read.
pub mod attribute {
  pub const DIRECTORY: u32 = 0x0000_0010;
  pub const REPARSE_POINT: u32 = 0x0000_0400;
  pub const OFFLINE: u32 = 0x0000_1000;
  /// Reported by directory enumeration only. A handle query uses the same bit
  /// for `FILE_ATTRIBUTE_EA`, so strip it from handle results first.
  pub const RECALL_ON_OPEN: u32 = 0x0004_0000;
  pub const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
}

/// Reparse tags of cloud, tiering and virtualization providers other than the
/// Cloud Files family.
const PROVIDER_TAGS: [u32; 8] = [
  0x8000_0021, // ONEDRIVE
  0x8000_0015, // FILE_PLACEHOLDER (Windows 8.1 online-only files)
  0xC000_0004, // HSM
  0x8000_0006, // HSM2
  0x8000_001E, // STORAGE_SYNC (Azure File Sync tiering)
  0x9000_0027, // STORAGE_SYNC_FOLDER
  0x9000_001C, // PROJFS (projected, filled on access)
  0xA000_0022, // PROJFS_TOMBSTONE
];

/// Reparse tags whose data lives on this volume.
const LOCAL_TAGS: [u32; 9] = [
  0x8000_0013, // DEDUP
  0x8000_0017, // WOF (system compression)
  0x8000_0018, // WCI (container layers)
  0x9000_1018, // WCI_1
  0x8000_001B, // APPEXECLINK
  0x8000_0023, // AF_UNIX
  0x8000_0024, // LX_FIFO
  0x8000_0025, // LX_CHR
  0x8000_0026, // LX_BLK
];

/// `IO_REPARSE_TAG_CLOUD` and its sixteen variants (`0x9000?01A`), the tags of
/// every Cloud Files sync provider, hydrated or not.
pub fn is_cloud_files_tag(tag: u32) -> bool {
  tag & 0xFFFF_0FFF == 0x9000_001A
}

/// What a Windows entry's attributes and reparse tag say about it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsEntry {
  Local,
  /// A file whose data is not here; reading it would download it.
  OnlineOnly,
  /// Belongs to a cloud provider: a hydrated cloud file, or any cloud folder
  /// (listing an unpopulated one asks the provider to fill it).
  Cloud,
  /// A symbolic link, junction, volume mount point or other name surrogate.
  Link,
  /// A reparse point this scanner does not know; excluded rather than guessed.
  UnknownReparse,
}

pub fn classify_windows(attributes: u32, reparse_tag: u32) -> WindowsEntry {
  let recall = attribute::OFFLINE | attribute::RECALL_ON_OPEN | attribute::RECALL_ON_DATA_ACCESS;
  if attributes & recall != 0 {
    return if attributes & attribute::DIRECTORY != 0 {
      WindowsEntry::Cloud
    } else {
      WindowsEntry::OnlineOnly
    };
  }
  if attributes & attribute::REPARSE_POINT == 0 {
    return WindowsEntry::Local;
  }
  if is_cloud_files_tag(reparse_tag) || PROVIDER_TAGS.contains(&reparse_tag) {
    WindowsEntry::Cloud
  } else if reparse_tag & 0x2000_0000 != 0 {
    WindowsEntry::Link
  } else if LOCAL_TAGS.contains(&reparse_tag) {
    WindowsEntry::Local
  } else {
    WindowsEntry::UnknownReparse
  }
}

/// A folder (or volume) name that a cloud client uses.
pub fn is_cloud_name(name: &str) -> bool {
  let name = name.to_ascii_lowercase();
  matches!(
    name.as_str(),
    "dropbox" | "icloud drive" | "iclouddrive" | "icloud photos" | "icloudphotos"
  ) || name.starts_with("dropbox (")
    || name.starts_with("onedrive")
    || name.starts_with("googledrive")
    || name.starts_with("google drive")
}

/// The name rules, plus caller-declared cloud roots. Purely lexical, so it can
/// run before the path is touched at all.
pub fn is_cloud_path(path: &Path, custom: &[PathBuf]) -> bool {
  if custom.iter().any(|root| starts_with(path, root)) {
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
  }) || names.iter().any(|name| is_cloud_name(name))
}

#[cfg(not(windows))]
fn starts_with(path: &Path, root: &Path) -> bool {
  path.starts_with(root)
}

/// Windows names are case-insensitive, and one folder can be spelled with or
/// without the `\\?\` prefix.
#[cfg(windows)]
fn starts_with(path: &Path, root: &Path) -> bool {
  fn fold(path: &Path) -> PathBuf {
    let text = path.to_string_lossy().to_lowercase();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
  }
  fold(path).starts_with(fold(root))
}

#[cfg(test)]
mod tests {
  use super::*;
  use attribute::*;

  #[test]
  fn recall_attributes_make_files_online_only_and_folders_cloud() {
    for bit in [OFFLINE, RECALL_ON_OPEN, RECALL_ON_DATA_ACCESS] {
      assert_eq!(
        classify_windows(bit, 0),
        WindowsEntry::OnlineOnly,
        "{bit:#x}"
      );
      assert_eq!(
        classify_windows(bit | DIRECTORY, 0),
        WindowsEntry::Cloud,
        "{bit:#x}"
      );
    }
    // The recall bits win over any reparse tag.
    assert_eq!(
      classify_windows(RECALL_ON_DATA_ACCESS | REPARSE_POINT, 0x9000_601A),
      WindowsEntry::OnlineOnly
    );
  }

  #[test]
  fn every_cloud_files_tag_and_provider_tag_is_cloud_even_when_hydrated() {
    for variant in 0..16u32 {
      let tag = 0x9000_001A | (variant << 12);
      assert_eq!(
        classify_windows(REPARSE_POINT, tag),
        WindowsEntry::Cloud,
        "{tag:#x}"
      );
      assert_eq!(
        classify_windows(REPARSE_POINT | DIRECTORY, tag),
        WindowsEntry::Cloud,
        "{tag:#x}"
      );
    }
    for tag in PROVIDER_TAGS {
      assert_eq!(
        classify_windows(REPARSE_POINT, tag),
        WindowsEntry::Cloud,
        "{tag:#x}"
      );
    }
  }

  #[test]
  fn links_local_reparse_points_and_unknown_tags_are_told_apart() {
    // Symbolic link, junction / mount point, WSL symlink.
    for tag in [0xA000_000C, 0xA000_0003, 0xA000_001D] {
      assert_eq!(
        classify_windows(REPARSE_POINT, tag),
        WindowsEntry::Link,
        "{tag:#x}"
      );
    }
    for tag in LOCAL_TAGS {
      assert_eq!(
        classify_windows(REPARSE_POINT, tag),
        WindowsEntry::Local,
        "{tag:#x}"
      );
    }
    // DFS, NFS and an invented tag fail closed.
    for tag in [0x8000_000A, 0x8000_0014, 0x0000_1234, 0] {
      assert_eq!(
        classify_windows(REPARSE_POINT, tag),
        WindowsEntry::UnknownReparse,
        "{tag:#x}"
      );
    }
  }

  #[test]
  fn ordinary_attributes_are_local_and_a_stale_tag_is_ignored() {
    const PINNED_ARCHIVE_COMPRESSED: u32 = 0x0008_0000 | 0x20 | 0x800;
    assert_eq!(classify_windows(0x20, 0), WindowsEntry::Local);
    assert_eq!(classify_windows(DIRECTORY, 0), WindowsEntry::Local);
    assert_eq!(
      classify_windows(PINNED_ARCHIVE_COMPRESSED, 0x9000_001A),
      WindowsEntry::Local
    );
  }

  #[test]
  fn cloud_client_names_match_anywhere_in_a_path() {
    for path in [
      "/home/me/OneDrive/a",
      "/home/me/OneDrive - Contoso",
      "/home/me/Dropbox",
      "/home/me/Dropbox (Personal)/a",
      "/home/me/iCloudDrive/a",
      "/home/me/Pictures/iCloud Photos",
      "/home/me/Google Drive/a",
      "/home/me/GoogleDrive-me@example.com",
      "/home/me/Library/CloudStorage/x",
      "/home/me/Library/Mobile Documents",
    ] {
      assert!(is_cloud_path(Path::new(path), &[]), "{path}");
    }
    for path in [
      "/home/me/Documents",
      "/home/me/dropbox-clone/src",
      "/home/me/Library/Caches",
      "/home/me/CloudStorage",
    ] {
      assert!(!is_cloud_path(Path::new(path), &[]), "{path}");
    }
    let custom = [PathBuf::from("/home/me/sync")];
    assert!(is_cloud_path(Path::new("/home/me/sync/a"), &custom));
    assert!(!is_cloud_path(Path::new("/home/me/synced"), &custom));
  }

  #[cfg(windows)]
  #[test]
  fn windows_cloud_paths_ignore_case_and_the_verbatim_prefix() {
    for path in [
      r"C:\Users\me\ONEDRIVE\a",
      r"\\?\C:\Users\me\iCloudDrive",
      r"G:\Shared\Google Drive",
    ] {
      assert!(is_cloud_path(Path::new(path), &[]), "{path}");
    }
    let custom = [PathBuf::from(r"C:\Users\me\Sync")];
    assert!(is_cloud_path(Path::new(r"\\?\c:\users\me\sync\a"), &custom));
    assert!(!is_cloud_path(Path::new(r"C:\Users\me\Other"), &custom));
  }
}
