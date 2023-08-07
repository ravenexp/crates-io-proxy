//! Registry index entry handling helpers

use std::fmt::{Display, Formatter, Result};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use httpdate::{fmt_http_date, parse_http_date};

/// Registry index entry structure
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct IndexEntry {
    /// Crate name
    name: String,
    /// HTTP entity tag header
    etag: Option<String>,
    /// Index file modification time
    mtime: Option<SystemTime>,
    /// Last index entry update check time
    atime: Option<Instant>,
}

impl Display for IndexEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(&self.name)
    }
}

impl IndexEntry {
    /// Creates a registry index entry object for a crate.
    #[must_use]
    pub fn new(name: &str) -> Self {
        IndexEntry {
            name: name.to_owned(),
            etag: None,
            mtime: None,
            atime: None,
        }
    }

    /// Creates an entry from the sparse index URL path.
    #[must_use]
    pub fn try_from_index_url(url: &str) -> Option<Self> {
        if url.contains('.') {
            return None;
        }

        let mut i = url.split('/');

        match i.next() {
            Some("1" | "2") => match (i.next(), i.next()) {
                (Some(name), None) => Some(IndexEntry::new(name)),
                _ => None,
            },
            _ => match (i.next(), i.next(), i.next()) {
                (Some(_), Some(name), None) => Some(IndexEntry::new(name)),
                _ => None,
            },
        }
    }

    /// Gets the crate name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Checks if this index entry file contents is the same
    /// as `other` according to the associated metadata.
    #[must_use]
    pub fn is_equivalent(&self, other: &IndexEntry) -> bool {
        (self.etag().is_some() && (self.etag() == other.etag()))
            || (self.last_modified().is_some() && (self.last_modified() == other.last_modified()))
    }

    /// Checks if this index entry is expired according for the TTL given.
    #[must_use]
    pub fn is_expired_with_ttl(&self, ttl: &Duration) -> bool {
        self.atime.map_or(false, |atime| atime.elapsed() > *ttl)
    }

    /// Gets the HTTP entity tag metadata.
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// Gets the HTTP Last-Modified metadata.
    pub fn last_modified(&self) -> Option<String> {
        self.mtime.map(fmt_http_date)
    }

    /// Sets the HTTP entity tag metadata.
    pub fn set_etag(&mut self, etag: &str) {
        self.etag = Some(etag.to_owned());
    }

    /// Sets the HTTP Last-Modified metadata.
    pub fn set_last_modified(&mut self, last_modified: &str) {
        self.mtime = parse_http_date(last_modified).ok();
    }

    /// Updates the last upstream server access time metadata.
    pub fn set_last_updated(&mut self) {
        self.atime = Some(Instant::now());
    }

    /// Builds the index entry download URL (relative).
    #[must_use]
    pub fn to_index_url(&self) -> String {
        let name = &self.name;

        match name.len() {
            0 => String::new(),
            sz @ (1 | 2) => format!("{sz}/{name}"),
            3 => format!("3/{first}/{name}", first = &name[..1]),
            _ => format!(
                "{first}/{second}/{name}",
                first = &name[0..2],
                second = &name[2..4]
            ),
        }
    }

    /// Builds the relative index entry file path for cache storage.
    #[must_use]
    pub fn to_file_path(&self) -> PathBuf {
        PathBuf::from(self.to_index_url())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_url() {
        assert_eq!(IndexEntry::try_from_index_url(""), None);
        assert_eq!(IndexEntry::try_from_index_url("abc"), None);
        assert_eq!(IndexEntry::try_from_index_url("a/bc"), None);
        assert_eq!(IndexEntry::try_from_index_url("a/b/c/d"), None);

        assert_eq!(
            IndexEntry::try_from_index_url("1/a"),
            Some(IndexEntry::new("a"))
        );
        assert_eq!(
            IndexEntry::try_from_index_url("2/ab"),
            Some(IndexEntry::new("ab"))
        );
        assert_eq!(
            IndexEntry::try_from_index_url("3/a/abc"),
            Some(IndexEntry::new("abc"))
        );
        assert_eq!(
            IndexEntry::try_from_index_url("ab/cd/abcd"),
            Some(IndexEntry::new("abcd"))
        );
    }

    #[test]
    fn test_to_url() {
        assert_eq!(IndexEntry::new("").to_index_url(), "");
        assert_eq!(IndexEntry::new("a").to_index_url(), "1/a");
        assert_eq!(IndexEntry::new("ab").to_index_url(), "2/ab");
        assert_eq!(IndexEntry::new("abc").to_index_url(), "3/a/abc");
        assert_eq!(IndexEntry::new("abcd").to_index_url(), "ab/cd/abcd");
    }
}
