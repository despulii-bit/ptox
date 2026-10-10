# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `fetch`: multi-address failover. `FetchRegistry::fetch_merged` and
  `Engine::fetch` now try every address of a `MergedResult` in preference order
  (`https` > `http` > `ipfs` > `magnet` > other) until one succeeds, so a dead
  mirror no longer fails the whole retrieval. The winning address is reported on
  the new `Fetched::address` field; when every address fails, the new
  `FetchError::AllFailed` aggregates the per-attempt errors. `preferred_addresses`
  exposes the ordering, and progress stays monotonic across a failed attempt
  (a dead mirror's partial bytes never make the bar jump backwards).
- `merge::suggest_filename`: a deterministic, offline helper that turns a
  `MergedResult` into a safe filename — the title, else the last path segment of
  the preferred address, else `download.bin` — replacing characters that are
  illegal on common platforms and appending an extension from a known
  `mimetype`.
- Persistence-stability guarantee for `MergedResult`. The serialized shape is a
  contract (fields are only added within a major version, with `serde(default)`
  and `skip_serializing_if`), documented on the type, so clients can persist
  results between a search and a later fetch without an SDK upgrade silently
  invalidating them. `providers`, `addresses`, and `score` now carry
  `#[serde(default)]` so minimal/older blobs still deserialize.
- Provider-authoring docs and example. `examples/rust_provider.rs` is a
  runnable, offline-testable `SearchProvider`: it implements the trait with a
  streaming `search`, declares `Capabilities`, maps a fake backend's failures
  onto the right `ErrorCode`s, and is covered by unit tests driven through the
  fake transport. `docs/writing-a-provider.md` is the end-to-end guide
  (shape, capabilities, errors, offline testing, `ptox-conformance`).
- `fetch`: an `ipfs://` gateway fetcher. `IpfsGatewayFetcher` rewrites
  `ipfs://<cid>[/path]` onto a configurable HTTP gateway (`<gateway>/ipfs/...`)
  and delegates to `HttpFetcher`, so IPFS joins `http`/`https` as a scheme the
  SDK can retrieve out of the box—with the same atomic writes, hash
  verification, and progress reporting. `DEFAULT_IPFS_GATEWAY` is a public
  convenience; production clients should pass their own gateway. Behind the new
  `ipfs` feature (on by default), and registered by `FetchRegistry::with_defaults`
  when enabled.
- `fetch`: optional progress reporting. `Fetcher::fetch_with_progress`,
  `FetchRegistry::fetch_with_progress` / `fetch_merged_with_progress`, and
  `Engine::fetch_with_progress` report the cumulative number of retrieved bytes
  to a `ProgressFn` callback as chunks arrive, so a client can render a progress
  bar. The default trait method is a no-op (custom fetchers need not implement
  it); `HttpFetcher` reports per-chunk totals, and `CommandFetcher` polls the
  destination file size while its command runs, so command-delegated schemes
  such as magnets report progress too.

## [0.1.3] - 2026-10-10

### Added

- `probe` and `Engine::probe`: classify each provider's answer to a query as
  `Results` / `Empty` / `Skipped` / `Failed` / `Timeout`, for health checks and
  `providers` commands. `ProviderProbe` and `ProbeStatus` are exported.

## [0.1.2] - 2026-10-10

### Added

- `Engine::search_stream` returns a streaming `SearchStream` (itself a
  `Stream<Item = Outcome>`), so clients can render results as they arrive
  instead of waiting for every provider. `Engine::search` still returns the
  ranked, merged `SearchReport`.
- `ptox-conformance`: a `--hash ALGO=VALUE` selector (repeatable) to send
  hash-only queries (e.g. `--hash cid=<cid>`), so hash-only providers such as
  IPFS/BitTorrent can be validated. `--query` is optional when `--hash` is given.

### Changed

- Capability probing is now concurrent: one slow provider no longer stalls
  engine startup.
- `fanout` now normalizes attribution: a result with an empty `provider` is
  stamped with the emitting provider's name, so Rust provider authors no longer
  have to set it themselves (matching the wire contract, where the host owns it).

## [0.1.1] - 2026-10-10

### Added

- `ptox-conformance`: validate the optional `capabilities` response. A provider
  that declines capability negotiation is treated as universal and still passes;
  one that answers must return a well-formed capability object.

### Fixed

- Removed references to internal planning drafts from the shipped contract, so
  the public SDK no longer points at private repositories.
- `fetch`: `CommandFetcher` now fails when the delegated command exits 0 without
  creating the destination, instead of reporting a zero-byte success.
- `fetch`: `HttpFetcher` removes its `.part` file when a download fails
  mid-stream, not only on a hash mismatch.
- `fetch`: `sha256_file` streams the file in chunks instead of reading it whole,
  so verifying a large download can't exhaust memory.

## [0.1.0] - 2026-10-10

First public release.

### Added

- **Contracts** — `FileQuery`, `FileResult`, `Address`, `Failure`, `ErrorCode`,
  `Outcome`, and structural `Validate`.
- **Search** — the `SearchProvider` trait, `Capabilities` negotiation, and the
  concurrent `fanout` engine (per-provider timeout/limit, streaming,
  cancellation).
- **Merge** — `merge`, deduplicating and ranking results across providers.
- **Fetch** (feature `fetch`, on by default) — `Fetcher`, `FetchRegistry`,
  `HttpFetcher`, and `CommandFetcher`.
- **Provider bridge** — the NDJSON wire protocol v2 and `SubprocessProvider`,
  including bounded line reads for hostile output.
- **Engine** — the high-level `Engine` client entry point (search + retrieve).
- **Tooling** — `ptox-conformance`, the contract conformance suite (library +
  CLI).
- **Spec** — the contract, conformance requirements, JSON Schema, and test
  vectors, shipped inside `crates/ptox/spec/`.

[Unreleased]: https://github.com/despulii-bit/ptox/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/despulii-bit/ptox/releases/tag/v0.1.3
[0.1.2]: https://github.com/despulii-bit/ptox/releases/tag/v0.1.2
[0.1.1]: https://github.com/despulii-bit/ptox/releases/tag/v0.1.1
[0.1.0]: https://github.com/despulii-bit/ptox/releases/tag/v0.1.0
