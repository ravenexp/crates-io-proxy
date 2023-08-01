//! Sparse registry configuration file helpers

use super::{ProxyConfig, CRATES_API_PATH};

/// Registry configuration file endpoint path
const CONFIG_JSON_ENDPOINT: &str = "config.json";

/// Checks for the registry configuration file download endpoint.
#[must_use]
pub fn is_config_json_url(index_url: &str) -> bool {
    index_url == CONFIG_JSON_ENDPOINT
}

/// Dynamically generates the registry configuration file contents.
#[must_use]
pub(super) fn gen_config_json_file(config: &ProxyConfig) -> String {
    // Generate the crate download API URL pointing to this same proxy server.
    let dl_url = config
        .proxy_url
        .join(CRATES_API_PATH)
        .expect("invalid proxy server URL");

    // Cargo can not handle trailing slashes in `config.json`.
    let dl = dl_url.as_str().trim_end_matches('/');
    let api = config.upstream_url.as_str().trim_end_matches('/');

    format!(r#"{{"dl":"{dl}","api":"{api}"}}"#)
}
