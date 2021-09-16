Caching HTTP proxy server for crates.io downloads
=================================================

Introduction
------------

The proxy server listens to HTTP GET requests at
`/api/v1/crates/{crate}/{version}/download`,
forwards them to <https://crates.io/> and caches the downloaded crates as
`.crate` files on the local filesystem.
Subsequent download API hits are serviced using the locally cached crate files.

To use the proxy server, clone and rehost the [crates.io index] repository
from GitHub and change `"dl"` parameter in `config.json` file in
the repository root to point to the proxy server instead:

```
{
    "dl": "https://crates-io-proxy.example.com/api/v1/crates",
    "api": "https://crates.io"
}
```

Cargo can be told to use the package index mirror by using the source
replacement feature. Add the following lines to your `.cargo/config`:

```
[source.crates-io]
registry = "https://crates-io-mirror.example.com/crates-io-index.git"
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
    -U, --upstream-url URL     upstream crates.io URL (https://crates.io/)
    -C, --cache-dir DIR        proxy cache directory (/var/cache/crates-io-proxy)

Environment:
    CRATES_IO_URL              same as --upstream-url option
    CRATES_IO_PROXY_CACHE_DIR  same as --cache-dir option

```

[crates.io index]: https://github.com/rust-lang/crates.io-index
