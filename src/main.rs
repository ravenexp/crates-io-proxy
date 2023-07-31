//! Caching HTTP proxy server for crates.io downloads
//! =================================================
//!
//! Listens to HTTP GET requests at `/api/v1/crates/{crate}/{version}/download`,
//! forwards them to <https://crates.io/> and caches the downloaded crates as
//! `.crate` files on the local filesystem.

mod crate_info;

use std::env;
use std::fmt::Display;
use std::fs::{create_dir_all, read, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pico_args::Arguments;

use env_logger::{Builder as LogBuilder, Env as LogEnv};
use log::{debug, error, info, warn};

use tiny_http::{Header, Method, Request, Response, Server};
use url::Url;

use crate::crate_info::CrateInfo;

/// Default listen address and port
const LISTEN_ADDRESS: &str = "0.0.0.0:3080";

/// Upstream `crates.io` registry URL
const CRATES_IO_URL: &str = "https://crates.io/";

/// Crates download API path
const CRATES_API_PATH: &str = "/api/v1/crates/";

/// Default crate files cache directory path
const DEFAULT_CACHE_DIR: &str = "/var/cache/crates-io-proxy";

/// Limit the download item size to 16 MiB
const MAX_CRATE_SIZE: usize = 0x100_0000;

/// HTTP Content-Type of the crate package file
const CRATE_HTTP_CTYPE: &str = "Content-Type: application/x-tar";

/// HTTP Content-Type of the crates API JSON response
const JSON_HTTP_CTYPE: &str = "Content-Type: application/json; charset=utf-8";

/// Program version tag: `"<major>.<minor>.<patch>"`
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Proxy server configuration
#[derive(Debug, Clone)]
struct ProxyConfig {
    /// Upstream crate download url (defaults to [`CRATES_IO_URL`])
    upstream_url: Url,

    /// Crate files cache directory
    crates_dir: PathBuf,
}

/// Gets the server-global ureq client instance.
///
/// The global agent instance is required to use HTTP request pipelining.
fn ureq_agent() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(ureq::agent).clone()
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
            // HTTP 400 Invalid Header
            return Err(Box::new(ureq::Error::Status(400, response)));
        };

        if len > MAX_CRATE_SIZE {
            // HTTP 507 Insufficient Storage
            return Err(Box::new(ureq::Error::Status(507, response)));
        }

        let mut data: Vec<u8> = Vec::with_capacity(len);
        response
            .into_reader()
            .read_to_end(&mut data)
            .map_err(|e| Box::new(e.into()))?;

        Ok(data)
    } else {
        // HTTP 502 Bad Gateway
        Err(Box::new(ureq::Error::Status(502, response)))
    }
}

/// Caches the crate package file on the local filesystem.
fn cache_store_crate(dir: &Path, crate_info: &CrateInfo, data: &[u8]) {
    let crate_file_path = dir.join(crate_info.to_file_path());

    if let Err(e) = create_dir_all(crate_file_path.parent().unwrap()) {
        error!("cache: failed to create crate directory: {e}");
        return;
    }

    match File::create(crate_file_path) {
        Ok(mut f) => {
            f.write_all(data)
                .unwrap_or_else(|e| error!("cache: failed to write crate file: {e}"));
        }
        Err(e) => {
            error!("cache: failed to create crate file: {e}");
        }
    }
}

/// Fetches the cached crate package file from the local filesystem, if present.
fn cache_fetch_crate(dir: &Path, crate_info: &CrateInfo) -> Option<Vec<u8>> {
    read(dir.join(crate_info.to_file_path())).ok()
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

/// Processes one HTTP GET request.
///
/// Only crate download requests are currently supported.
fn handle_get_request(request: Request, config: &ProxyConfig) {
    let url = request.url().to_owned();

    let Some(crate_url) = url.strip_prefix(CRATES_API_PATH) else {
        warn!("proxy: unknown download API path: {url}");
        send_error_response(request, 404);
        return;
    };

    handle_download_request(request, crate_url, config);
}

/// Runs HTTP proxy server forever.
fn main_loop(listen_addr: &str, config: &ProxyConfig) -> ! {
    info!("proxy: starting HTTP server at: {listen_addr}");

    let server = Server::http(listen_addr).expect("failed to start the HTTP server");

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
    println!("    -U, --upstream-url URL     upstream crates.io URL (https://crates.io/)");
    println!("    -C, --cache-dir DIR        proxy cache directory (/var/cache/crates-io-proxy)");
    println!("\nEnvironment:");
    println!("    CRATES_IO_URL              same as --upstream-url option");
    println!("    CRATES_IO_PROXY_CACHE_DIR  same as --cache-dir option");
}

fn main() {
    let crates_io_url = env::var("CRATES_IO_URL").unwrap_or_else(|_| CRATES_IO_URL.to_string());
    let default_cache_dir =
        env::var("CRATES_IO_PROXY_CACHE_DIR").unwrap_or_else(|_| DEFAULT_CACHE_DIR.to_string());

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

    let listen_addr = args
        .opt_value_from_str(["-L", "--listen"])
        .expect("bad listen address argument")
        .unwrap_or_else(|| LISTEN_ADDRESS.to_string());

    let upstream_url_string = args
        .opt_value_from_str(["-U", "--upstream-url"])
        .expect("bad upstream URL argument")
        .unwrap_or(crates_io_url);

    let cache_dir_string = args
        .opt_value_from_str(["-C", "--cache-dir"])
        .expect("bad cache directory argument")
        .unwrap_or(default_cache_dir);

    let loglevel = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    LogBuilder::from_env(LogEnv::new().default_filter_or(loglevel)).init();

    let upstream_url = Url::parse(&upstream_url_string).expect("invalid upstream URL format");

    info!("proxy: using upstream server URL: {upstream_url}");

    let cache_dir = PathBuf::from(cache_dir_string);
    let crates_dir = cache_dir.join("crates");

    info!(
        "cache: using crates directory: {}",
        crates_dir.to_string_lossy()
    );

    let config = ProxyConfig {
        upstream_url,
        crates_dir,
    };

    // Start the main HTTP server.
    main_loop(&listen_addr, &config)
}
