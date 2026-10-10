# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `ptox-conformance`: validate the optional `capabilities` response. A provider
  that declines capability negotiation is treated as universal and still passes;
  one that answers must return a well-formed capability object.

### Fixed

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

[Unreleased]: https://github.com/despulii-bit/ptox/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/despulii-bit/ptox/releases/tag/v0.1.0
