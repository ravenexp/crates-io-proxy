//! Caching HTTP proxy server for crates.io downloads
//! =================================================
//!
//! Listens to HTTP GET requests at `/api/v1/crates/{crate}/{version}/download`,
//! forwards them to <https://crates.io/> and caches the downloaded crates as
//! `.crate` files on the local filesystem.

use std::env;
use std::fs::{create_dir_all, read, File};
use std::io::{Read, Write};
use std::path::Path;

use pico_args::Arguments;

use env_logger::{Builder as LogBuilder, Env as LogEnv};
use log::{debug, error, info, warn};

use url::Url;

use rouille::{log as log_request, router, start_server, Response};
use ureq::{request_url, Error};

/// Default listen address and port
const LISTEN_ADDRESS: &str = "0.0.0.0:3080";

/// Upstream `crates.io` download URL: also hardcoded in Cargo.
const CRATES_IO_URL: &str = "https://crates.io/";

/// Default crate files cache directory path
const DEFAULT_CACHE_DIR: &str = "/var/cache/crates-io-proxy";

/// Limit the download item size to 16 MiB
const MAX_CRATE_SIZE: usize = 0x100_0000;

/// HTTP Content-Type of the crate package file
const CRATE_HTTP_CTYPE: &str = "application/x-tar";

/// HTTP Content-Type of the download API error response
const ERROR_HTTP_CTYPE: &str = "application/json; charset=utf-8";

/// Program version tag: `"<major>.<minor>.<patch>"`
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Downloads the crate file from <https://crates.io/>
fn download_crate(base_url: &Url, name: &str, version: &str) -> Result<Vec<u8>, Box<Error>> {
    let url = base_url
        .join(&format!("/api/v1/crates/{}/{}/download", name, version))
        .unwrap();

    let resp = request_url("GET", &url).call().map_err(Box::new)?;

    if let Some(clen) = resp.header("Content-Length") {
        let len: usize = clen.parse().unwrap();

        if len > MAX_CRATE_SIZE {
            // Insufficient Storage
            return Err(Box::new(Error::Status(507, resp)));
        }

        let mut bytes: Vec<u8> = Vec::with_capacity(len);
        resp.into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| Box::new(e.into()))?;

        Ok(bytes)
    } else {
        // Bad Gateway
        Err(Box::new(Error::Status(502, resp)))
    }
}

/// Caches the crate package file on the local filesystem.
fn cache_store_crate(dir: &Path, name: &str, version: &str, data: &[u8]) {
    let pkgdir = dir.join(name);

    if let Err(e) = create_dir_all(&pkgdir) {
        error!("Failed to create pkg cache dir: {}", e);
        return;
    }

    let pkgfile = pkgdir.join(format!("{}-{}.crate", name, version));

    let mut file = match File::create(pkgfile) {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to create pkg cache file: {}", e);
            return;
        }
    };

    if let Err(e) = file.write_all(data) {
        error!("Failed to write pkg cache file: {}", e);
    }
}

/// Fetches the cached crate package file from the local filesystem, if present.
fn cache_fetch_crate(dir: &Path, name: &str, version: &str) -> Option<Vec<u8>> {
    let pkgfile = dir.join(name).join(format!("{}-{}.crate", name, version));

    read(pkgfile).ok()
}

/// Builds a HTTP error response from an `ureq` client download error.
fn make_error_response(client_err: Box<Error>) -> Response {
    match *client_err {
        // Return the HTTP error status received from crates.io
        Error::Status(code, resp) => {
            let unknown = r#"{"errors":[{"detail":"Unknown error"}]}"#;
            let err_json = resp.into_string().unwrap_or_else(|_| unknown.to_string());
            warn!("crates.io returned HTTP status {}: {}", code, err_json);

            Response::from_data(ERROR_HTTP_CTYPE, err_json).with_status_code(code)
        }
        // Return 502 Bad Gateway for connection errors
        Error::Transport(err) => {
            error!("Network error: {}", err);
            let err_json = format!(r#"{{"errors":[{{"detail":"{}"}}]}}"#, err);

            Response::from_data(ERROR_HTTP_CTYPE, err_json).with_status_code(502)
        }
    }
}

/// Runs Rouille HTTP server forever.
fn main_loop(listen_addr: &str, download_url: Url, cache_dir: &Path) -> ! {
    info!("Starting HTTP server at: {}", listen_addr);

    let crates_dir = cache_dir.join("crates");
    info!(
        "Using crates cache directory: {}",
        crates_dir.to_string_lossy()
    );

    start_server(listen_addr, move |request| {
        log_request(request, std::io::stdout(), || {
            router!(
                request,
                (GET) (/api/v1/crates/{name: String}/{version: String}/download) => {
                    debug!("Download API endpoint hit: {}", request.url());

                    if let Some(bytes) = cache_fetch_crate(&crates_dir, &name, &version) {
                        debug!("Local cache hit for {} v{}", name, version);
                        return Response::from_data(CRATE_HTTP_CTYPE, bytes);
                    }

                    match download_crate(&download_url, &name, &version) {
                        Ok(bytes) => {
                            info!("Successfully downloaded {} v{}", name, version);
                            cache_store_crate(&crates_dir, &name, &version, &bytes);
                            Response::from_data(CRATE_HTTP_CTYPE, bytes)
                        }
                        Err(err) => make_error_response(err),
                    }
                },
                _ => {
                    warn!("Unknown API endpoint hit: {}", request.url());
                    Response::empty_404()
                },
            )
        })
    })
}

/// Prints the program version banner.
fn version() {
    let build = option_env!("CI_PIPELINE_ID");
    let rev = option_env!("CI_COMMIT_SHORT_SHA");
    let tag = option_env!("CI_COMMIT_REF_NAME");

    if let (Some(build), Some(rev), Some(tag)) = (build, rev, tag) {
        println!("crates-io-proxy {}+{}.g{}.{}", VERSION, build, rev, tag,);
    } else {
        println!("crates-io-proxy {}", VERSION);
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
        .expect("Bad listen address argument")
        .unwrap_or_else(|| LISTEN_ADDRESS.to_string());

    let url_string = args
        .opt_value_from_str(["-U", "--upstream-url"])
        .expect("Bad upstream URL argument")
        .unwrap_or(crates_io_url);

    let cache_dir_string = args
        .opt_value_from_str(["-C", "--cache-dir"])
        .expect("Bad cache directory argument")
        .unwrap_or(default_cache_dir);

    let loglevel = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    LogBuilder::from_env(LogEnv::new().default_filter_or(loglevel)).init();

    let download_url = Url::parse(&url_string).expect("Invalid upstream URL format");

    info!("Using crates.io server URL: {}", download_url);

    let cache_dir = Path::new(&cache_dir_string);

    // Go run the main server
    main_loop(&listen_addr, download_url, cache_dir)
}
