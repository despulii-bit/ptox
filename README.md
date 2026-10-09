# cwm

[![CI](https://github.com/despulii-bit/cwm/actions/workflows/ci.yml/badge.svg)](https://github.com/despulii-bit/cwm/actions/workflows/ci.yml)

A language-neutral core for querying **files across many networks** (clearnet, I2P,
Freenet/Hyphanet, IPFS, BitTorrent, Tor, …) from a single query.

The core does exactly one thing: take a `FileQuery`, fan it out to injected
`SearchProvider`s, and stream back attributed results. It has **no UI, no CLI, no
storage, no ranking, and no network I/O**.

```
FileQuery ──▶ fanout(query, providers) ──▶ Stream[FileResult | Failure]
```

Everything else is separate software built on this contract:

| Layer | Examples |
|---|---|
| Providers (one per network) | clearnet, I2P, Freenet, IPFS, BitTorrent |
| Retrieval / fetch fabric | protocol fetchers, download manager |
| Post-processing | merge, dedup, ranking |
| Frontends | CLI, TUI, GUI, web |
| Index | a local store exposed as just another provider |

## Layout

- `spec/core-contract.md` — the locked, language-neutral contract.
- `spec/conformance.md` — what any implementation must satisfy.
- `spec/vectors.json` — language-neutral test vectors.
- `crates/cwm-core` — the Rust implementation of the core.
- `crates/cwm-provider-btdht` — reference provider: BitTorrent DHT infohash→peers.

## Development

Rust is provided by Nix (see `shell.nix`):

```sh
nix-shell --run 'cargo test --workspace'
nix-shell --run 'cargo clippy --workspace --all-targets -- -D warnings'
```

## Writing a provider

A provider is one network or index. Two ways to plug one in:

- **Rust:** implement the `SearchProvider` trait and pass it to `fanout`.
- **Any language:** speak the NDJSON wire format (contract §9) and wrap the
  program with `SubprocessProvider` — the child is killed on cancellation.

The first reference provider is `crates/cwm-provider-btdht`. It resolves an
infohash or magnet to peers and returns a magnet `Address`; keyword queries are
`unsupported`, because the DHT is a hash table, not a keyword index.

See `crates/cwm-core/examples/ndjson_provider.rs` for a minimal wire provider.

## Development files

Agent/development scaffolding does **not** live here. It is tracked in a separate
repo so that cloning this program never pulls in unrelated files.

## Status

Contract v0 locked; Rust core implemented (fan-out, streaming, isolation,
timeout, cancellation, NDJSON subprocess bridge); BitTorrent DHT provider
(infohash → peers); 27 tests green. Keyword search over BitTorrent and all
frontends are separate software.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
