# Building a client

A **client** (CLI, GUI, agent, service) is the software a person actually uses.
It owns the user experience and the policy; the SDK owns the contracts and the
engines. A client does four things:

1. gathers **providers** (one per network/index — separate software),
2. builds an [`Engine`] (fan-out search + retrieval),
3. runs searches and ranks/presents the results,
4. fetches a chosen result to disk.

This guide is the counterpart to [writing-a-provider.md](writing-a-provider.md).
The contract itself is `crates/ptox/spec/core-contract.md`; retrieval policy is
[fetching.md](fetching.md); client wiring is [configuration.md](configuration.md).

The runnable pieces referenced here ship with the SDK: a client assembled from a
config is `crates/ptox/examples/config.rs`.

## 1. What the client owns (and what it must not)

The SDK deliberately does **not** do these — they are the client's job:

- **Policy:** which providers to run, ranking weights, timeouts, page sizes,
  where downloads go and what they are named.
- **Discovery:** finding/installing providers. The SDK is handed providers; it
  does not go looking for them.
- **Presentation and persistence:** how results are shown and stored between a
  search and a fetch.
- **Fetching is explicit, never automatic.** The core never fetches an `Address`
  it returns — a search is a search. The client decides when to retrieve.

Two things the SDK *does* handle, so a client shouldn't reimplement them:
**attribution** (the host stamps every result with its provider's name) and
**result validation** (a structurally invalid result becomes a `malformed`
failure instead of reaching the client).

## 2. Add the dependency

```toml
[dependencies]
ptox = "0.2"
```

`fetch` (retrieval) is on by default. For a **search-only** client, turn default
features off:

```toml
ptox = { version = "0.2", default-features = false }
```

Three features are **opt-in**:

```toml
ptox = { version = "0.2", features = ["config"] }   # canonical TOML loader
ptox = { version = "0.2", features = ["ipfs"] }      # ipfs:// gateway fetcher
ptox = { version = "0.2", features = ["file"] }      # file:// fetcher (local reads)
```

## 3. Gather providers

Providers are separate software. A client ships none; it configures the ones the
user has.

**Any language, via the NDJSON wire format:**

```rust
use ptox::SubprocessProvider;

let provider = SubprocessProvider::new("archive-org", "node")
    .arg("/opt/ptox/adapters/archive_org.js")
    .env("ARCHIVE_ORG_KEY", std::env::var("ARCHIVE_ORG_KEY").unwrap_or_default());
```

**In-process Rust:** implement `SearchProvider` and pass the value directly (see
[writing-a-provider.md](writing-a-provider.md)).

Either kind becomes an `Arc<dyn SearchProvider>`:

```rust
use std::sync::Arc;
use ptox::SearchProvider;

let providers: Vec<Arc<dyn SearchProvider>> = vec![Arc::new(provider)];
```

If you adopt the canonical config ([§11](#11-optional-adopt-the-canonical-config)),
`Config::providers()` builds this list for you.

## 4. Build an engine

```rust
use std::collections::HashMap;
use std::time::Duration;
use ptox::{Engine, Options};

let mut weights = HashMap::new();
weights.insert("archive-org".to_owned(), 1.2); // policy: prefer this source

let engine = Engine::new(providers)
    .with_options(Options {
        timeout: Some(Duration::from_secs(10)), // per-provider budget
        ..Options::default()
    })
    .with_weights(weights);
```

`Options` is the client's policy knob set: `timeout` (per provider), `deadline`
(a whole-fan-out wall-clock budget — when it elapses the stream stops, keeps the
partial results already delivered, and reports every provider still running as a
`timeout` failure), `limit` and `offset` (global, over the streamed result
sequence), and `concurrency` (max providers polled at once). Always set a
`timeout` in an interactive client; add a `deadline` to bound the whole search.

If a provider's `capabilities()` is expensive and stable, `Engine::with_capability_cache()`
asks each provider once; `Engine::invalidate_capabilities()` clears it.

## 5. Run a search — two shapes

You run one provider set but want two different things, and the engine gives you
both. Pick by UX, not by habit.

### 5a. Ranked, deduplicated list — `Engine::search`

Waits for every provider, deduplicates across sources, ranks, and returns one
list. Use this for a `ptox search`-style result table.

```rust
let report = engine.search(ptox::FileQuery::new("annual report")).await;

for (index, result) in report.results.iter().enumerate() {
    let title = result.title.as_deref().unwrap_or("<untitled>");
    println!("{:>3}. [{}] {title}", index + 1, result.providers.join(","));
    for address in &result.addresses {
        println!("      {} {}", address.scheme, address.uri);
    }
}
```

`SearchReport` carries four things:

| field | meaning |
|---|---|
| `results` | ranked `MergedResult`s (deduplicated across providers) |
| `failures` | one `Failure` per provider that errored (never aborts the search) |
| `skipped` | providers whose declared capabilities rejected the query |
| `metrics` | per-provider result/failure counts and elapsed time |

### 5b. Live results — `Engine::search_stream`

Returns a `SearchStream` immediately; results arrive as each provider produces
them. Use this for an interactive/streaming UI so a fast provider is visible
before a slow one finishes.

```rust
use futures::StreamExt;
use ptox::Outcome;

let mut stream = engine.search_stream(ptox::FileQuery::new("annual report")).await;
println!("skipped: {:?}", stream.skipped);

while let Some(outcome) = stream.next().await {
    match outcome {
        Outcome::Result(result) => println!("hit: {:?}", result.title),
        Outcome::Failure(failure) => {
            eprintln!("{} failed: {}: {}", failure.provider, failure.error.code, failure.error.message)
        }
    }
}
```

Outcomes are **not** merged or ranked (merging needs every result). Dropping the
stream cancels in-flight providers. `search_stream` returns the same
capability-based `skipped` list up front.

## 6. Handle partial failure like a feature

A search is a fan-out: providers fail independently, and that must never make the
whole search fail. Surface it instead of hiding it.

```rust
// After Engine::search:
let skipped = report.skipped.len();
let failed = report.failures.len();
println!(
    "\n{} result(s), {skipped} skipped, {failed} provider error(s)",
    report.results.len()
);
for failure in &report.failures {
    eprintln!("  {}: {} ({})", failure.provider, failure.error.message, failure.error.code);
}
```

`report.failures` and `report.skipped` are also how you distinguish "no hits"
from "couldn't ask". For a per-provider health view without a query, use
`Engine::provider_capabilities()`; to ask each provider one probe query and
classify the answer (`Results`/`Empty`/`Skipped`/`Failed`/`Timeout`), use
`Engine::probe(query)`.

## 7. Ranking, deduplication, and paging

- **Dedup** keys, in order: a content hash (`sha256`, `sha1`, `infohash`, `md5`,
  `cid`), else normalized title + size. One file seen by three providers becomes
  one `MergedResult` whose `providers` lists all three and whose `addresses`
  holds every mirror.
- **Rank** is `MergedResult::score` (higher is better) and is only comparable
  *within one `merge`*. The default rewards multi-source agreement and metadata
  completeness; `with_weights` biases per provider. If you want your own
  ordering, call `ptox::merge` directly on raw `FileResult`s.
- **Paging** is **best-effort and per-provider**: `FileQuery::with_limit`/
  `with_offset` ask each provider to skip then cap, and `Options::limit`/`offset`
  window the global stream. Because results are neither ordered nor
  deterministic, a paged query is meaningful within one provider/query; for a
  stable page, snapshot the first result set and page over the snapshot.

## 8. Persist results between search and fetch

A common client flow is `search` → show a numbered list → `get N`. Persist the
**smallest stable handle**, not the whole in-memory aggregate.

```rust
use ptox::{MergedResult, ResultRef};

// After search: store one handle per result.
let refs: Vec<ResultRef> = report.results.iter().map(ResultRef::from_merged).collect();
let blob = serde_json::to_string(&refs)?; // -> your client state

// Later: resolve it back to something fetchable.
let refs: Vec<ResultRef> = serde_json::from_str(&blob)?;
let chosen: MergedResult = refs[2].to_merged();
```

`ResultRef` keeps only the fetch-essential fields plus a schema `version`
(`ResultRef::is_current()` lets you check it) and a stable `id`
(`merge::result_id`). Both `ResultRef` and `MergedResult` have a **stability
contract**: within a major version fields are only added, so an upgrade does not
invalidate stored state. Prefer `ResultRef`; it is smaller and deliberately
decoupled from the SDK's internal aggregate.

## 9. Fetch a result

Fetching is explicit and goes through the engine so you inherit **multi-source
failover**, **sha256 verification**, and **atomic writes**.

```rust
use ptox::FetchError;

let destination = ptox::suggest_filename(&chosen); // safe default name
match engine.fetch(&chosen, &destination).await {
    Ok(fetched) => println!("saved {} ({}) bytes", fetched.path.display(), fetched.bytes),
    Err(FetchError::AllFailed { count, .. }) => eprintln!("all {count} mirrors failed"),
    Err(FetchError::NoAddress) => eprintln!("result has no address"),
    Err(error) => eprintln!("fetch failed: {error}"),
}
```

- **Address preference / failover:** `https` > `http` > `ipfs` > `magnet` >
  other. `fetch_merged` tries each mirror until one succeeds and records the
  winner on `Fetched::address`.
- **Integrity:** if the result or address carries a `sha256`, the retrieved bytes
  are verified; a mismatch removes the partial file.
- **Naming:** `suggest_filename` is deterministic and offline (title → address
  basename → `download.bin`, sanitized). It is a bare file name, never a path, so
  join it to your own destination directory (and handle collisions yourself).
- **Progress:** for large downloads, pass a callback:

  ```rust
  engine
      .fetch_with_progress(&chosen, &destination, &|bytes| {
          // cheap, non-blocking: update a bar or counter
          eprint!("\r{bytes} bytes");
      })
      .await?;
  ```

  Progress is cumulative and stays monotonic across failover and retries.
- **Which schemes work:** `engine`'s registry answers — `FetchRegistry::schemes()`
  (sorted, deduped) or print the registry (it implements `Display`). Schemes with
  no built-in fetcher (e.g. `magnet`) can be delegated to an external command via
  the config; see [fetching.md](fetching.md).

## 10. Diagnostics

```rust
// What can each provider do, without sending a query?
for row in engine.provider_capabilities().await {
    println!("{}: schemes={:?} answered={}", row.provider, row.capabilities.schemes, row.answered);
}

// Is each provider alive? (one probe query, classified per provider)
for probe in engine.probe(ptox::FileQuery::new("ptox selftest")).await {
    println!("{}: {:?}", probe.provider, probe.status);
}
```

Both never fail: a provider that does not answer is reported with the universal
default (`answered: false`) or a `Timeout` status, so a `providers`/health
command always has a row per provider.

## 11. (Optional) Adopt the canonical config

If you don't want to invent your own TOML, the SDK ships one (`config` feature):

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

```rust
use ptox::Config;

let config = Config::load("~/.config/ptox/providers.toml")?;
let engine = config.engine();                          // providers + weights + fetchers
let engine = config.engine_with_options(options);      // …plus your policy knobs
let registry = config.fetch_registry();                // just the fetchers
```

`Config::from_toml_str` / `to_toml_string` parse and write it (`config init`
commands), `Config::validate()` reports problems, and the serialized shape is a
stability contract. `Config::load` also expands `${VAR}` / `$VAR` references from
the environment, so one config is portable across machines. Full reference:
[configuration.md](configuration.md).

## 12. Test a client offline

Keep the SDK at the seam so you never hit a network in tests:

- **Fake providers:** implement `SearchProvider` in the test (canned hits, a
  scripted error, a hang) and build an `Engine` over them. The SDK's own
  `crates/ptox/tests/engine.rs` and `crates/ptox/tests/fanout.rs` do exactly
  this.
- **Fake subprocess provider:** a small shell script that prints NDJSON
  `result`/`done` lines, wrapped with `SubprocessProvider`. Drive the whole
  `search → state → get` flow with an isolated cache dir.
- **Fake fetcher:** register a `Fetcher` for a test scheme in a `FetchRegistry`
  and assert the destination your client chose.

No sockets, no real providers, deterministic.

## 13. Minimal client, end to end

```rust
use std::sync::Arc;
use std::time::Duration;
use ptox::{Engine, FileQuery, Options, SearchProvider, SubprocessProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = Arc::new(
        SubprocessProvider::new("archive-org", "node").arg("/opt/ptox/archive_org.js"),
    ) as Arc<dyn SearchProvider>;

    let engine = Engine::new(vec![provider]).with_options(Options {
        timeout: Some(Duration::from_secs(10)),
        ..Options::default()
    });

    let report = engine.search(FileQuery::new("annual report").with_mimetype("application/pdf")).await;
    for result in &report.results {
        println!("{:?} from {:?}", result.title, result.providers);
    }
    println!("{} failed, {} skipped", report.failures.len(), report.skipped.len());

    if let Some(first) = report.results.first() {
        let destination = ptox::suggest_filename(first);
        let fetched = engine.fetch(first, &destination).await?;
        println!("saved {} ({} bytes)", fetched.path.display(), fetched.bytes);
    }
    Ok(())
}
```

## 14. Checklist

- [ ] Providers are configured/injected; the client ships none and discovers none.
- [ ] A per-provider `timeout` is set.
- [ ] The UI does not block on the slowest provider (`search_stream`, or an
      explicit "waiting…" state) — or blocking is a deliberate choice.
- [ ] Partial results, `failures`, and `skipped` are surfaced, not swallowed.
- [ ] Results are deduplicated/ranked by the SDK (or a custom `merge`), not by hand.
- [ ] Client state persists `ResultRef`, and `is_current()` is checked on load.
- [ ] Fetching is explicit and uses `Engine::fetch` (failover + verification +
      atomic write); `AllFailed`/`NoAddress` are handled.
- [ ] Large downloads show progress via `fetch_with_progress`.
- [ ] No `Address` is ever fetched automatically as a side effect of searching.
- [ ] The SDK version is pinned, and the [CHANGELOG](../CHANGELOG.md) is read on upgrade.

[`Engine`]: https://docs.rs/ptox/latest/ptox/struct.Engine.html
