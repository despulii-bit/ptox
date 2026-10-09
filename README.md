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
- `crates/cwm-conformance` — provider conformance suite (library + CLI).

Providers are separate software with their own repos; this repo keeps only the
core and its tooling.

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

Contract v0 locked; Rust core implemented (fan-out, streaming, isolation,
timeout, cancellation, bounded NDJSON subprocess bridge, provider env config);
provider conformance suite (`cwm-conformance`); 25 tests green. Providers,
keyword search, and all frontends are separate software.

## Contributing

Contributions are welcome — especially providers. See [CONTRIBUTING.md](CONTRIBUTING.md)
for the provider on-ramp, scope rules, and CI requirements.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
