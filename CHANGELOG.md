# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- examples: `crates/ptox/examples/client.rs`, a runnable, offline end-to-end
  client — an in-process `SearchProvider`, a ranked `Engine::search`, and an
  `Engine::fetch` of the top hit to a temp file through a fake `Fetcher` that
  still honors the fabric's sha256 verification. Declared as a test target, so
  `cargo test` exercises the whole search-then-fetch path.

### Documentation

- `docs/troubleshooting.md`: a symptom-oriented FAQ for the common warnings and
  errors — an unsupported scheme, `FetchError::AllFailed`, per-provider
  timeouts, the `unavailable`/pending state, the `file://` opt-in and
  `allowed_roots`, I2P/Tor proxy wiring, and `ptox search | head` losing state to
  a broken pipe. Linked from the root README and the client guide.

## [0.5.0] - 2026-10-10

### Added

- `config`: `[fetch.proxies]` — a proxy URL per overlay scheme (`tor`, `i2p`, …).
  `Config::fetch_registry()` registers a `ProxyFetcher` for each, so overlay
  retrieval is configurable without code.

## [0.4.0] - 2026-10-10

### Added

- `fetch`: `ProxyConfig` and `HttpFetcher::with_proxy`, routing requests through
  an HTTP (`http://`) or SOCKS5 (`socks5://` / `socks5h://`) proxy. Overlay
  networks (Tor, I2P) can now reuse the HTTP fetcher's atomic writes, sha256
  verification, progress, retries, and timeouts instead of shelling out to
  `curl`. An invalid proxy URL fails the fetch rather than silently going direct.
- `types`: `ErrorCode::unavailable()` — a well-known code for "the backing
  service is not ready yet" (e.g. a network daemon still bootstrapping),
  distinct from a real `transport` failure. `probe` classifies it as
  `ProbeStatus::Pending`, which is not a failure, so a client can wait and retry.
- `fetch`: `ProxyFetcher` — a generic fetcher for a proxy-based overlay scheme.
  It rewrites `<scheme>://<host>[/path]` to `http://<host>[/path]` and routes
  through a `ProxyConfig`, so `ProxyFetcher::i2p()` / `ProxyFetcher::tor()` are
  one line to register and inherit the HTTP fetcher's atomic writes, sha256
  verification, progress, retries, and timeouts. The contract now documents the
  overlay addressing convention: the address is the network address, not the
  transport URL.

## [0.3.0] - 2026-10-10

### Added

- `search`: `Options::deadline`, a whole-fan-out wall-clock budget. When it
  elapses, the stream stops, keeps the partial results already delivered, and
  reports every provider still running (or not yet started) as a single
  `timeout` failure — so a client need not wait for the sum of each provider's
  `Options::timeout`.
- `config`: `${VAR}` / `$VAR` expansion in provider and fetch-command
  `program`/`args` and in provider `env` values. `Config::load` expands using the
  process environment; `Config::expand_env` / `expand_env_with` do it explicitly.
  `$$` is a literal `$`; an unset variable is an error.
- `fetch`: `IpfsGatewayFetcher::with_gateways` takes an ordered list of gateways
  and fails over between them, reporting the winning gateway on
  `Fetched::address`. `new` / `with_default_gateway` remain single-gateway.

## [0.2.0] - 2026-10-10

### Added

- `search`: the fan-out now enforces the result contract at the boundary. After
  host attribution, each `FileResult` is validated (`FileResult::validate`); a
  result with no `addresses`, or an address with an empty `scheme`/`uri`, is
  reported as a `Failure{code:"malformed"}` attributed to its provider instead of
  being streamed into a client's merge/fetch path. One rejected result never
  discards the provider's remaining results. This closes the gap where the
  conformance suite checked result shape but the runtime accepted anything a
  provider emitted. `spec/core-contract.md` §8.13 documents the requirement and
  the provider guide spells out the invariant.

- `ptox-conformance`: a `limit_honored` check. The suite now sends a probe query
  carrying a small `limit` and fails a provider that returns more results than
  the selector allows, closing the gap where `limit` was documented as an upper
  bound but never exercised by the provider checks. A provider that returns
  fewer results (or none) still passes. `spec/conformance.md` §G and the provider
  guide document the requirement; the fixture provider gained a `limit-ignored`
  mode to prove the check catches a violation.

- `search`: query-free capability introspection. `Engine::provider_capabilities()`
  (and the free function `ptox::provider_capabilities`) asks every provider for
  its declared `Capabilities` concurrently, without running a query, so a
  `providers`/diagnostics command can list the query facets, requirements, and
  address schemes each provider supports. A provider that does not answer within
  the timeout is reported with the universal default and `answered: false`; the
  call never fails, and the engine's opt-in capability cache is reused.

- `config`: `Config::engine()` and `Config::engine_with_options(Options)` build a
  ready-to-run `Engine` from the parsed config — the configured providers, the
  ranking weights, and the fetch registry — so the documented `load → search`
  path is one call instead of every client hand-wiring `Engine::new(...)
  .with_options(...).with_weights(...).with_fetchers(...)`. The module doctest
  and `examples/config.rs` now use it; `docs/configuration.md` gains a “Building
  an engine” section.

- `fetch`: opt-in HTTP timeouts. `HttpFetcher` now offers
  `with_connect_timeout(Duration)` (bounds the TCP/TLS connect phase) and
  `with_read_timeout(Duration)` (bounds the idle wait for more bytes, resetting
  after each successful read, so it detects a stalled transfer without capping a
  legitimately large download), plus `connect_timeout()` / `read_timeout()`
  getters. Both are **off by default** (reqwest's default), so an existing client
  is unchanged; a timed-out attempt fails with `FetchError::Network`, removes any
  `.part` file, and participates in `RetryPolicy` like any other transient
  network error. Documented in `docs/fetching.md`.

- `fetch`: registry introspection. `FetchRegistry::schemes()` returns every
  address scheme the registry can resolve (sorted, deduplicated), so a client can
  report what it can fetch without re-deriving the set from its own config.
  `FetchRegistry` also implements `Display`, printing one line per registered
  fetcher in registration order (`name (scheme, scheme)`) for diagnostics; unlike
  `schemes()` it preserves precedence when two fetchers claim a scheme.

- Search paging. `FileQuery` gains `offset`, the per-provider counterpart to
  `limit`: a provider skips its first N matches, then returns at most `limit`, so
  a client can request the next page without re-fetching and discarding the
  previous one. `FileQuery::with_offset` builds it; it is additive on the v2
  wire (a provider that ignores it simply returns the first page) and is not a
  capability facet, because pagination is not filtering. `Options` also gains a
  global `offset`, the stream-level counterpart to `Options::limit`: it skips the
  first N result outcomes while never skipping failures, so a client driving the
  fan-out directly can window the sequence the same way.

- `Engine` capability memoization (opt-in). Capability probing runs before every
  search and probe, and a provider may compute its declarations dynamically.
  `Engine::with_capability_cache()` now asks each provider once and reuses the
  answer, for both `Engine::search`/`Engine::search_stream` and `Engine::probe`;
  `Engine::invalidate_capabilities()` drops the engine cache and calls a new
  default-no-op `SearchProvider::invalidate_capabilities` hook so a provider
  that also caches internally (e.g. `SubprocessProvider`) can be re-probed. Off
  by default, since it is only safe for providers whose capabilities are stable
  between searches.

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
  new `Config` type (opt-in feature `config`) models `[[provider]]`
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
  `ipfs` feature (opt-in), and registered by `FetchRegistry::with_defaults`
  when enabled.
- `fetch`: optional progress reporting. `Fetcher::fetch_with_progress`,
  `FetchRegistry::fetch_with_progress` / `fetch_merged_with_progress`, and
  `Engine::fetch_with_progress` report the cumulative number of retrieved bytes
  to a `ProgressFn` callback as chunks arrive, so a client can render a progress
  bar. The default trait method is a no-op (custom fetchers need not implement
  it); `HttpFetcher` reports per-chunk totals, and `CommandFetcher` polls the
  destination file size while its command runs, so command-delegated schemes
  such as magnets report progress too.
- Property tests for the NDJSON wire boundary. Deterministic pseudo-random
  bytes and values now drive `parse_line` (never panics, tolerates unknown
  fields, has fixed `done`/`error`/`result` precedence, survives pathological
  nesting) and `read_line_bounded` (never returns a line past its bound,
  round-trips arbitrary/multi-byte UTF-8 split across buffer boundaries, strips
  CRLF, and rejects invalid UTF-8). No fuzzing dependency is added; the PRNG is
  a fixed splitmix64 so failures are reproducible.

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

[Unreleased]: https://github.com/despulii-bit/ptox/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/despulii-bit/ptox/releases/tag/v0.5.0
[0.4.0]: https://github.com/despulii-bit/ptox/releases/tag/v0.4.0
[0.3.0]: https://github.com/despulii-bit/ptox/releases/tag/v0.3.0
[0.2.0]: https://github.com/despulii-bit/ptox/releases/tag/v0.2.0
[0.1.3]: https://github.com/despulii-bit/ptox/releases/tag/v0.1.3
[0.1.2]: https://github.com/despulii-bit/ptox/releases/tag/v0.1.2
[0.1.1]: https://github.com/despulii-bit/ptox/releases/tag/v0.1.1
[0.1.0]: https://github.com/despulii-bit/ptox/releases/tag/v0.1.0
