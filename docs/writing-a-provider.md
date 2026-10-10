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

## 4. Report failures with the right code

`SearchProvider::search` yields `Result<FileResult, ProviderError>`. Failures are
**per item**, so one provider erroring never affects the others. Map your
backend's errors onto the well-known `ErrorCode`s:

| Situation | Code |
|---|---|
| Network/socket/DNS/TLS failure | `ErrorCode::transport()` |
| Took too long | `ErrorCode::timeout()` |
| Query you cannot answer | `ErrorCode::unsupported()` |
| Policy, rate limit, refused | `ErrorCode::refused()` |
| Bad data from the network | `ErrorCode::malformed()` |
| Anything else | `ErrorCode::internal()` |

Do not return an empty stream for a query you can't serve — yield
`unsupported` so the client can tell "no hits" from "cannot answer".

## 5. Make it offline-testable

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

## 6. Validate with the conformance suite

`ptox-conformance` checks the contract for you — termination, result shape,
error shape, attribution, and capability negotiation.

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

## 7. Checklist

- [ ] `name()` is stable and non-empty.
- [ ] `capabilities()` matches what you actually do.
- [ ] Yields `FileResult`s with at least one `Address`; leaves `provider` empty.
- [ ] Unsupported queries yield `unsupported`, not a misleading empty stream.
- [ ] Failures carry a sensible `ErrorCode` and a secret-free message.
- [ ] Preserves the untouched upstream payload in `raw` when there is one.
- [ ] All I/O is injectable; tests are offline and deterministic.
- [ ] Passes `ptox-conformance`.

Then open a PR against `ptox` or publish the provider in its own repo — see
[CONTRIBUTING.md](../CONTRIBUTING.md). Either way, the conformance suite is how
a provider proves it fits.
