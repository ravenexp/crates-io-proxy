Caching HTTP proxy server for crates.io downloads
=================================================

Listens to HTTP GET requests at `/api/v1/crates/{crate}/{version}/download`,
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

[crates.io index]: https://github.com/rust-lang/crates.io-index
