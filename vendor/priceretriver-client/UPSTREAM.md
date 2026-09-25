# Vendored: priceretriver-client

Copied verbatim from the private sibling repository
`https://github.com/ChillingWombat/PriceRetriever` (crate
`crates/priceretriver-client`, upstream rev `0a8614258b86fdda8cd4fd9afdc28fb6e4c7a63b`).

Vendored so Arbitrageur builds and CI resolves manifests without access to the
private repository (a `path = "../../../PriceRetriver"` dependency fails on any
machine without that checkout; a `git` dependency on a private repo would
require credentials plumbing in CI).

The upstream crate also declared unused dependencies (`priceretriver-core`,
`tokio`, `tokio-tungstenite`, `futures`, `serde`, `thiserror`, `tracing`,
`bytes`); only `reqwest`/`serde_json`/`anyhow` are actually used by the source.

To re-sync: copy `crates/priceretriver-client/src/lib.rs` from the upstream
repo over `src/lib.rs` and update the rev above.
