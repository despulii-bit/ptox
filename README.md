# ptox

[![CI](https://github.com/despulii-bit/ptox/actions/workflows/ci.yml/badge.svg)](https://github.com/despulii-bit/ptox/actions/workflows/ci.yml)

**ptox is a language-neutral SDK for building file-search clients that work across
many networks.**

It has three parts:

- **Contracts** — the data model and interfaces: `FileQuery`, `FileResult`,
  `Address`, `SearchProvider`, `Fetcher`, `Capabilities`.
- **Engines** — the high-level [`Engine`](crates/ptox/src/engine.rs), the search
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

- `crates/ptox/spec/` — the contract, conformance requirements, test vectors, JSON
  Schema (shipped inside the `ptox` crate).
- `crates/ptox` — **the SDK**, one crate with six modules:
  - `ptox::engine` — the high-level `Engine` (search + fetch), the client entry point.
  - `ptox::types` — data contracts (`FileQuery`, `FileResult`, `Address`, …).
  - `ptox::search` — the `SearchProvider` contract, `Capabilities`, and the fan-out engine.
  - `ptox::provider` — the NDJSON wire format and `SubprocessProvider`.
  - `ptox::fetch` — the `Fetcher` contract and retrieval (feature `fetch`),
    including an HTTP gateway fetcher for `ipfs://` (feature `ipfs`).
  - `ptox::merge` — deduplicate + rank results from many providers.
- `crates/ptox-conformance` — provider conformance suite (library + CLI).

A client depends on `ptox`. Retrieval (`fetch` and the `ipfs` gateway) is on by
default; disable default features for a search-only build. A provider author
uses `ptox::types` + `ptox::search` and tests with `ptox-conformance`.

## Development

Rust is provided by Nix (see `shell.nix`):

```sh
nix-shell --run 'cargo test --workspace'
nix-shell --run 'cargo clippy --workspace --all-targets -- -D warnings'
```

## Writing a provider

A provider is one network or index. Two ways to plug one in:

- **Rust:** implement the `SearchProvider` trait and pass it to `fanout`.
  See the runnable [`rust_provider`](crates/ptox/examples/rust_provider.rs)
  example (trait impl, `Capabilities`, error codes, and an offline test with a
  fake transport).
- **Any language:** speak the NDJSON wire format (contract §9) and wrap the
  program with `SubprocessProvider`. Configuration (proxies, routers, data dirs)
  is passed via `.arg()` / `.env()`. Output is read with a bound, so a hostile
  provider cannot exhaust memory. See
  [`ndjson_provider`](crates/ptox/examples/ndjson_provider.rs).

The end-to-end walkthrough — shape, capabilities, error codes, offline testing,
and validation — is in [**docs/writing-a-provider.md**](docs/writing-a-provider.md).

Test any provider against the contract with the conformance suite:

```sh
cargo run -p ptox-conformance -- -- <provider-command> [args...]
```

`crates/ptox-conformance/examples/fixture_provider.rs` is a provider that
demonstrates the failure modes the suite detects.

## Development files

Agent/development scaffolding does **not** live here. It is tracked in a separate
private repo so that cloning this SDK never pulls in unrelated files.

## Status

Private, pre-release. Contract **v2** locked (with a JSON Schema). The SDK is a
single `ptox` crate with `types` / `search` / `provider` / `fetch` modules (fetch
behind a feature); the conformance suite is the `ptox-conformance` crate.
Providers, clients, ranking policy, and indexing are separate; the SDK supplies a
default merge/rank. 76 tests green.

## Contributing

Contributions are welcome — especially providers. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
