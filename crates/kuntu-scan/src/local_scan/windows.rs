//! Windows backend of the protected scanner.
//!
//! Directories are pinned by handle and opened one name at a time relative to
//! their parent, never following a reparse point and never recalling. Children
//! are described by their parent's directory listing, so a file is not opened
//! at all (the one exception is a local `.gitignore`), and a cloud placeholder
//! is recognised from that listing before anything touches it.

use super::{policy_error, Facts, ScanVolume};
use crate::locality::{attribute, classify_windows, is_cloud_name, WindowsEntry};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{Arc, Mutex};
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
  NtCreateFile, RtlSetThreadPlaceholderCompatibilityMode, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
  FILE_OPEN_FOR_BACKUP_INTENT, FILE_OPEN_NO_RECALL, FILE_OPEN_REPARSE_POINT,
  FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
  RtlNtStatusToDosError, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
  ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

/// `PHCM_EXPOSE_PLACEHOLDERS`: show this thread the real placeholder attributes
/// and reparse tags instead of the disguise ordinary programs get.
const EXPOSE_PLACEHOLDERS: i8 = 2;

const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const TRAVERSE: u32 = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const LIST: u32 = TRAVERSE | FILE_LIST_DIRECTORY;
const DESCRIBE: u32 = FILE_READ_ATTRIBUTES | SYNCHRONIZE;
/// Every relative open: the entry itself, never its target, never a download.
/// `FILE_DIRECTORY_FILE` is left out because the I/O manager rejects it next to
/// `FILE_OPEN_NO_RECALL`; the kind is checked on the opened handle instead.
const OPEN_OPTIONS: u32 = FILE_OPEN_REPARSE_POINT
  | FILE_OPEN_NO_RECALL
  | FILE_OPEN_FOR_BACKUP_INTENT
  | FILE_SYNCHRONOUS_IO_NONALERT;

/// `GetDriveTypeW` results that name storage in this machine.
const LOCAL_DRIVE_TYPES: [u32; 4] = [
  2, // removable
  3, // fixed
  5, // optical
  6, // RAM disk
];
const LOCAL_FILESYSTEMS: [&str; 6] = ["NTFS", "ReFS", "FAT", "FAT32", "exFAT", "UDF"];

pub(super) fn expose_placeholders() -> io::Result<i8> {
  let previous = unsafe { RtlSetThreadPlaceholderCompatibilityMode(EXPOSE_PLACEHOLDERS) };
  if previous < 0 {
    return Err(io::Error::new(
      io::ErrorKind::PermissionDenied,
      "cloud placeholders could not be exposed to the scanning thread",
    ));
  }
  Ok(previous)
}

pub(super) fn restore_placeholder_mode(previous: i8) -> io::Result<()> {
  if unsafe { RtlSetThreadPlaceholderCompatibilityMode(previous) } < 0 {
    return Err(io::Error::other(
      "the thread's placeholder mode could not be restored",
    ));
  }
  Ok(())
}

#[derive(Clone)]
pub(super) struct Directory {
  handle: Arc<OwnedHandle>,
  /// NTFS and ReFS keep one file id per file; FAT ids are not comparable
  /// between a listing and a handle.
  stable_ids: bool,
  /// What the last listing of this directory said about each child.
  listed: Arc<Mutex<HashMap<OsString, Facts>>>,
}

impl Directory {
  fn new(handle: OwnedHandle, stable_ids: bool) -> Self {
    Self {
      handle: Arc::new(handle),
      stable_ids,
      listed: Arc::default(),
    }
  }
  fn raw(&self) -> HANDLE {
    self.handle.as_raw_handle() as HANDLE
  }
}

fn facts(
  device: u64,
  inode: u128,
  allocated: i64,
  logical: i64,
  attributes: u32,
  reparse_tag: u32,
) -> Facts {
  let entry = classify_windows(attributes, reparse_tag);
  let directory = attributes & attribute::DIRECTORY != 0;
  Facts {
    device,
    inode,
    allocated: allocated.max(0) as u64,
    logical: logical.max(0) as u64,
    directory: directory && entry != WindowsEntry::Link,
    file: !directory && entry == WindowsEntry::Local,
    symlink: entry == WindowsEntry::Link,
    dataless: entry == WindowsEntry::OnlineOnly,
    excluded: match entry {
      WindowsEntry::Cloud => Some((
        "cloud-root",
        "cloud item excluded by its attributes or reparse tag",
      )),
      WindowsEntry::UnknownReparse => {
        Some(("reparse-point", "unrecognised reparse point excluded"))
      }
      _ => None,
    },
  }
}

/// Refuses anything a scan must not open or read.
pub(super) fn require_local(facts: &Facts) -> io::Result<()> {
  if facts.dataless {
    return Err(policy_error("dataless", "online-only item was not opened"));
  }
  if facts.symlink {
    return Err(policy_error(
      "symlink",
      "links and junctions are never followed",
    ));
  }
  if let Some((reason, message)) = facts.exclusion() {
    return Err(policy_error(reason, message));
  }
  Ok(())
}

fn query<T>(handle: HANDLE, class: FILE_INFO_BY_HANDLE_CLASS) -> io::Result<T> {
  let mut value = std::mem::MaybeUninit::<T>::zeroed();
  // SAFETY: the buffer is exactly one `T`, the structure `class` fills.
  let ok = unsafe {
    GetFileInformationByHandleEx(
      handle,
      class,
      value.as_mut_ptr().cast(),
      size_of::<T>() as u32,
    )
  };
  if ok == 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: zero-initialised plain data, filled by the successful call.
  Ok(unsafe { value.assume_init() })
}

/// Volume serial and file id of an open handle.
fn identity(handle: HANDLE) -> io::Result<(u64, u128)> {
  if let Ok(info) = query::<FILE_ID_INFO>(handle, FileIdInfo) {
    return Ok((
      info.VolumeSerialNumber,
      file_id(u128::from_le_bytes(info.FileId.Identifier)),
    ));
  }
  let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
  // SAFETY: the output pointer refers to one writable structure.
  if unsafe { GetFileInformationByHandle(handle, info.as_mut_ptr()) } == 0 {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: filled by the successful call.
  let info = unsafe { info.assume_init() };
  Ok((
    info.dwVolumeSerialNumber as u64,
    file_id(((info.nFileIndexHigh as u128) << 32) | info.nFileIndexLow as u128),
  ))
}

/// Zero means "no usable identity".
fn file_id(raw: u128) -> u128 {
  if raw == u128::MAX || raw == u64::MAX as u128 {
    0
  } else {
    raw
  }
}

fn handle_facts(handle: &OwnedHandle) -> io::Result<Facts> {
  let handle = handle.as_raw_handle() as HANDLE;
  let tag = query::<FILE_ATTRIBUTE_TAG_INFO>(handle, FileAttributeTagInfo)?;
  let sizes = query::<FILE_STANDARD_INFO>(handle, FileStandardInfo)?;
  let (device, inode) = identity(handle)?;
  Ok(facts(
    device,
    inode,
    sizes.AllocationSize,
    sizes.EndOfFile,
    tag.FileAttributes & !attribute::RECALL_ON_OPEN,
    tag.ReparseTag,
  ))
}

/// Opens one name inside `parent`.
fn open_child(
  parent: HANDLE,
  name: &OsStr,
  access: u32,
  options: u32,
  case_insensitive: bool,
) -> io::Result<OwnedHandle> {
  let mut wide: Vec<u16> = name.encode_wide().collect();
  let bytes = u16::try_from(wide.len() * 2)
    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "file name is too long"))?;
  if wide.is_empty() || wide.contains(&(b'\\' as u16)) || wide.contains(&0) {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "not a single path component",
    ));
  }
  let object_name = UNICODE_STRING {
    Length: bytes,
    MaximumLength: bytes,
    Buffer: wide.as_mut_ptr(),
  };
  let attributes = OBJECT_ATTRIBUTES {
    Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
    RootDirectory: parent,
    ObjectName: &object_name,
    Attributes: if case_insensitive {
      OBJ_CASE_INSENSITIVE
    } else {
      0
    },
    SecurityDescriptor: std::ptr::null(),
    SecurityQualityOfService: std::ptr::null(),
  };
  let mut handle: HANDLE = std::ptr::null_mut();
  let mut status_block = IO_STATUS_BLOCK::default();
  // SAFETY: every pointer refers to a live local for the duration of the call.
  let status = unsafe {
    NtCreateFile(
      &mut handle,
      access,
      &attributes,
      &mut status_block,
      std::ptr::null(),
      0,
      SHARE_ALL,
      FILE_OPEN,
      OPEN_OPTIONS | options,
      std::ptr::null(),
      0,
    )
  };
  if status < 0 {
    // SAFETY: a pure status translation.
    let code = unsafe { RtlNtStatusToDosError(status) };
    return Err(io::Error::from_raw_os_error(code as i32));
  }
  // SAFETY: a successful open returns a handle this process now owns.
  Ok(unsafe { OwnedHandle::from_raw_handle(handle as _) })
}

fn terminated(text: &OsStr) -> Vec<u16> {
  text.encode_wide().chain(Some(0)).collect()
}

fn until_nul(buffer: &[u16]) -> String {
  let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(0);
  String::from_utf16_lossy(&buffer[..end])
}

fn not_local(message: &str) -> io::Error {
  policy_error("non-local-volume", message)
}

struct Drive {
  root: PathBuf,
  stable_ids: bool,
}

/// Verifies that `path` is on a local disk, from the drive letter and the
/// volume's own description; nothing below the drive root is touched.
fn local_drive(path: &Path) -> io::Result<Drive> {
  let mut components = path.components();
  let letter = match components.next() {
    Some(Component::Prefix(prefix)) => match prefix.kind() {
      Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
      _ => return Err(not_local("network and device paths are excluded")),
    },
    _ => return Err(not_local("the path does not name a drive")),
  };
  if components.next() != Some(Component::RootDir) {
    return Err(not_local("the path does not name a drive"));
  }
  let root = PathBuf::from(format!("{}:\\", char::from(letter.to_ascii_uppercase())));
  let wide = terminated(root.as_os_str());
  // SAFETY: a terminated string that outlives the call.
  let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
  if kind == 1 {
    return Err(io::Error::new(io::ErrorKind::NotFound, "no such drive"));
  }
  if !LOCAL_DRIVE_TYPES.contains(&kind) {
    return Err(not_local("network or unknown drive excluded"));
  }
  let mut label = [0u16; 261];
  let mut filesystem = [0u16; 261];
  // SAFETY: both buffers are writable for the lengths passed.
  let ok = unsafe {
    GetVolumeInformationW(
      wide.as_ptr(),
      label.as_mut_ptr(),
      label.len() as u32,
      std::ptr::null_mut(),
      std::ptr::null_mut(),
      std::ptr::null_mut(),
      filesystem.as_mut_ptr(),
      filesystem.len() as u32,
    )
  };
  if ok == 0 {
    return Err(io::Error::last_os_error());
  }
  // Google Drive mounts a drive letter that otherwise looks like a FAT disk.
  if is_cloud_name(&until_nul(&label)) {
    return Err(policy_error("cloud-root", "cloud drive excluded"));
  }
  let filesystem = until_nul(&filesystem);
  if !LOCAL_FILESYSTEMS.contains(&filesystem.as_str()) {
    return Err(not_local("network or unknown filesystem excluded"));
  }
  Ok(Drive {
    root,
    stable_ids: filesystem == "NTFS" || filesystem == "ReFS",
  })
}

fn open_drive(drive: &Drive, access: u32) -> io::Result<OwnedHandle> {
  let wide = terminated(drive.root.as_os_str());
  // SAFETY: a terminated string and no optional pointers.
  let raw = unsafe {
    CreateFileW(
      wide.as_ptr(),
      access,
      SHARE_ALL,
      std::ptr::null(),
      OPEN_EXISTING,
      FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
      std::ptr::null_mut(),
    )
  };
  if raw == INVALID_HANDLE_VALUE {
    return Err(io::Error::last_os_error());
  }
  // SAFETY: a successful open returns a handle this process now owns.
  let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
  // Only a network filesystem can answer this query.
  if query::<FILE_REMOTE_PROTOCOL_INFO>(handle.as_raw_handle() as HANDLE, FileRemoteProtocolInfo)
    .is_ok()
  {
    return Err(not_local("network drive excluded"));
  }
  // A substituted drive (`subst`) is an alias of a folder somewhere else, whose
  // ancestors this walk would never check.
  let mut target = [0u16; 512];
  // SAFETY: the buffer is writable for the length passed.
  let length = unsafe {
    GetFinalPathNameByHandleW(
      raw,
      target.as_mut_ptr(),
      target.len() as u32,
      FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
    )
  };
  if length == 0 || length as usize >= target.len() {
    return Err(not_local("the drive's location could not be verified"));
  }
  let target = String::from_utf16_lossy(&target[..length as usize]);
  let plain = target.strip_prefix(r"\\?\").unwrap_or(&target);
  if plain.len() != 3 || !plain.ends_with(r":\") {
    return Err(not_local(
      "substituted drive excluded; scan the folder it points to",
    ));
  }
  Ok(handle)
}

/// Opens `path` component by component from its drive root.
pub(super) fn open_root(
  path: &Path,
  cancelled: &dyn Fn() -> io::Result<()>,
) -> io::Result<(Directory, Facts, ScanVolume)> {
  let drive = local_drive(path)?;
  let names: Vec<&OsStr> = path
    .components()
    .filter_map(|component| match component {
      Component::Normal(name) => Some(name),
      _ => None,
    })
    .collect();
  let mut handle = open_drive(&drive, if names.is_empty() { LIST } else { TRAVERSE })?;
  let mut current = handle_facts(&handle)?;
  require_local(&current)?;
  for (index, name) in names.iter().enumerate() {
    cancelled()?;
    let parent = handle.as_raw_handle() as HANDLE;
    // Describe first: nothing but a verified local folder is opened for use.
    let described = handle_facts(&open_child(parent, name, DESCRIBE, 0, true)?)?;
    require_local(&described)?;
    if !described.directory {
      return Err(io::Error::new(
        io::ErrorKind::NotADirectory,
        "scan roots must be folders",
      ));
    }
    let access = if index + 1 == names.len() {
      LIST
    } else {
      TRAVERSE
    };
    let next = open_child(parent, name, access, 0, true)?;
    current = handle_facts(&next)?;
    require_local(&current)?;
    if !current.directory || current.inode != described.inode {
      return Err(policy_error(
        "changed-identity",
        "directory changed between metadata and open",
      ));
    }
    handle = next;
  }
  let volume = volume(path, &drive)?;
  Ok((Directory::new(handle, drive.stable_ids), current, volume))
}

fn volume(path: &Path, drive: &Drive) -> io::Result<ScanVolume> {
  let wide = terminated(drive.root.as_os_str());
  let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
  // SAFETY: a terminated string and three writable u64 outputs.
  if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
    return Err(io::Error::last_os_error());
  }
  Ok(ScanVolume {
    path: path.to_path_buf(),
    total_bytes: total,
    available_bytes: available,
    free_bytes: free,
    is_local: true,
  })
}

fn name_of(path: &Path) -> io::Result<&OsStr> {
  path
    .file_name()
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing path component"))
}

pub(super) fn metadata(path: &Path, parent: &Directory) -> io::Result<Facts> {
  let name = name_of(path)?;
  if let Some(facts) = parent.listed.lock().unwrap().get(name) {
    return Ok(*facts);
  }
  handle_facts(&open_child(parent.raw(), name, DESCRIBE, 0, false)?)
}

fn require_same(directory: &Directory, current: &Facts, expected: &Facts) -> io::Result<()> {
  require_local(current)?;
  let replaced = current.device != expected.device
    || directory.stable_ids && expected.inode != 0 && current.inode != expected.inode;
  if replaced {
    return Err(policy_error(
      "changed-identity",
      "entry changed between metadata and open",
    ));
  }
  Ok(())
}

pub(super) fn directory(path: &Path, parent: &Directory, expected: Facts) -> io::Result<Directory> {
  let handle = open_child(parent.raw(), name_of(path)?, LIST, 0, false)?;
  let current = handle_facts(&handle)?;
  let opened = Directory::new(handle, parent.stable_ids);
  require_same(parent, &current, &expected)?;
  if !current.directory {
    return Err(policy_error(
      "changed-identity",
      "directory was replaced by a file",
    ));
  }
  Ok(opened)
}

/// One listing record: where its name is and what it says.
struct Record {
  next: usize,
  name: OsString,
  facts: Facts,
}

/// Reads the record at `offset` of a `FILE_ID_EXTD_DIR_INFO` listing, or of a
/// `FILE_ID_BOTH_DIR_INFO` one where the filesystem lacks the former.
fn record(buffer: &[u64], offset: usize, extended: bool, device: u64) -> io::Result<Record> {
  let bytes = buffer.len() * 8;
  let header = if extended {
    offset_of!(FILE_ID_EXTD_DIR_INFO, FileName)
  } else {
    offset_of!(FILE_ID_BOTH_DIR_INFO, FileName)
  };
  let malformed = || io::Error::new(io::ErrorKind::InvalidData, "malformed directory listing");
  if !offset.is_multiple_of(8) || offset + header > bytes {
    return Err(malformed());
  }
  // SAFETY: `offset + header` is inside the buffer; fields are read by value
  // through raw pointers, without forming a reference to the whole structure.
  unsafe {
    let base = buffer.as_ptr().cast::<u8>().add(offset);
    let (next, length, attributes, tag, id, allocated, logical) = if extended {
      let entry = base.cast::<FILE_ID_EXTD_DIR_INFO>();
      let id = std::ptr::addr_of!((*entry).FileId).read_unaligned();
      (
        std::ptr::addr_of!((*entry).NextEntryOffset).read_unaligned(),
        std::ptr::addr_of!((*entry).FileNameLength).read_unaligned(),
        std::ptr::addr_of!((*entry).FileAttributes).read_unaligned(),
        std::ptr::addr_of!((*entry).ReparsePointTag).read_unaligned(),
        u128::from_le_bytes(id.Identifier),
        std::ptr::addr_of!((*entry).AllocationSize).read_unaligned(),
        std::ptr::addr_of!((*entry).EndOfFile).read_unaligned(),
      )
    } else {
      let entry = base.cast::<FILE_ID_BOTH_DIR_INFO>();
      (
        std::ptr::addr_of!((*entry).NextEntryOffset).read_unaligned(),
        std::ptr::addr_of!((*entry).FileNameLength).read_unaligned(),
        std::ptr::addr_of!((*entry).FileAttributes).read_unaligned(),
        // A reparse point's tag is carried in the extended-attribute size.
        std::ptr::addr_of!((*entry).EaSize).read_unaligned(),
        std::ptr::addr_of!((*entry).FileId).read_unaligned() as u64 as u128,
        std::ptr::addr_of!((*entry).AllocationSize).read_unaligned(),
        std::ptr::addr_of!((*entry).EndOfFile).read_unaligned(),
      )
    };
    let length = length as usize;
    if !length.is_multiple_of(2) || offset + header + length > bytes {
      return Err(malformed());
    }
    let name = std::slice::from_raw_parts(base.add(header).cast::<u16>(), length / 2);
    Ok(Record {
      next: next as usize,
      name: OsString::from_wide(name),
      facts: facts(device, file_id(id), allocated, logical, attributes, tag),
    })
  }
}

pub(super) fn entries(
  path: &Path,
  directory: &Directory,
  cancelled: &dyn Fn() -> io::Result<()>,
) -> io::Result<Vec<io::Result<PathBuf>>> {
  let current = handle_facts(&directory.handle)?;
  require_local(&current)?;
  // 64 KiB, aligned for the 64-bit fields of a listing record.
  let mut buffer = vec![0u64; 8 * 1024];
  let mut extended = true;
  let mut restart = true;
  let mut entries = Vec::new();
  let mut listed = HashMap::new();
  loop {
    cancelled()?;
    let class = match (extended, restart) {
      (true, true) => FileIdExtdDirectoryRestartInfo,
      (true, false) => FileIdExtdDirectoryInfo,
      (false, true) => FileIdBothDirectoryRestartInfo,
      (false, false) => FileIdBothDirectoryInfo,
    };
    // SAFETY: the buffer is writable for its full length.
    let ok = unsafe {
      GetFileInformationByHandleEx(
        directory.raw(),
        class,
        buffer.as_mut_ptr().cast(),
        (buffer.len() * 8) as u32,
      )
    };
    if ok == 0 {
      let error = io::Error::last_os_error();
      let code = error.raw_os_error().unwrap_or_default() as u32;
      if code == ERROR_NO_MORE_FILES {
        break;
      }
      if extended
        && restart
        && [
          ERROR_INVALID_PARAMETER,
          ERROR_INVALID_FUNCTION,
          ERROR_NOT_SUPPORTED,
        ]
        .contains(&code)
      {
        extended = false;
        continue;
      }
      // An empty root directory reports "file not found" on FAT volumes.
      if restart && error.kind() == io::ErrorKind::NotFound {
        break;
      }
      if restart {
        return Err(error);
      }
      entries.push(Err(error));
      break;
    }
    restart = false;
    let mut offset = 0;
    loop {
      let entry = record(&buffer, offset, extended, current.device)?;
      if entry.name != "." && entry.name != ".." {
        entries.push(Ok(path.join(&entry.name)));
        listed.insert(entry.name, entry.facts);
      }
      if entry.next == 0 {
        break;
      }
      offset += entry.next;
    }
  }
  *directory.listed.lock().unwrap() = listed;
  Ok(entries)
}

pub(super) fn ignore_text(path: &Path, parent: &Directory, expected: Facts) -> io::Result<String> {
  let handle = open_child(
    parent.raw(),
    name_of(path)?,
    FILE_READ_DATA | DESCRIBE,
    FILE_NON_DIRECTORY_FILE,
    false,
  )?;
  let current = handle_facts(&handle)?;
  require_same(parent, &current, &expected)?;
  if !current.file {
    return Err(policy_error(
      "changed-identity",
      "ignore rules changed identity, kind or filesystem",
    ));
  }
  let mut text = String::new();
  std::fs::File::from(handle)
    .take(1024 * 1024 + 1)
    .read_to_string(&mut text)?;
  if text.len() > 1024 * 1024 {
    return Err(io::Error::new(
      io::ErrorKind::InvalidData,
      ".gitignore exceeds the protected 1 MiB limit",
    ));
  }
  Ok(text)
}

#[cfg(test)]
mod tests;
