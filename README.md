# cwm

[![CI](https://github.com/despulii-bit/cwm/actions/workflows/ci.yml/badge.svg)](https://github.com/despulii-bit/cwm/actions/workflows/ci.yml)

**cwm is a language-neutral SDK for building file-search clients that work across
many networks.**

It has three parts:

- **Contracts** — the data model and interfaces: `FileQuery`, `FileResult`,
  `Address`, `SearchProvider`, `Fetcher`, `Capabilities`.
- **Engines** — the high-level [`Engine`](crates/cwm/src/engine.rs), the search
  fan-out engine, the retrieval engine, and the merge/rank post-processor.
- **Tooling** — the conformance suite and the wire schema.

It does **not** include networks or user interfaces. Those are built on top:

- **Providers** adapt one network (e.g. clearnet, I2P, Freenet, IPFS, BitTorrent).
- **Clients** are frontends (CLI, GUI, agent) that assemble the SDK.

```
FileQuery ─▶ fanout(query, providers) ─▶ Stream[FileResult | Failure]
FileResult ─▶ fetch(address) ─▶ bytes on disk
```

## Repo model

Only **clients** (distributions) and **providers** (plugins) are separate repos.
**All shared libraries live in this repo**, as a Cargo workspace. Don't create a
repo per shared component — add a crate here.

## Layout

- `spec/` — the contract, conformance requirements, test vectors, JSON Schema.
- `crates/cwm` — **the SDK**, one crate with six modules:
  - `cwm::engine` — the high-level `Engine` (search + fetch), the client entry point.
  - `cwm::types` — data contracts (`FileQuery`, `FileResult`, `Address`, …).
  - `cwm::search` — the `SearchProvider` contract, `Capabilities`, and the fan-out engine.
  - `cwm::provider` — the NDJSON wire format and `SubprocessProvider`.
  - `cwm::fetch` — the `Fetcher` contract and retrieval (feature `fetch`).
  - `cwm::merge` — deduplicate + rank results from many providers.
- `crates/cwm-conformance` — provider conformance suite (library + CLI).

A client depends on `cwm` (enable the `fetch` feature for retrieval). A provider
author uses `cwm::types` + `cwm::search` and tests with `cwm-conformance`.

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
  program with `SubprocessProvider`. Configuration (proxies, routers, data dirs)
  is passed via `.arg()` / `.env()`. Output is read with a bound, so a hostile
  provider cannot exhaust memory.

Test any provider against the contract with the conformance suite:

```sh
cargo run -p cwm-conformance -- -- <provider-command> [args...]
```

See `crates/cwm-provider/examples/ndjson_provider.rs` for a minimal wire provider
and `crates/cwm-conformance/examples/fixture_provider.rs` for one that
demonstrates the failure modes the suite detects.

## Development files

Agent/development scaffolding does **not** live here. It is tracked in a separate
private repo so that cloning this SDK never pulls in unrelated files.

## Status

Private, pre-release. Contract **v2** locked (with a JSON Schema). The SDK is a
single `cwm` crate with `types` / `search` / `provider` / `fetch` modules (fetch
behind a feature); the conformance suite is the `cwm-conformance` crate.
Providers, clients, ranking policy, and indexing are separate; the SDK supplies a
default merge/rank. 39 tests green.

## Contributing

Contributions are welcome — especially providers. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
