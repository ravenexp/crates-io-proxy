//! Index entry file metadata cache helpers

use std::collections::BTreeMap;
use std::sync::RwLock;

use super::IndexEntry;

/// Volatile registry index entry metadata cache
static INDEX_CACHE: RwLock<BTreeMap<String, IndexEntry>> = RwLock::new(BTreeMap::new());

/// Caches the index entry metadata in memory.
pub fn metadata_store_index_entry(entry: &IndexEntry) {
    let name = entry.name().to_owned();

    INDEX_CACHE.write().unwrap().insert(name, entry.clone());
}

/// Fetches the cached index entry metadata from memory.
pub fn metadata_fetch_index_entry(name: &str) -> Option<IndexEntry> {
    INDEX_CACHE.read().unwrap().get(name).map(ToOwned::to_owned)
}

/// Erases the cached index entry metadata from memory.
pub fn metadata_invalidate_index_entry(entry: &IndexEntry) {
    INDEX_CACHE.write().unwrap().remove(entry.name());
}
