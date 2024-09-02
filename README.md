Caching HTTP proxy server for the `crates.io` registry
======================================================

Introduction
------------

`crates-io-proxy` implements transparent caching for both
the sparse registry index at <https://index.crates.io/> and
the static crate file download server.

Two independent HTTP proxy endpoints are implemented:

1. Listens to HTTP GET requests at `/index/.../{crate}`,
   forwards them to <https://index.crates.io/> and caches the downloaded registry
   index entries as JSON text files on the local filesystem.

2. Listens to HTTP GET requests at `/api/v1/crates/{crate}/{version}/download`,
   forwards them to <https://crates.io/> and caches the downloaded crates as
   `.crate` files on the local filesystem.

Subsequent sparse registry index and crate download API hits are serviced
using the locally cached index entry and crate files.

As a convenience feature, the download requests for the `config.json` file
found at the sparse index root are served with a replacement file,
which changes the crate download URL to point to this same proxy server.

Usage
-----

Cargo can be told to use the crate registry mirror by using the source
replacement feature. Add the following lines to your `.cargo/config`:

```
[source.crates-io]
replace-with = "crates-io-mirror"

[registries.crates-io-mirror]
index = "sparse+http://crates-io-proxy.example.com:3080/index/"
```

Using static git index mirror
-----------------------------

`crates-io-proxy` can also be used as the crate file download proxy server
with a separate git-based registry index.

To use this configuration, clone and rehost the [crates.io index] repository
from GitHub and change `"dl"` parameter in `config.json` file in
the repository root to point to the `crates-io-proxy` server instead:

```
{
    "dl": "https://crates-io-proxy.example.com:3080/api/v1/crates",
    "api": "https://crates.io"
}
```

In this configuration, the git registry index link should be used instead:

```
[registries.crates-io-mirror]
index = "https://crates-io-index.example.com/crates-io-index.git"
```

Configuration
-------------

The proxy server can be configured by either command line options
or environment variables.

Run `crates-io-proxy --help` to get the following help page:

```
Usage:
    crates-io-proxy [options]

Options:
    -v, --verbose              print more debug info
    -h, --help                 print help and exit
    -V, --version              print version and exit
    -L, --listen ADDRESS:PORT  address and port to listen at (0.0.0.0:3080)
        --listen-unix PATH     Unix domain socket path to listen at
    -U, --upstream-url URL     upstream download URL (https://crates.io/)
    -I, --index-url URL        upstream index URL (https://index.crates.io/)
    -S, --proxy-url URL        this proxy server URL (http://localhost:3080/)
    -C, --cache-dir DIR        proxy cache directory (/var/cache/crates-io-proxy)
    -T, --cache-ttl SECONDS    index cache entry Time-to-Live in seconds (3600)

Environment:
    INDEX_CRATES_IO_URL        same as --index-url option
    CRATES_IO_URL              same as --upstream-url option
    CRATES_IO_PROXY_URL        same as --proxy-url option
    CRATES_IO_PROXY_CACHE_DIR  same as --cache-dir option
    CRATES_IO_PROXY_CACHE_TTL  same as --cache-ttl option
```

Advanced configuration
----------------------

By default, `crates-io-proxy` uses embedded TLS trusted root certificates.
It is possible to configure it to use the system certificate store
at the build time by setting the `native-certs` feature flag.

Configuring this behavior at the run time is not supported yet.

[crates.io index]: https://github.com/rust-lang/crates.io-index
