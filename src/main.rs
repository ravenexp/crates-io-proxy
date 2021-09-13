//! Caching HTTP proxy server for crates.io downloads
//! =================================================
//!
//! Listens to HTTP GET requests at `/api/v1/crates/{crate}/{version}/download`,
//! forwards them to <https://crates.io/> and caches the downloaded crates as
//! `.crate` files on the local filesystem.

use pico_args::Arguments;

use env_logger::{Builder as LogBuilder, Env as LogEnv};
use log::{debug, info, warn};

use rouille::{log as log_request, router, start_server, Response};

/// Program version tag: `"<major>.<minor>.<patch>"`
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs Rouille HTTP server forever.
fn main_loop(listen_addr: &str) -> ! {
    info!("Starting HTTP server at: {}", listen_addr);

    start_server(listen_addr, move |request| {
        log_request(request, std::io::stdout(), || {
            router!(
                request,
                (GET) (/api/v1/crates/{name: String}/{version: String}/download) => {
                    debug!("Download API endpoint hit: {}", request.url());
                    Response::text(format!("Pretending to be {} v{}", name, version))
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
    println!("    -v, --verbose        print more debug info");
    println!("    -h, --help           print help and exit");
    println!("    -V, --version        print version and exit");
}

fn main() {
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

    let loglevel = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    LogBuilder::from_env(LogEnv::new().default_filter_or(loglevel)).init();

    // Go run the main server
    main_loop("0.0.0.0:3080")
}
