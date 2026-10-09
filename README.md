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

This repo is the **shared SDK**: the contracts, the search engine, the retrieval
fabric, and the conformance kit. Everything else is separate software built on
those contracts:

| Layer | Examples |
|---|---|
| Providers (one per network) | clearnet, I2P, Freenet, IPFS, BitTorrent |
| Post-processing | merge, dedup, ranking |
| Frontends (clients) | CLI, TUI, GUI, web, agents |
| Index | a local store exposed as just another provider |

Only **clients** (distributions) and **providers** (plugins) are separate repos;
all shared libraries live here.

## Layout

- `spec/core-contract.md` — the locked, language-neutral contract.
- `spec/conformance.md` — what any implementation must satisfy.
- `spec/vectors.json` — language-neutral test vectors.
- `spec/schema/wire.schema.json` — JSON Schema for the wire protocol.
- `crates/cwm-core` — the contracts, search engine, and NDJSON bridge.
- `crates/cwm-fetch` — retrieval fabric (`Fetcher` contract + HTTP/command fetchers).
- `crates/cwm-conformance` — provider conformance suite (library + CLI).

Providers and clients are separate repos; this repo holds only the shared
libraries.

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
  Configuration (proxies, routers, data dirs) is passed via `.arg()` / `.env()`.
  Output is read with a bound, so a hostile provider cannot exhaust memory.

Test any provider against the contract with the conformance suite:

```sh
cargo run -p cwm-conformance -- -- <provider-command> [args...]
```

See `crates/cwm-core/examples/ndjson_provider.rs` for a minimal wire provider.
See `crates/cwm-conformance/examples/fixture_provider.rs` for one that
demonstrates the failure modes the suite detects.

## Development files

Agent/development scaffolding does **not** live here. It is tracked in a separate
repo so that cloning this program never pulls in unrelated files.

## Status

Contract v2 locked (with a JSON Schema); Rust core (fan-out, streaming,
isolation, timeout, cancellation, bounded NDJSON subprocess bridge); retrieval
fabric (`cwm-fetch`); conformance suite (`cwm-conformance`); 34 tests green.
Providers, clients, ranking, and indexing are separate software.

## Contributing

Contributions are welcome — especially providers. See [CONTRIBUTING.md](CONTRIBUTING.md)
for the provider on-ramp, scope rules, and CI requirements.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
