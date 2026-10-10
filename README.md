# ptox

[![CI](https://github.com/despulii-bit/ptox/actions/workflows/ci.yml/badge.svg)](https://github.com/despulii-bit/ptox/actions/workflows/ci.yml)

**ptox is a language-neutral SDK for building file-search clients that work across
many networks.**

It has three parts:

- **Contracts** — the data model and interfaces: `FileQuery`, `FileResult`,
  `Address`, `SearchProvider`, `Fetcher`, `Capabilities`.
- **Engines** — the high-level [`Engine`](crates/ptox/src/engine.rs), the search
  fan-out engine, the retrieval engine, and the merge/rank post-processor.
  Capability probing is concurrent and can be memoized per engine.
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
- `crates/ptox` — **the SDK**, one crate with seven modules:
  - `ptox::engine` — the high-level `Engine` (search + fetch), the client entry point.
  - `ptox::types` — data contracts (`FileQuery`, `FileResult`, `Address`, …).
  - `ptox::search` — the `SearchProvider` contract, `Capabilities`, and the fan-out engine.
  - `ptox::provider` — the NDJSON wire format and `SubprocessProvider`.
  - `ptox::fetch` — the `Fetcher` contract and retrieval (feature `fetch`),
    including an HTTP gateway fetcher for `ipfs://` (feature `ipfs`) and an
    opt-in `file://` fetcher (off-by-default feature `file`).
  - `ptox::merge` — deduplicate + rank results from many providers, plus
    `suggest_filename` for safe client-side naming and the versioned `ResultRef`
    persistence handle.
  - `ptox::config` — the canonical, versioned provider/fetcher TOML schema and
    loader (feature `config`).
- `crates/ptox-conformance` — provider conformance suite (library + CLI).

A client depends on `ptox`. Retrieval (`fetch` and the `ipfs` gateway) and the
config loader are on by default; disable default features for a search-only
build. The `file://` fetcher is an explicit opt-in: enable the `file` feature to
fetch local addresses (see [`docs/fetching.md`](docs/fetching.md) for the policy
implications). A provider author uses `ptox::types` + `ptox::search` and tests with
`ptox-conformance`.

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

## Configuring a client

Client wiring (which provider commands to run, which schemes to delegate to an
external fetcher, and ranking weights) has a canonical, versioned TOML shape, so
clients and tools agree on one format. Load it with `ptox::Config` and build the
pieces a client needs:

```toml
version = 1

[[provider]]
name = "archive-org"
program = "node"
args = ["/opt/ptox/adapters/archive_org.js"]

[ranking]
weights = { "archive-org" = 1.2 }

[fetch.commands.magnet]
program = "aria2c"
args = ["-d", "{dir}", "-o", "{name}", "{uri}"]
```

`Config::providers()` / `Config::fetch_registry()` / `Config::merge_options()`
turn it into runnable pieces; `Config::to_toml_string()` writes it back out. The
full field reference is in
[**docs/configuration.md**](docs/configuration.md), with a runnable example in
[`crates/ptox/examples/config.rs`](crates/ptox/examples/config.rs).

## Fetching a result

Retrieval turns a result's `Address` into bytes on disk. `Engine::fetch` /
`FetchRegistry::fetch_merged` pick the best mirror and fail over when it is dead,
verify the result's sha256, and write atomically; `fetch_with_progress` reports
cumulative bytes for a progress bar. Built-in schemes are `http`/`https` and an
`ipfs://` HTTP gateway (feature `ipfs`), and any other scheme can be delegated to
an external command (`magnet` → `aria2c`, …). The end-to-end guide — scheme
registry, the `CommandFetcher` `{dest}` contract, gateway policy, failover, and
retries — is in [**docs/fetching.md**](docs/fetching.md).

## Observing a search

`Engine::search` returns per-provider [`ProviderMetric`]s on
[`SearchReport::metrics`](crates/ptox/src/engine.rs) — result/failure counts and
elapsed time for each provider — so a client can show progress and diagnostics
without instrumenting the fan-out itself. The collector (`ptox::Metrics`) is also
available to `fanout_with_metrics` for clients that drive providers directly.
Metrics carry **counts and timings only**, never query text or result content.

## Development files

Agent/development scaffolding does **not** live here. It is tracked in a separate
private repo so that cloning this SDK never pulls in unrelated files.

## Status

Private, pre-release. Contract **v2** locked (with a JSON Schema). The SDK is a
single `ptox` crate with `types` / `search` / `provider` / `fetch` modules (fetch
behind a feature); the conformance suite is the `ptox-conformance` crate.
Providers, clients, ranking policy, and indexing are separate; the SDK supplies a
default merge/rank. 186 tests green (200 with `--all-features`).

## Contributing

Contributions are welcome — especially providers. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
