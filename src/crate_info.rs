//! Rust crate information helpers

use std::fmt::{Display, Formatter, Result};
use std::path::PathBuf;

/// Crate download API endpoint suffix
const DOWNLOAD_API_ENDPOINT: &str = "/download";

/// Rust crate information structure
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrateInfo {
    name: String,
    version: String,
}

impl Display for CrateInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "{} v{}", self.name, self.version)
    }
}

impl CrateInfo {
    /// Creates a new crate information object.
    #[must_use]
    pub fn new(name: &str, version: &str) -> Self {
        CrateInfo {
            name: name.to_owned(),
            version: version.to_owned(),
        }
    }

    /// Gets the crate name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Extracts crate information from the download API URL path.
    #[must_use]
    pub fn try_from_download_url(url: &str) -> Option<Self> {
        let name_version = url.strip_suffix(DOWNLOAD_API_ENDPOINT)?;

        let mut i = name_version.split('/');
        match (i.next(), i.next(), i.next()) {
            (Some(name), Some(version), None) => Some(CrateInfo::new(name, version)),
            _ => None,
        }
    }

    /// Builds the crate download URL (relative).
    #[must_use]
    pub fn to_download_url(&self) -> String {
        format!(
            "{name}/{version}{DOWNLOAD_API_ENDPOINT}",
            name = self.name,
            version = self.version
        )
    }

    /// Builds the crate file name for cache storage.
    #[must_use]
    pub fn to_file_name(&self) -> String {
        format!("{}-{}.crate", self.name, self.version)
    }

    /// Builds the relative crate file path for cache storage.
    #[must_use]
    pub fn to_file_path(&self) -> PathBuf {
        PathBuf::from(self.name()).join(self.to_file_name())
    }
}
