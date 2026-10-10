# Troubleshooting

A symptom-oriented FAQ for the warnings and errors clients hit most often. Every
entry is *cause → fix*, with a pointer into the relevant guide:

- search and providers — [writing-a-provider.md](writing-a-provider.md)
- retrieval — [fetching.md](fetching.md)
- config — [configuration.md](configuration.md)
- assembling a client — [building-a-client.md](building-a-client.md)

Most failures are **per-provider or per-address**; the SDK reports them and keeps
going rather than aborting a whole search or fetch. Nothing here needs a change
to the contract.

## First: which error am I looking at?

Three error families, three layers:

| Type | Layer | What it means |
|---|---|---|
| `ErrorCode` | search | A provider's own classification (`timeout`, `transport`, `unavailable`, `malformed`, …), carried on a `Failure` |
| `FetchError` | retrieval | A fetch failed (`Unsupported`, `AllFailed`, `Network`, `HashMismatch`, …) |
| `ConfigError` | config | The TOML could not be read, parsed, or validated |

A provider failure is **not** a search failure: `SearchReport::failures` /
`SearchStream` `Outcome::Failure` carry it and the other providers still deliver.
A `FetchError` is a single retrieval job, or all mirrors of one result.

## `unsupported address scheme `…`` (`FetchError::Unsupported`)

**Symptom.** Fetching a result fails with
`` unsupported address scheme `magnet` `` (or `i2p`, `tor`, `file`, `ipfs`, …).

**Cause.** No fetcher is registered for that scheme. This is a *client* policy
gap, not a provider bug: a provider is free to emit a scheme it declares, but the
client decides whether it can retrieve it.

**Fix.** Find out what the registry can actually resolve, then register a
fetcher or enable the feature:

```rust
let registry = ptox::FetchRegistry::with_defaults();
assert_eq!(registry.schemes(), vec!["http".into(), "https".into()]); // default build
println!("{registry}"); // one line per fetcher: name (scheme, scheme)
```

- Built-ins: `http`/`https`. Add `ipfs://` with the `ipfs` feature, `file://`
  with the `file` feature (see below).
- Overlays (`i2p`, `tor`) and other schemes: register a `ProxyFetcher` or a
  `CommandFetcher` (delegate to `aria2c`, a local `ipfs get`, …). With config,
  `[fetch.commands.<scheme>]` / `[fetch.proxies]` make
  `Config::fetch_registry()` register them for you.
- `Fetcher::schemes()` precedence is **first registered wins**; check
  `fetcher_for(scheme)` when a built-in seems to shadow your fetcher.

A scheme a provider declares in `Capabilities::schemes` is *not* a promise that
the client can fetch it — see “emitted address schemes” in
[fetching.md § The scheme registry](fetching.md#the-scheme-registry).

## `` all N address(es) failed `` (`FetchError::AllFailed`)

**Symptom.** `Engine::fetch` / `FetchRegistry::fetch_merged` fails with
`AllFailed { count, errors }` even though the result listed several mirrors.

**Cause.** Failover is working: every `Address` was tried in preference order
(`https` > `http` > `ipfs` > `magnet` > other) and all failed. The per-attempt
errors are preserved in `errors`, in the order they were tried.

**Fix.** Don't discard `errors` — it names the real cause per mirror:

```rust
match engine.fetch(&result, &destination).await {
    Err(ptox::FetchError::AllFailed { count, errors }) => {
        eprintln!("all {count} mirrors failed:");
        for error in &errors {
            eprintln!("  {error}");
        }
    }
    Err(error) => eprintln!("fetch failed: {error}"),
    Ok(fetched) => println!("{} bytes from {}", fetched.bytes, fetched.address.uri),
}
```

Common inner causes and their fixes:

| Inner error | Meaning | Fix |
|---|---|---|
| `Network` | DNS/TLS/connect/proxy/timeout | Check the host and any proxy; bound it with a connect/read timeout |
| `HashMismatch` | a mirror served the wrong bytes | Treat the content as changed; **do not** bypass verification |
| `Unsupported` | a mirror used a scheme the client cannot fetch | Register that scheme (above) |
| `InvalidAddress` | a malformed `ipfs://` / overlay address | Fix the provider's address encoding |

Do not confuse `AllFailed` with its neighbours: `NoAddress` means the result had
**no** addresses to try, and a single `InvalidAddress` can appear on its own when
only one address was present. For flaky mirrors, opt into retries
(`HttpFetcher::with_retry`) and/or resume (`HttpFetcher::with_resume`); see
[fetching.md § Retrying transient HTTP failures](fetching.md#retrying-transient-http-failures).

## A provider reports `timeout` (or the search never finishes)

**Symptom.** `report.failures` contains `code: timeout`, a `ProviderMetric` shows
a large `elapsed`, or `Engine::search` waits far longer than expected.

**Cause.** Search has two independent budgets:

- `Options::timeout` — the **per-provider** budget. A provider that streams past
  it is reported as a `timeout` failure, and the others are unaffected.
- `Options::deadline` — the **whole fan-out** wall-clock budget. When it elapses,
  the stream stops, keeps the results already delivered, and reports every
  provider still running (or not yet started) as a single `timeout` failure.

**Fix.** Set both in an interactive client: a short `timeout` so one slow
provider cannot dominate, and a `deadline` so the sum of providers cannot.

```rust
use ptox::{Engine, Options};
use std::time::Duration;

let engine = Engine::new(providers).with_options(Options {
    timeout: Some(Duration::from_secs(10)),  // per provider
    deadline: Some(Duration::from_secs(20)), // whole search
    ..Options::default()
});
```

`Engine::probe` uses the same `timeout` and classifies a slow provider as
`ProbeStatus::Timeout`; `Engine::provider_capabilities` returns
`answered: false` (with the universal fallback) when a provider does not answer
in time. Neither call fails.

**A *fetch* hanging is separate.** `Options::timeout` does not apply to
retrieval. `HttpFetcher` uses reqwest defaults, which include **no timeout**, so
bound it yourself with `with_connect_timeout` (TCP/TLS phase) and
`with_read_timeout` (idle wait between reads). A timed-out attempt surfaces as
`FetchError::Network` and participates in the retry loop.
[fetching.md § Bounding HTTP timeouts](fetching.md#bounding-http-timeouts).

## The provider is `unavailable`, or the probe says `Pending`

**Symptom.** A provider's `Failure` has `code: unavailable`, or `Engine::probe`
reports `ProbeStatus::Pending` for it.

**Cause.** `ErrorCode::unavailable()` is the provider saying “my backing service
is not ready yet” — for example an I2P/Tor daemon still bootstrapping, or an
index that is still warming. It is deliberately distinct from `transport`
(a real network failure).

**Fix.** Treat it as **temporary**: `ProbeStatus::Pending.is_failure()` is
`false`, so a health view should show “starting…”, and a client should wait and
retry rather than disable the provider. A provider should emit `unavailable`
instead of `transport` while it is coming up, and switch to a normal result or a
real error once ready. Use the classification to drive backoff:

```rust
use ptox::ProbeStatus;

for probe in engine.probe(ptox::FileQuery::new("ptox selftest")).await {
    if probe.status == ProbeStatus::Pending {
        eprintln!("{} is still starting; will retry", probe.provider);
    } else if probe.status.is_failure() {
        eprintln!("{} is unhealthy: {:?}", probe.provider, probe.status);
    }
}
```

## `file://` fetches fail or expose too much (`AccessDenied`)

**Symptom.** `file://` addresses are `Unsupported`; or a local fetch fails with
`FetchError::AccessDenied`; or you are worried a provider can read arbitrary
paths.

**Cause.** `FileFetcher` is behind the **off-by-default** `file` feature
(a filesystem-read capability). Once enabled, `FileFetcher::new()` copies *any*
path the process can read — a provider emitting `file:///etc/passwd` would make
the client copy it. `with_allowed_roots` is the restriction.

**Fix.**

```toml
# Cargo.toml — opt in explicitly.
ptox = { version = "0.2", features = ["file"] }
```

```rust
use ptox::{FetchRegistry, FileFetcher};
use std::sync::Arc;

let mut registry = FetchRegistry::new();
registry.register(Arc::new(FileFetcher::new()));                        // any path
// Or restrict to one data directory (recommended):
// registry.register(Arc::new(FileFetcher::with_allowed_roots(vec!["/srv/data".into()])));
```

`with_allowed_roots` resolves symlinks and refuses any source outside the named
directories with `AccessDenied`. Note that `Config::fetch_registry()` registers
the unrestricted `FileFetcher::new()` when the feature is on — to restrict it,
build the registry yourself as above. `file://` URIs parse as `file:///abs/path`
or `file://localhost/abs/path` (any other authority is `InvalidAddress`, `%XX`
is decoded), and a directory fails with `Io`. Only enable this for **trusted,
local providers**; see
[fetching.md § Local-file policy](fetching.md#local-file-policy).

## I2P/Tor fetches fail — proxy configuration

**Symptom.** Fetching an `i2p://` or `tor://` address fails with
`FetchError::Network`, or overlay names do not resolve.

**Cause.** Overlay networks are reached through a **local daemon's proxy**
(i2pd's HTTP proxy, Tor's SOCKS port). If the daemon is not running, or the proxy
URL/DNS mode is wrong, the rewritten HTTP request fails.

**Fix.** Run the daemon, then register a `ProxyFetcher` for the scheme the
provider emits — or configure it in TOML:

```toml
[fetch.proxies]
i2p = "http://127.0.0.1:4444"       # i2pd HTTP proxy
tor = "socks5h://127.0.0.1:9050"    # Tor SOCKS, remote DNS
```

```rust
use ptox::{FetchRegistry, ProxyFetcher};
use std::sync::Arc;

let mut registry = FetchRegistry::new();
registry.register(Arc::new(ProxyFetcher::i2p())); // = http://127.0.0.1:4444
registry.register(Arc::new(ProxyFetcher::tor())); // = socks5h://127.0.0.1:9050
```

Checklist:

- The **address is the network address** (`tor://<onion>/file`,
  `i2p://<b32>.i2p`), *not* the proxy transport URL; `ProxyFetcher` owns the
  rewrite to `http://<host>[/path]`.
- Use **`socks5h://`** (remote DNS) for `.onion`/`.i2p` names; plain `socks5://`
  resolves names locally and will fail.
- Keep the shared `http`/`https` entry **unproxied**, so clearnet stays direct.
- An invalid proxy URL fails with `FetchError::Network` rather than silently
  going direct — so a `Network` error is often "the proxy is down", not "the
  destination is down".
- Need timeouts/retries/custom `User-Agent` on the proxy path? Pass an
  `HttpFetcher::with_proxy(...)` through `ProxyFetcher::with_http_fetcher`.

[fetching.md § Proxies and overlay networks](fetching.md#proxies-and-overlay-networks-i2p-tor-),
[configuration.md § `[fetch.proxies]`](configuration.md#fetchproxiesscheme).

## `ptox search | head` exits early and `ptox get` says “no previous search”

**Symptom.** Piping a search into a truncating consumer (`ptox search … | head`)
shows the first lines, but the “run `ptox get <number>`” hint is missing and a
later `ptox get 1` reports no previous search.

**Cause.** The CLI renders the result list to stdout and **then** saves the
result set to its cache. When `head` has seen enough lines it closes the pipe;
the next write to stdout fails with a broken pipe (`EPIPE`). On Unix, Rust's
runtime ignores `SIGPIPE`, so the `println!` *panics* instead of the process
being killed quietly — and the panic unwinds the process **before** `state::save`
runs. The pipe closing is expected; losing the state because output is ordered
before persistence is the bug.

**Fix.** For the CLI, avoid a truncating pipe on the same invocation:

```sh
ptox search "annual report" --limit 10      # ask for fewer results, don't truncate
ptox search "annual report" > results.txt   # then: head results.txt
ptox search "annual report" | less          # a consumer that keeps reading
```

For client authors the lesson generalizes: **persist the result set before (or
independently of) rendering it**, and treat stdout errors as non-fatal. A client
should not let a closed pager or a `head` take down the process that owns the
search state. (The same applies to any `println!`-heavy loop: writing state
first, or catching the broken pipe, means `ptox get` still works after
`| head`.)

## See also

- [writing-a-provider.md](writing-a-provider.md) — `ErrorCode`s a provider should emit.
- [fetching.md](fetching.md) — scheme registry, failover, retries, proxies, `file://`.
- [configuration.md](configuration.md) — provider, command, and proxy wiring.
- [building-a-client.md](building-a-client.md) — timeouts, partial failure, persistence.
