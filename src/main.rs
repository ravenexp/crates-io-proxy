//! Caching HTTP proxy server for the `crates.io` registry
//! ======================================================
//!
//! `crates-io-proxy` implements transparent caching for both
//! the sparse registry index at <https://index.crates.io/> and
//! the static crate file download server.
//!
//! Two independent HTTP proxy endpoints are implemented:
//!
//! 1. Listens to HTTP GET requests at `/index/.../{crate}`,
//!    forwards them to <https://index.crates.io/> and caches the downloaded registry
//!    index entries as JSON text files on the local filesystem.
//!
//! 2. Listens to HTTP GET requests at `/api/v1/crates/{crate}/{version}/download`,
//!    forwards them to <https://crates.io/> and caches the downloaded crates as
//!    `.crate` files on the local filesystem.
//!
//! Subsequent sparse registry index and crate download API hits are serviced
//! using the locally cached index entry and crate files.
//!
//! As a convenience feature, the download requests for the `config.json` file
//! found at the sparse index root are served with a replacement file,
//! which changes the crate download URL to point to this same proxy server.

mod config_json;
mod crate_info;
mod file_cache;
mod index_entry;
mod metadata_cache;

use std::env;
use std::fmt::Display;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use pico_args::Arguments;

use env_logger::{Builder as LogBuilder, Env as LogEnv};
use log::{debug, error, info, warn};

use tiny_http::{Header, Method, Request, Response, Server};
use url::Url;

use crate::config_json::{gen_config_json_file, is_config_json_url};
use crate::crate_info::CrateInfo;
use crate::file_cache::{
    cache_fetch_crate, cache_fetch_index_entry, cache_store_crate, cache_store_index_entry,
    cache_try_find_index_entry,
};
use crate::index_entry::IndexEntry;
use crate::metadata_cache::{
    metadata_fetch_index_entry, metadata_invalidate_index_entry, metadata_store_index_entry,
};

/// Default listen address and port
const LISTEN_ADDRESS: &str = "0.0.0.0:3080";

/// Upstream `crates.io` registry index URL
const INDEX_CRATES_IO_URL: &str = "https://index.crates.io/";

/// Upstream `crates.io` registry URL
const CRATES_IO_URL: &str = "https://crates.io/";

/// Default external URL of this proxy server
const DEFAULT_PROXY_URL: &str = "http://localhost:3080/";

/// Sparse registry index access path
const CRATES_INDEX_PATH: &str = "/index/";

/// Crates download API path
const CRATES_API_PATH: &str = "/api/v1/crates/";

/// Default crate files cache directory path
const DEFAULT_CACHE_DIR: &str = "/var/cache/crates-io-proxy";

/// Default index cache entry Time-to-Live in seconds
const DEFAULT_CACHE_TTL_SECS: u64 = 3600;

/// Default index entry download buffer capacity
const INDEX_ENTRY_CAPACITY: usize = 0x10000;

/// Limit the download item size to 16 MiB
const MAX_CRATE_SIZE: usize = 0x100_0000;

/// HTTP Content-Type of the registry index entry JSON file
const INDEX_HTTP_CTYPE: &str = "Content-Type: text/plain";

/// HTTP Content-Type of the crate package file
const CRATE_HTTP_CTYPE: &str = "Content-Type: application/x-tar";

/// HTTP Content-Type of the crates API JSON response
const JSON_HTTP_CTYPE: &str = "Content-Type: application/json; charset=utf-8";

/// Program version tag: `"<major>.<minor>.<patch>"`
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// HTTP client User Agent string
const HTTP_USER_AGENT: &str = concat!("crates-io-proxy/", env!("CARGO_PKG_VERSION"));

/// Proxy server configuration
#[derive(Debug, Clone)]
struct ProxyConfig {
    /// Upstream registry index URL (defaults to [`INDEX_CRATES_IO_URL`])
    index_url: Url,

    /// Upstream crate download URL (defaults to [`CRATES_IO_URL`])
    upstream_url: Url,

    /// External URL of this proxy server (defaults to [`DEFAULT_PROXY_URL`])
    proxy_url: Url,

    /// Registry index cache directory (defaults to [`DEFAULT_CACHE_DIR`])
    index_dir: PathBuf,

    /// Crate files cache directory (defaults to [`DEFAULT_CACHE_DIR`])
    crates_dir: PathBuf,

    /// Index entry cache Time-to-Live (defaults to [`DEFAULT_CACHE_TTL_SECS`])
    cache_ttl: Duration,
}

/// Registry index entry download response
struct IndexResponse {
    /// Index entry requested + response metadata
    entry: IndexEntry,

    /// HTTP response status code
    status: u16,

    /// HTTP response data
    data: Vec<u8>,
}

/// Gets the server-global ureq client instance.
///
/// The global agent instance is required to use HTTP request pipelining.
fn ureq_agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();

    AGENT
        .get_or_init(|| ureq::builder().user_agent(HTTP_USER_AGENT).build())
        .clone()
}

/// Makes boxed custom ureq status errors for `download_crate()`.
fn ureq_status_error(status_code: u16, msg: &str) -> Box<ureq::Error> {
    assert!(status_code >= 400);

    Box::new(ureq::Error::Status(
        status_code,
        ureq::Response::new(status_code, msg, msg).unwrap(),
    ))
}

/// Downloads the crate file from the upstream download server
/// (usually <https://crates.io/>).
fn download_crate(site_url: &Url, crate_info: &CrateInfo) -> Result<Vec<u8>, Box<ureq::Error>> {
    let url = site_url
        .join(CRATES_API_PATH)
        .unwrap()
        .join(&crate_info.to_download_url())
        .unwrap();

    let response = ureq_agent()
        .request_url("GET", &url)
        .call()
        .map_err(Box::new)?;

    if let Some(content_len) = response.header("Content-Length") {
        let Ok(len) = content_len.parse::<usize>() else {
            return Err(ureq_status_error(400, "Invalid header"));
        };

        if len > MAX_CRATE_SIZE {
            return Err(ureq_status_error(507, "Insufficient storage"));
        }

        let mut data: Vec<u8> = Vec::with_capacity(len);
        response
            .into_reader()
            .read_to_end(&mut data)
            .map_err(|e| Box::new(e.into()))?;

        Ok(data)
    } else {
        // Got no "Content-Length" header, most likely because "Transfer-Encoding: chunked"
        // is being sent by the server (crates.io servers do not do this).
        //
        // Using an arbitrary initial estimate for the total response size...
        let mut data: Vec<u8> = Vec::with_capacity(MAX_CRATE_SIZE / 256);

        response
            .into_reader()
            .take(MAX_CRATE_SIZE as u64)
            .read_to_end(&mut data)
            .map_err(|e| Box::new(e.into()))?;

        // Abort download here if the crate file has been truncated by
        // the `reader.take()` limit above.
        if data.len() >= MAX_CRATE_SIZE {
            return Err(ureq_status_error(507, "Insufficient storage"));
        }

        Ok(data)
    }
}

/// Downloads the sparse index entry from the upstream registry.
/// (usually <https://index.crates.io/>).
fn download_index_entry(
    index_url: &Url,
    mut entry: IndexEntry,
) -> Result<IndexResponse, Box<ureq::Error>> {
    let url = index_url.join(&entry.to_index_url()).unwrap();

    let mut request = ureq_agent().request_url("GET", &url);

    // Add cache control headers to all index requests.
    if let Some(etag) = entry.etag() {
        request = request.set("If-None-Match", etag);
    } else if let Some(last_modified) = entry.last_modified() {
        request = request.set("If-Modified-Since", &last_modified);
    }

    let response = request.call().map_err(Box::new)?;

    let status = response.status();

    // Update the index entry metadata from the upstream response.
    if let Some(etag) = response.header("ETag") {
        entry.set_etag(etag);
    }
    if let Some(last_modified) = response.header("Last-Modified") {
        entry.set_last_modified(last_modified);
    }

    // Update the upstream server access timestamp.
    entry.set_last_updated();

    let mut data: Vec<u8> = Vec::with_capacity(INDEX_ENTRY_CAPACITY);
    response
        .into_reader()
        .read_to_end(&mut data)
        .map_err(|e| Box::new(e.into()))?;

    Ok(IndexResponse {
        entry,
        status,
        data,
    })
}

/// Logs network errors when sending HTTP responses.
fn log_send_error(error: std::io::Error) {
    error!("proxy: sending response failed: {error}");
    drop(error);
}

/// Sends an empty HTTP error response.
fn send_error_response(request: Request, code: u16) {
    request
        .respond(Response::empty(code))
        .unwrap_or_else(log_send_error);
}

/// Sends a generic JSON-encoded HTTP response.
fn send_json_response(request: Request, code: u16, json: String) {
    let content_type = JSON_HTTP_CTYPE.parse::<Header>().unwrap();

    let response = Response::from_string(json)
        .with_status_code(code)
        .with_header(content_type);

    request.respond(response).unwrap_or_else(log_send_error);
}

/// Sends the crate data download response.
fn send_crate_data_response(request: Request, data: Vec<u8>) {
    let content_type = CRATE_HTTP_CTYPE.parse::<Header>().unwrap();
    let response = Response::from_data(data).with_header(content_type);

    request.respond(response).unwrap_or_else(log_send_error);
}

/// Adds cache control metadata headers to an index entry response.
fn set_index_response_headers<R: Read>(
    mut response: Response<R>,
    entry: &IndexEntry,
) -> Response<R> {
    if let Some(etag) = entry.etag() {
        let etag = Header::from_bytes("ETag", etag).unwrap();
        response = response.with_header(etag);
    };

    if let Some(last_modified) = entry.last_modified() {
        let last_modified = Header::from_bytes("Last-Modified", last_modified).unwrap();
        response = response.with_header(last_modified);
    };

    response
}

/// Sends the registry index entry download response.
fn send_index_entry_data_response(request: Request, index_response: IndexResponse) {
    let content_type = INDEX_HTTP_CTYPE.parse::<Header>().unwrap();
    let mut response = Response::from_data(index_response.data)
        .with_status_code(index_response.status)
        .with_header(content_type);

    response = set_index_response_headers(response, &index_response.entry);
    request.respond(response).unwrap_or_else(log_send_error);
}

/// Sends the registry index entry file download response.
///
/// This kind of response is always successful.
fn send_index_entry_file_response(request: Request, entry: IndexEntry, data: Vec<u8>) {
    // HTTP 200 OK
    let status = 200;

    let response = IndexResponse {
        entry,
        status,
        data,
    };

    send_index_entry_data_response(request, response);
}

/// Sends the registry index entry HTTP 304 Not Modified response.
fn send_index_entry_not_modified_response(request: Request, entry: &IndexEntry) {
    let mut response = Response::empty(304);
    response = set_index_response_headers(response, entry);
    request.respond(response).unwrap_or_else(log_send_error);
}

/// Formats the crate download API JSON error response.
#[must_use]
fn format_json_error(error: impl Display) -> String {
    format!(r#"{{"errors":[{{"detail":"{error}"}}]}}"#)
}

/// Sends the HTTP error response from an ureq client error.
fn send_fetch_error_response(request: Request, error: Box<ureq::Error>) {
    match *error {
        // Forward the HTTP error status received from the upstream server.
        ureq::Error::Status(code, response) => {
            let json = response.into_string().unwrap_or_else(format_json_error);
            warn!("fetch: upstream returned HTTP status {code}: {json}");
            send_json_response(request, code, json);
        }

        // Return HTTP 502 Bad Gateway for client connection errors.
        ureq::Error::Transport(err) => {
            error!("fetch: connection failed: {err}");
            send_json_response(request, 502, format_json_error(err));
        }
    };
}

/// Forwards the crate download request to the upstream server.
///
/// Processes the download request in a dedicated thread.
fn forward_download_request(request: Request, crate_info: CrateInfo, config: ProxyConfig) {
    let thread_name = format!("worker-fetch-crate-{}", crate_info.name());

    let thread_proc = move || match download_crate(&config.upstream_url, &crate_info) {
        Ok(data) => {
            info!("fetch: successfully downloaded {crate_info}");
            cache_store_crate(&config.crates_dir, &crate_info, &data);
            send_crate_data_response(request, data);
        }
        Err(err) => send_fetch_error_response(request, err),
    };

    std::thread::Builder::new()
        .name(thread_name)
        .spawn(thread_proc)
        .expect("failed to spawn the crate download thread");
}

/// Forwards the registry index entry download request to the upstream server.
///
/// Processes the download request in a dedicated thread.
///
/// If the requested index entry file already exists in the cache,
/// attempts to reduce the amount of data transferred on both sides.
fn forward_index_request(
    request: Request,
    entry: IndexEntry,
    cached_entry: Option<IndexEntry>,
    config: ProxyConfig,
) {
    let thread_name = format!("worker-fetch-index-{entry}");

    // Select where the new HTTP request headers will come from.
    let req_entry = cached_entry.unwrap_or_else(|| entry.clone());

    let thread_proc = move || match download_index_entry(&config.index_url, req_entry) {
        Ok(response) => {
            // Check for HTTP 200 or HTTP 304 statuses.
            if response.status == 200 {
                info!("fetch: successfully got index entry for {entry}");
                cache_store_index_entry(&config.index_dir, &response.entry, &response.data);
            } else {
                debug!("fetch: cached index entry for {entry} is up to date");
            }

            metadata_store_index_entry(&response.entry);

            if response.entry.is_equivalent(&entry) {
                // Updated index entry file metadata matches that of the client request.
                debug!("proxy: forwarding the up to date status for {entry}");
                send_index_entry_not_modified_response(request, &response.entry);
            } else if response.status == 200 {
                // Upstream registry sent us updated index entry data.
                debug!("proxy: forwarding new index data for {entry}");
                send_index_entry_data_response(request, response);
            } else if let Some(data) = cache_fetch_index_entry(&config.index_dir, &entry) {
                // Upstream registry sent us 304 Not Modified,
                // but the client does not have this file cached.
                // Fetch the index entry file from the local filesystem cache.
                debug!("proxy: forwarding cached index data for {entry}");
                send_index_entry_file_response(request, response.entry, data);
            } else {
                // Something went very wrong with the local filesystem cache.
                error!("cache: lost index cache file for {entry}");
                // Invalidate the volatile metadata cache and ask the client to retry.
                metadata_invalidate_index_entry(&entry);
                send_error_response(request, 503);
            }
        }
        Err(err) => {
            if let ureq::Error::Transport(err) = err.as_ref() {
                if let Some(data) = cache_fetch_index_entry(&config.index_dir, &entry) {
                    error!("fetch: index connection failed: {err}");

                    // The upstream registry can not be reached at the moment, likely
                    // due to an intermittent network failure.
                    // Serve a possibly stale index entry file from the local filesystem
                    // cache anyway to keep the clients running.
                    warn!("proxy: forwarding possibly stale cached index data for {entry}");

                    send_index_entry_file_response(request, entry, data);
                    return;
                }
            }

            // Forward non-recoverable download errors back to the clients.
            send_fetch_error_response(request, err);
        }
    };

    std::thread::Builder::new()
        .name(thread_name)
        .spawn(thread_proc)
        .expect("failed to spawn the index download thread");
}

/// Processes one crate download API request.
fn handle_download_request(request: Request, crate_url: &str, config: &ProxyConfig) {
    let Some(crate_info) = CrateInfo::try_from_download_url(crate_url) else {
        warn!("proxy: unrecognized download API endpoint: {crate_url}");
        send_error_response(request, 404);
        return;
    };

    debug!("proxy: download API endpoint hit: {crate_url}");

    if let Some(data) = cache_fetch_crate(&config.crates_dir, &crate_info) {
        debug!("proxy: local cache hit for {crate_info}");
        send_crate_data_response(request, data);
    } else {
        forward_download_request(request, crate_info, config.clone());
    }
}

/// Processes one sparse registry index API request.
fn handle_index_request(request: Request, index_url: &str, config: &ProxyConfig) {
    if is_config_json_url(index_url) {
        debug!("proxy: sending registry config file");
        send_json_response(request, 200, gen_config_json_file(config));
        return;
    }

    let Some(mut index_entry) = IndexEntry::try_from_index_url(index_url) else {
        warn!("proxy: malformed registry index path: {index_url}");
        send_error_response(request, 404);
        return;
    };

    debug!("proxy: requesting index entry for {index_entry}");

    // Extract cache control headers from all index requests.
    for header in request.headers() {
        if header.field.equiv("If-None-Match") {
            let etag = header.value.as_str();
            debug!("proxy: checking known index entry {index_entry} with ETag: {etag}");
            index_entry.set_etag(etag);
        }
        if header.field.equiv("If-Modified-Since") {
            let last_modified = header.value.as_str();
            debug!("proxy: checking known index entry {index_entry} with Last-Modified: {last_modified}");
            index_entry.set_last_modified(last_modified);
        }
    }

    // Try to serve the request from the local index cache first.
    // NOTE: The index file cache can not be used without matching metadata.
    if let Some(cached_entry) = metadata_fetch_index_entry(index_entry.name()) {
        // Expired cache entries require a new request to the upstream registry.
        if cached_entry.is_expired_with_ttl(&config.cache_ttl) {
            info!("proxy: index cache expired for {index_entry}, refreshing...");
            forward_index_request(request, index_entry, Some(cached_entry), config.clone());
            return;
        }

        // Check for the index metadata cache hit via ETag and Last-Modified fields.
        if cached_entry.is_equivalent(&index_entry) {
            debug!("proxy: index metadata cache hit for {index_entry}");
            send_index_entry_not_modified_response(request, &cached_entry);
            return;
        }

        // Check for the index file cache hit next.
        if let Some(data) = cache_fetch_index_entry(&config.index_dir, &index_entry) {
            debug!("proxy: index data cache hit for {index_entry}");
            send_index_entry_file_response(request, cached_entry, data);
            return;
        }
    }

    // Try to recreate the index entry metadata from the cached file mtime.
    let mtimed_entry = cache_try_find_index_entry(&config.index_dir, index_entry.name());

    if let Some(entry) = &mtimed_entry {
        let last_modified = entry.last_modified().unwrap();

        info!(
            "proxy: recreated index cache metadata for {entry} with Last-Modified: {last_modified}"
        );
    }

    // Fall back to forwarding the request to the upstream registry.
    forward_index_request(request, index_entry, mtimed_entry, config.clone());
}

/// Processes one HTTP GET request.
///
/// Only registry index and download API requests are supported.
fn handle_get_request(request: Request, config: &ProxyConfig) {
    let url = request.url().to_owned();

    if let Some(index_url) = url.strip_prefix(CRATES_INDEX_PATH) {
        handle_index_request(request, index_url, config);
    } else if let Some(crate_url) = url.strip_prefix(CRATES_API_PATH) {
        handle_download_request(request, crate_url, config);
    } else {
        warn!("proxy: unknown index or download API path: {url}");
        send_error_response(request, 404);
    };
}

/// Server listening address
enum ListenAddress {
    /// IP address + port
    SocketAddr(String),
    /// Unix domain socket path
    UnixPath(String),
}

/// Runs HTTP proxy server forever.
fn main_loop(listen_addr: &ListenAddress, config: &ProxyConfig) -> ! {
    let server = match listen_addr {
        ListenAddress::SocketAddr(addr) => {
            info!("proxy: starting HTTP server at: {addr}");
            Server::http(addr).expect("failed to start the HTTP server")
        }
        ListenAddress::UnixPath(path) => {
            info!("proxy: starting HTTP server at Unix socket {path}");
            let path = Path::new(path);
            // Reap stale socket files before binding.
            std::fs::remove_file(path).ok();
            Server::http_unix(path).expect("failed to start the HTTP server")
        }
    };

    // Main HTTP request accept loop.
    loop {
        let request = server.recv().expect("failed to accept new HTTP requests");

        // Forbid non-downloading HTTP methods.
        if *request.method() != Method::Get {
            warn!(
                "proxy: unexpected download API method: {}",
                request.method()
            );
            send_error_response(request, 403);
            continue;
        }

        handle_get_request(request, config);
    }
}

/// Prints the program version banner.
fn version() {
    let build = option_env!("CI_PIPELINE_ID");
    let rev = option_env!("CI_COMMIT_SHORT_SHA");
    let tag = option_env!("CI_COMMIT_REF_NAME");

    if let (Some(build), Some(rev), Some(tag)) = (build, rev, tag) {
        println!("crates-io-proxy {VERSION}+{build}.g{rev}.{tag}");
    } else {
        println!("crates-io-proxy {VERSION}");
    }
}

/// Prints the program invocation help page.
fn usage() {
    println!("Usage:\n    crates-io-proxy [options]\n");
    println!("Options:");
    println!("    -v, --verbose              print more debug info");
    println!("    -h, --help                 print help and exit");
    println!("    -V, --version              print version and exit");
    println!("    -L, --listen ADDRESS:PORT  address and port to listen at (0.0.0.0:3080)");
    println!("        --listen-unix PATH     Unix domain socket path to listen at");
    println!("    -U, --upstream-url URL     upstream download URL (https://crates.io/)");
    println!("    -I, --index-url URL        upstream index URL (https://index.crates.io/)");
    println!("    -S, --proxy-url URL        this proxy server URL (http://localhost:3080/)");
    println!("    -C, --cache-dir DIR        proxy cache directory (/var/cache/crates-io-proxy)");
    println!("    -T, --cache-ttl SECONDS    index cache entry Time-to-Live in seconds (3600)");
    println!("\nEnvironment:");
    println!("    INDEX_CRATES_IO_URL        same as --index-url option");
    println!("    CRATES_IO_URL              same as --upstream-url option");
    println!("    CRATES_IO_PROXY_URL        same as --proxy-url option");
    println!("    CRATES_IO_PROXY_CACHE_DIR  same as --cache-dir option");
    println!("    CRATES_IO_PROXY_CACHE_TTL  same as --cache-ttl option");
}

fn main() {
    let index_crates_io_url =
        env::var("INDEX_CRATES_IO_URL").unwrap_or_else(|_| INDEX_CRATES_IO_URL.to_string());
    let crates_io_url = env::var("CRATES_IO_URL").unwrap_or_else(|_| CRATES_IO_URL.to_string());
    let default_proxy_url =
        env::var("CRATES_IO_PROXY_URL").unwrap_or_else(|_| DEFAULT_PROXY_URL.to_string());
    let default_cache_dir =
        env::var("CRATES_IO_PROXY_CACHE_DIR").unwrap_or_else(|_| DEFAULT_CACHE_DIR.to_string());
    let default_cache_ttl_secs: u64 = env::var("CRATES_IO_PROXY_CACHE_TTL")
        .map_or(DEFAULT_CACHE_TTL_SECS, |s| {
            s.parse().expect("bad CRATES_IO_PROXY_CACHE_DIR value")
        });

    let mut verbose: u32 = 0;
    let mut args = Arguments::from_env();

    if args.contains(["-h", "--help"]) {
        usage();
        return;
    }

    if args.contains(["-V", "--version"]) {
        version();
        return;
    }

    while args.contains(["-v", "--verbose"]) {
        verbose += 1;
    }

    let listen_addr_unix = args
        .opt_value_from_str("--listen-unix")
        .expect("bad listen socket path");

    let listen_addr_ip = args
        .opt_value_from_str(["-L", "--listen"])
        .expect("bad listen address argument")
        .unwrap_or_else(|| LISTEN_ADDRESS.to_string());

    let index_url_string = args
        .opt_value_from_str(["-I", "--index-url"])
        .expect("bad upstream index URL argument")
        .unwrap_or(index_crates_io_url);

    let upstream_url_string = args
        .opt_value_from_str(["-U", "--upstream-url"])
        .expect("bad upstream download URL argument")
        .unwrap_or(crates_io_url);

    let proxy_url_string = args
        .opt_value_from_str(["-S", "--proxy-url"])
        .expect("bad proxy URL argument")
        .unwrap_or(default_proxy_url);

    let cache_dir_string = args
        .opt_value_from_str(["-C", "--cache-dir"])
        .expect("bad cache directory argument")
        .unwrap_or(default_cache_dir);

    let cache_ttl_secs: u64 = args
        .opt_value_from_str(["-T", "--cache-ttl"])
        .expect("bad cache TTL argument")
        .unwrap_or(default_cache_ttl_secs);

    let loglevel = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    LogBuilder::from_env(LogEnv::new().default_filter_or(loglevel)).init();

    let index_url = Url::parse(&index_url_string).expect("invalid upstream URL format");

    info!("proxy: using upstream index URL: {index_url}");

    let upstream_url = Url::parse(&upstream_url_string).expect("invalid upstream URL format");

    info!("proxy: using upstream download URL: {upstream_url}");

    let proxy_url = Url::parse(&proxy_url_string).expect("invalid proxy URL format");

    info!("proxy: using proxy server URL: {proxy_url}");

    let cache_dir = PathBuf::from(cache_dir_string);
    let index_dir = cache_dir.join("index");
    let crates_dir = cache_dir.join("crates");
    let cache_ttl = Duration::from_secs(cache_ttl_secs);

    info!(
        "cache: using index directory: {}",
        index_dir.to_string_lossy()
    );

    info!(
        "cache: using crates directory: {}",
        crates_dir.to_string_lossy()
    );

    info!("cache: using index entry TTL = {cache_ttl_secs} seconds");

    let config = ProxyConfig {
        index_url,
        upstream_url,
        proxy_url,
        index_dir,
        crates_dir,
        cache_ttl,
    };

    let listen_addr = match listen_addr_unix {
        Some(unix_path) => ListenAddress::UnixPath(unix_path),
        None => ListenAddress::SocketAddr(listen_addr_ip),
    };

    // Start the main HTTP server.
    main_loop(&listen_addr, &config)
}
