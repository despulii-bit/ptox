# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `fetch`: opt-in resuming of interrupted HTTP downloads.
  `HttpFetcher::with_resume(true)` re-hashes an existing `<destination>.part`
  and continues it with a `Range` request instead of restarting: it validates
  `Content-Range`, ignores servers that disregard the range (`200`), and
  finishes a partial the server reports as already complete (`416`). A transient
  failure keeps the partial, so a retry or a later process resumes instead of
  throwing bytes away, and the final sha256 still covers the whole file. A
  `<destination>.part.src` sidecar records the source address so two mirrors
  never splice. Off by default, like `RetryPolicy`.

- `fetch`: an opt-in `file://` fetcher. A new `FileFetcher` (behind the
  **off-by-default** `file` feature) copies a local file to the destination with
  the same guarantees as the HTTP fetcher — a `.part` file, a streamed sha256,
  verification against `expected_sha256`, an atomic rename, and per-chunk
  progress. `file:///abs/path` and `file://localhost/abs/path` are accepted (any
  other authority is rejected), `%XX` escapes are decoded, and directories fail
  with `FetchError::Io`. A new `FetchError::AccessDenied` is returned for a path
  outside every configured root. `FileFetcher::with_allowed_roots` restricts the
  fetcher to a data directory; `FileFetcher::new` allows any readable path. The
  fetcher is registered by `FetchRegistry::with_defaults` and
  `Config::fetch_registry` when the feature is enabled. Because a `file://`
  address grants read access to the filesystem, the feature is off by default and
  the policy is documented in `docs/fetching.md`.

- Conformance: emitted address schemes are now cross-checked against declared
  capabilities. `ptox-conformance` reports an `address_schemes_declared` check
  (for both in-process and subprocess providers) that fails when a provider
  declares `Capabilities::schemes` but emits an address outside that set. A
  provider that declares no schemes stays universal. `Capabilities` gains an
  `allows_scheme` helper so clients can apply the same rule.

- `FileQuery` builder methods. `FileQuery::new(text)` can now be chained with
  `with_hashes` / `with_hash`, `with_mimetypes` / `with_mimetype`,
  `with_extensions` / `with_extension`, `with_filename`, `with_size_range` /
  `with_size_min` / `with_size_max`, `with_published_after` /
  `with_published_before`, `with_lang`, `with_limit`, and `with_extras` /
  `with_extra`. Singular methods append; plurals replace; empty collections clear
  the field. Extension helpers normalize to the contract's lowercased, dotless
  form (`.PDF` → `pdf`), so clients no longer have to reimplement selector
  plumbing.

- Per-provider search metrics. `Engine::search` now returns `SearchReport::metrics`
  — a `ProviderMetric` per provider with its result count, failure count, and
  elapsed time. A new `fanout_with_metrics` fills a shared `ptox::Metrics`
  collector, so clients driving `fanout` directly get the same data. Metrics are
  counts and timings only: they never carry query text or result content.

- Documentation: `docs/fetching.md`, an end-to-end guide to the retrieval side —
  the scheme registry, atomic write + hash verification, multi-source failover,
  progress reporting, the `CommandFetcher` `{dest}` contract (and the 0-byte
  "success" it used to hide), IPFS gateway policy, and opt-in HTTP retries.
  Linked from both READMEs.

- `config`: a canonical, versioned provider/fetcher configuration schema. The
  new `Config` type (feature `config`, on by default) models `[[provider]]`
  commands, `[ranking] weights`, and `[fetch.commands.<scheme>]` delegations in
  one documented TOML shape. `Config::load` / `from_toml_str` parse and validate
  it (supported `version`, unique non-empty provider names, non-empty programs,
  finite non-negative weights), `to_toml_string` round-trips it back, and
  `providers()` / `fetch_registry()` / `merge_options()` turn it into the pieces
  a client needs. The shape is a stability contract like `MergedResult`'s:
  fields are only added within a schema version, new fields default, and unknown
  fields are ignored. See `docs/configuration.md` and
  `crates/ptox/examples/config.rs`.
- `merge::ResultRef`: a small, versioned persistence handle that decouples client
  state from the full `MergedResult` serde shape. It keeps only what is needed to
  retrieve and name a file — a stable `id`, `title`, `mimetype`, `size`,
  `hashes`, and `addresses` — plus its own `version`. `ResultRef::from_merged` /
  `to_merged` convert both ways, and `merge::result_id` derives a deterministic
  identity (a content hash when present, else title+size, else the preferred
  URI). `ResultRef`'s serialized shape is a stability contract, exactly like
  `MergedResult`'s.
- `fetch`: an optional HTTP retry policy. `HttpFetcher::with_retry` takes a
  `RetryPolicy` (off by default) that replays idempotent GETs after transient
  failures — connection errors, mid-stream read errors, and `408`/`429`/`5xx`
  responses — with exponential backoff (`base_delay * 2^(n-1)`, capped by
  `max_delay`). Client errors (`4xx`) and hash mismatches are never retried.
  Every attempt restarts from a clean `.part` file, and progress stays monotonic
  across retries.
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
