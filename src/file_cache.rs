//! Index entry and crate file cache helpers

use std::fs::{create_dir_all, read, write};
use std::path::Path;

use log::error;

use super::{CrateInfo, IndexEntry};

/// Caches the crate package file on the local filesystem.
pub fn cache_store_crate(dir: &Path, crate_info: &CrateInfo, data: &[u8]) {
    let crate_file_path = dir.join(crate_info.to_file_path());

    // Create all parent directories first.
    if let Err(e) = create_dir_all(crate_file_path.parent().unwrap()) {
        error!("cache: failed to create crate directory: {e}");
        return;
    }

    write(crate_file_path, data)
        .unwrap_or_else(|e| error!("cache: failed to write crate file: {e}"));
}

/// Fetches the cached crate package file from the local filesystem, if present.
pub fn cache_fetch_crate(dir: &Path, crate_info: &CrateInfo) -> Option<Vec<u8>> {
    read(dir.join(crate_info.to_file_path())).ok()
}

/// Caches the index entry file on the local filesystem.
pub fn cache_store_index_entry(dir: &Path, entry: &IndexEntry, data: &[u8]) {
    let entry_file_path = dir.join(entry.to_file_path());

    if let Err(e) = create_dir_all(entry_file_path.parent().unwrap()) {
        error!("cache: failed to create index directory: {e}");
        return;
    }

    write(entry_file_path, data)
        .unwrap_or_else(|e| error!("cache: failed to write index entry file: {e}"));
}

/// Fetches the cached index entry file from the local filesystem, if present.
pub fn cache_fetch_index_entry(dir: &Path, entry: &IndexEntry) -> Option<Vec<u8>> {
    read(dir.join(entry.to_file_path())).ok()
}
