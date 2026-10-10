# Writing a provider

A **provider** adapts exactly one network or index to ptox. It turns a
`FileQuery` into a stream of `FileResult`s and does nothing else: no fetching,
no ranking, no state, no user identity. This guide takes you from an empty file
to a provider that passes the conformance suite.

The runnable reference is
[`crates/ptox/examples/rust_provider.rs`](../crates/ptox/examples/rust_provider.rs);
run it with `cargo run -p ptox --example rust_provider`.

## 1. Pick a shape

There are two ways to plug in a network:

- **Rust** — implement the `SearchProvider` trait and hand it to `Engine`/`fanout`.
  Best when the provider is a Rust crate (this guide).
- **Any language** — speak the NDJSON wire format and wrap the program with
  `SubprocessProvider`. See `crates/ptox/spec/core-contract.md` §9 and
  [`crates/ptox/examples/ndjson_provider.rs`](../crates/ptox/examples/ndjson_provider.rs).

The contract is the same either way: name, capabilities, and a stream of
results where failures are per-item, never global.

## 2. Implement the trait

```rust
use async_trait::async_trait;
use ptox::{Capabilities, ErrorCode, FileQuery, FileResult, ProviderError, ResultStream, SearchProvider};

struct MyProvider { /* connection pool, proxy, data dir, … */ }

#[async_trait]
impl SearchProvider for MyProvider {
    fn name(&self) -> &str {
        "my-network" // stable; used for attribution and ranking weights
    }

    async fn capabilities(&self) -> Capabilities {
        Capabilities {
            query: vec!["text".into(), "extensions".into()],
            requires: vec!["text".into()],
            schemes: vec!["https".into()],
        }
    }

    fn search(&self, query: &FileQuery) -> ResultStream<'_> {
        // Clone up-front: the stream must not borrow `query`.
        let text = query.text.clone();
        Box::pin(async_stream::stream! {
            match self.lookup(&text).await {
                Ok(hits) => {
                    for hit in hits {
                        // Attribution is host-owned: leave `provider` empty.
                        yield Ok(FileResult::new(String::new(), vec![hit]));
                    }
                }
                Err(message) => {
                    yield Err(ProviderError::new(ErrorCode::transport(), message));
                }
            }
        })
    }
}
```

Notes that save time later:

- **Leave `FileResult.provider` empty.** The host (`fanout`/`Engine`) stamps the
  emitting provider's name. Do not set it yourself.
- **All I/O and configuration live in the struct**, injected at construction —
  never per query. Proxies, routers, and auth tokens are construction details.
- **Clone what the stream needs before the `Box::pin`.** The trait ties the
  stream's lifetime to `&self`, so a stream that borrows `query` will not
  compile. Clone the fields you need (see the example).
- **Prefer streaming.** Yield each hit as you get it; don't buffer the whole
  result set. The engine fans out concurrently and streams to the client.

## 3. Declare capabilities honestly

`Capabilities` tells the engine which queries to send and which address schemes
you emit. An empty field means "any".

- `query` — facets you can honor (`text`, `hashes`, `mimetypes`, `extensions`,
  `filename`, `size`, `published`, `lang`).
- `requires` — facets that **must** be present. A hash-only network sets
  `requires: ["hashes"]` so the engine skips it for keyword queries instead of
  spawning it.
- `schemes` — address schemes you emit (`https`, `magnet`, `ipfs`, …).

The engine probes capabilities concurrently and skips a provider whose
capabilities reject the query (the skip is reported in `SearchReport::skipped`).
This happens before every search, so keep `capabilities()` cheap and free of
I/O. If yours is expensive, a client can enable the engine's opt-in memoization
(`Engine::with_capability_cache()`); if you cache internally and the answer can
change, override `SearchProvider::invalidate_capabilities` so
`Engine::invalidate_capabilities()` can drop your cache and force a re-probe.
Do not declare a facet you don't actually honor. If you declare any `schemes`,
`ptox-conformance` checks that every address you emit uses one of them; declaring
none keeps you universal (any scheme accepted). A client can call
`Engine::provider_capabilities()` to list every provider's declarations without
running a query; a provider that does not answer in time is shown with the
universal default (`answered: false`), so this diagnostics call never fails.

## 4. Worked example: a hash-only provider

The `requires` rule is easier to see end to end. Here is a provider backed by an
infohash index: it can only answer when the query carries a `hashes` selector,
and it emits `magnet` addresses.

```rust
use async_trait::async_trait;
use ptox::search::{Capabilities, ProviderError, ResultStream, SearchProvider};
use ptox::types::{Address, ErrorCode, FileQuery, FileResult, HashAlgos};
use std::sync::Arc;

/// The I/O half: an infohash → metadata index.
#[async_trait]
trait InfohashIndex: Send + Sync {
    async fn lookup(&self, infohash: &str) -> Result<Option<Entry>, String>;
}

struct Entry {
    title: String,
    size: Option<u64>,
    magnet_uri: String,
}

struct HashIndexProvider {
    index: Arc<dyn InfohashIndex>,
}

#[async_trait]
impl SearchProvider for HashIndexProvider {
    fn name(&self) -> &str {
        "hash-index"
    }

    async fn capabilities(&self) -> Capabilities {
        // `requires` makes the engine skip this provider for keyword queries;
        // `query` documents that `hashes` is the only facet it understands.
        Capabilities {
            query: vec!["hashes".into()],
            requires: vec!["hashes".into()],
            schemes: vec!["magnet".into()],
        }
    }

    fn search(&self, query: &FileQuery) -> ResultStream<'_> {
        let infohash = query
            .hashes
            .as_ref()
            .and_then(|hashes| hashes.get("infohash"))
            .cloned();
        let index = self.index.clone();

        Box::pin(async_stream::stream! {
            // The engine only sends hash queries, but validate anyway: a
            // provider is its own last line of defense.
            let Some(infohash) = infohash else {
                yield Err(ProviderError::new(
                    ErrorCode::unsupported(),
                    "hash-index needs an `infohash` selector",
                ));
                return;
            };

            match index.lookup(&infohash).await {
                Ok(Some(entry)) => {
                    let mut result = FileResult::new(
                        String::new(), // attribution is host-owned
                        vec![Address::new("magnet", entry.magnet_uri)],
                    );
                    result.title = Some(entry.title);
                    result.size = entry.size;
                    result.hashes = Some(HashAlgos::from([(
                        "infohash".to_owned(),
                        infohash,
                    )]));
                    yield Ok(result);
                }
                Ok(None) => {} // no hits — yield nothing
                Err(message) => {
                    yield Err(ProviderError::new(ErrorCode::transport(), message));
                }
            }
        })
    }
}
```

At the engine the skip is explicit — the hash index is never spawned for a
keyword query, and is asked once the query carries a hash:

```rust
use ptox::{Engine, FileQuery};
use std::sync::Arc;

let engine = Engine::new(vec![
    Arc::new(HashIndexProvider { index: infohash_index }),
    Arc::new(keyword_index), // a `text` provider
]);

// Keyword query: `hash-index` requires `hashes`, so the engine skips it.
let report = engine.search(FileQuery::new("annual report")).await;
assert_eq!(report.skipped, vec!["hash-index".to_owned()]);

// Hash query: now the hash index participates.
let mut query = FileQuery::new("");
query.hashes = Some(HashAlgos::from([("infohash".to_owned(), hex.to_owned())]));
let report = engine.search(query).await;
assert!(report.skipped.is_empty());
assert!(report
    .results
    .iter()
    .any(|hit| hit.providers.iter().any(|name| name == "hash-index")));
```

The conformance suite sends a keyword probe by default, which a hash-only
provider legitimately answers with `unsupported`. To exercise the hash path,
pass at least one `--hash` selector — the suite then sends a hash query (and no
keyword query unless you also pass `--query`):

```sh
cargo run -p ptox-conformance -- \
  --hash infohash=0123456789abcdef0123456789abcdef01234567 \
  -- my-provider --config /etc/my.conf
```

The `requires`/`query` declarations are a routing hint at the fan-out boundary,
not a substitute for validating your inputs: the engine uses them to avoid
spawning you needlessly, but your `search` must still reject queries it cannot
serve with `unsupported` rather than returning an empty stream.

## 5. Report failures with the right code

`SearchProvider::search` yields `Result<FileResult, ProviderError>`. Failures are
**per item**, so one provider erroring never affects the others. Map your
backend's errors onto the well-known `ErrorCode`s:

| Situation | Code |
|---|---|
| Network/socket/DNS/TLS failure | `ErrorCode::transport()` |
| Backing service not ready (daemon bootstrapping) | `ErrorCode::unavailable()` |
| Took too long | `ErrorCode::timeout()` |
| Query you cannot answer | `ErrorCode::unsupported()` |
| Policy, rate limit, refused | `ErrorCode::refused()` |
| Bad data from the network | `ErrorCode::malformed()` |
| Anything else | `ErrorCode::internal()` |

Do not return an empty stream for a query you can't serve — yield
`unsupported` so the client can tell "no hits" from "cannot answer".

At runtime the fan-out enforces the structural contract (`FileResult::validate`):
a result with no `addresses`, or an address with an empty `scheme`/`uri`, is
reported to the client as a `malformed` failure instead of being delivered. (The
host fills in `provider`, so you may leave it empty.) Emitting one bad result
does not discard your other results, but a client never sees the bad one —
validate your own output in tests so this never fires.

## 6. Make it offline-testable

Keep the network behind a trait so the provider can be driven by a fake in
tests. The example does this with a `Backend` trait and a scripted
`FakeBackend`:

```rust
#[async_trait]
trait Backend: Send + Sync {
    async fn search(&self, text: &str) -> Result<Vec<Hit>, BackendError>;
}
```

The provider holds `Arc<dyn Backend>`; production wires in the real client,
tests wire in a fake that returns canned hits or failures. No sockets are opened
in the test suite.

## 7. Validate with the conformance suite

`ptox-conformance` checks the contract for you — termination, result shape,
error shape, attribution, capability negotiation, and that a `limit`-bearing
query is capped (it sends a probe with a small `limit` and fails you if you emit
more than that).

For a subprocess (any language):

```sh
cargo run -p ptox-conformance -- -- my-provider --config /etc/my.conf
```

For an in-process Rust provider, call the library:

```rust
use ptox_conformance::{check_in_process, Options};

let report = check_in_process("my-network", &provider, &Options::default()).await;
assert!(report.passed(), "{:?}", report.failures().collect::<Vec<_>>());
```

Use `--json` for machine-readable output and `--hash ALGO=VALUE` /
`--query TEXT` to send hash-only or keyword queries. The binary exits non-zero
when any check fails, so it drops straight into CI.

## 8. Checklist

- [ ] `name()` is stable and non-empty.
- [ ] `capabilities()` matches what you actually do.
- [ ] Yields `FileResult`s with at least one `Address`; leaves `provider` empty.
- [ ] Honors `limit`: a query with a `limit` returns at most that many results.
- [ ] Unsupported queries yield `unsupported`, not a misleading empty stream.
- [ ] Failures carry a sensible `ErrorCode` and a secret-free message.
- [ ] Preserves the untouched upstream payload in `raw` when there is one.
- [ ] All I/O is injectable; tests are offline and deterministic.
- [ ] Passes `ptox-conformance`.

Then open a PR against `ptox` or publish the provider in its own repo — see
[CONTRIBUTING.md](../CONTRIBUTING.md). Either way, the conformance suite is how
a provider proves it fits.
