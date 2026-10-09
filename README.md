# cwm

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

## Development

Rust is provided by Nix (see `shell.nix`):

```sh
nix-shell --run 'cargo test --workspace'
nix-shell --run 'cargo clippy --workspace --all-targets -- -D warnings'
```

## Development files

Agent/development scaffolding does **not** live here. It is tracked in a separate
repo so that cloning this program never pulls in unrelated files.

## Status

Contract v0 locked; Rust core implemented; conformance suite green. No providers
or frontends yet — those are separate software.
