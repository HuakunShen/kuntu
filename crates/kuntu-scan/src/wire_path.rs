//! How paths leave in JSON. Names need not be valid Unicode (unpaired UTF-16
//! surrogates on Windows, arbitrary bytes on Unix) and one such name must not
//! fail a whole report, so it is spelled with replacement characters. Paths in
//! memory stay exact.

use serde::Serializer;
use std::path::{Path, PathBuf};

pub fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
  serializer.serialize_str(&path.to_string_lossy())
}

pub fn serialize_option<S: Serializer>(
  path: &Option<PathBuf>,
  serializer: S,
) -> Result<S::Ok, S::Error> {
  match path {
    Some(path) => serializer.serialize_some(&path.to_string_lossy()),
    None => serializer.serialize_none(),
  }
}
