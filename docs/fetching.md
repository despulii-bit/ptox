# Fetching files

The **search** half of ptox produces `Address`es; the **fetch** half turns one of
those addresses into bytes on disk. This guide covers the retrieval side end to
end: the scheme registry, how a fetch is performed, multi-source failover,
progress, delegated commands, the IPFS gateway, and retries.

The code lives in `ptox::fetch`, behind the `fetch` feature (on by default). The
`ipfs` gateway fetcher is behind the opt-in `ipfs` feature, a
local-filesystem fetcher is behind the **off-by-default** `file` feature, and the
canonical TOML wiring for fetchers is behind the opt-in `config` feature (see
[`configuration.md`](configuration.md)).

## Quick start

Most clients start from a `MergedResult` (a ranked search hit) and ask the
`Engine` to retrieve it:

```rust
use ptox::{Engine, MergedResult};

# async fn run(engine: &Engine, result: &MergedResult) -> Result<(), ptox::FetchError> {
let fetched = engine.fetch(result, "download.bin").await?;
println!("{} bytes -> {}", fetched.bytes, fetched.path.display());
# Ok(())
# }
```

`Engine::fetch` delegates to its `FetchRegistry`. You can also drive the registry
directly when you have an `Address` rather than a merged result:

```rust
use ptox::{Address, FetchRegistry};

# async fn run() -> Result<(), ptox::FetchError> {
let registry = FetchRegistry::with_defaults();
let address = Address::new("https", "https://example.org/file.bin");
let fetched = registry.fetch(&address, "file.bin", None).await?;
println!("{} bytes", fetched.bytes);
# Ok(())
# }
```

The third argument to `FetchRegistry::fetch` is an optional expected sha256. Pass
`Some(...)` to verify the download, or `None` to skip verification. Prefer
`Engine::fetch` / `FetchRegistry::fetch_merged` when you have a `MergedResult`:
they pick up the result's own sha256 and try its mirrors.

## The fetcher contract

A network is a type implementing [`Fetcher`]:

```rust
use async_trait::async_trait;

#[async_trait]
pub trait Fetcher: Send + Sync {
    fn name(&self) -> &str;                 // stable, for diagnostics
    fn schemes(&self) -> &[String];         // schemes this fetcher resolves
    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError>;

    async fn fetch_with_progress(
        &self,
        request: &FetchRequest,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, FetchError> { /* defaults to fetch() */ }
}
```

`FetchRequest` carries the `address`, the `destination` path, and an optional
`expected_sha256`. `Fetched` reports the written `path`, the number of `bytes`,
the computed `sha256` (when available), and the `address` the bytes came from.

Everything is per-request: a fetcher is `Send + Sync` and must not carry mutable
per-fetch state. Proxies, gateways, and credentials are construction details.

## The scheme registry

`FetchRegistry` dispatches by address scheme. Each registered fetcher advertises
the schemes it handles; the **first** one registered for a scheme wins.

```rust
use ptox::{FetchRegistry, HttpFetcher};
use std::sync::Arc;

let mut registry = FetchRegistry::new();
registry.register(Arc::new(HttpFetcher::new()));
assert!(registry.fetcher_for("https").is_some());
assert!(registry.fetcher_for("magnet").is_none());
```

- `FetchRegistry::new()` is empty.
- `FetchRegistry::with_defaults()` adds the built-ins:
  - `http` and `https` — [`HttpFetcher`] (always, with the `fetch` feature);
  - `ipfs` — [`IpfsGatewayFetcher`] using `DEFAULT_IPFS_GATEWAY`, when the `ipfs`
    feature is enabled;
  - `file` — [`FileFetcher`] (any readable path), when the `file` feature is
    enabled. Register a `FileFetcher::with_allowed_roots` yourself to restrict it
    (see [Local files](#local-files-file)).
- `register` returns `&mut Self`, so calls chain.
- `fetcher_for(scheme)` returns the `Arc<dyn Fetcher>` that would handle it, or
  `None`.
- `schemes()` returns every scheme the registry can resolve, sorted and
  deduplicated. Use it to show a client which address types it can actually
  fetch instead of re-deriving the set from its own config.
- `FetchRegistry` implements `Display`: printing it gives one line per
  registered fetcher, in registration order, as `name (scheme, scheme)`. That
  includes registration precedence, which `schemes()` intentionally drops (the
  first fetcher registered for a scheme still wins). It is meant for humans and
  error messages, not for parsing.

```rust
use ptox::{CommandFetcher, FetchRegistry};
use std::sync::Arc;

let mut registry = FetchRegistry::new();
registry.register(Arc::new(CommandFetcher::new(
    "aria2",
    vec!["magnet".to_owned()],
    "aria2c",
    vec![],
)));
assert_eq!(registry.schemes(), vec!["magnet".to_owned()]);
assert_eq!(registry.to_string(), "aria2 (magnet)");
```

`FetchRegistry::fetch` fails with `FetchError::Unsupported(scheme)` when no
fetcher is registered, so an unknown scheme is a clear error rather than a
zero-byte "success".

### Building the registry from config

`ptox::Config::fetch_registry()` builds a registry from the canonical TOML
schema. **Configured commands are registered before the built-ins**, so an
explicitly configured scheme overrides a built-in handler for the same scheme
(for example, replacing the `ipfs` gateway with a local command). See
[`configuration.md`](configuration.md) for the `[fetch.commands.<scheme>]`
table.

## How a fetch is performed

For `http`/`https` (and the IPFS gateway, which delegates to it), `HttpFetcher`
runs the same sequence:

1. **GET** the URI (for IPFS, the rewritten gateway URL).
2. **Stream** the body to `<destination>.part`, writing chunks as they arrive and
   reporting cumulative progress.
3. **Verify** the computed sha256 against `expected_sha256`, if one was given.
4. **Rename** the `.part` file into place.

Because the rename is atomic and the `.part` file is removed on failure, a
destination path never contains a half-written download. A hash mismatch leaves
no file behind. With `HttpFetcher::with_resume(true)` a partial is instead kept
and continued on the next attempt (see [Resuming interrupted
downloads](#resuming-interrupted-downloads)).

`FetchRegistry::fetch_merged` / `Engine::fetch` derive the expected hash from
the result's `sha256` (falling back to the address's own `sha256`), so a result
that carries a content hash is verified automatically.

## Multi-source failover

A `MergedResult` usually has several `addresses` — the same file seen at more
than one mirror. `FetchRegistry::fetch_merged` tries them in **preference order**
until one succeeds, so a dead mirror does not fail the whole retrieval:

```
https  >  http  >  ipfs  >  magnet  >  other
```

The result's own order is preserved within a scheme. `preferred_addresses`
exposes the full ordering, and `preferred_address` gives just the best one
(used by `suggest_filename` so the suggested name matches the address that is
fetched). The winning address is recorded on `Fetched::address`, which may not
be the preferred one when failover happened.

If every address fails, the error is `FetchError::AllFailed { count, errors }`,
carrying each attempt's error in the order they were tried. An address-less
result fails with `FetchError::NoAddress`.

```rust
use ptox::{merge, Address, FetchRegistry, FileResult, MergeOptions};

# async fn run(registry: &FetchRegistry) -> Result<(), ptox::FetchError> {
let result = merge(
    [FileResult::new(
        "provider",
        vec![
            Address::new("https", "https://dead.example/file.bin"),
            Address::new("http", "http://mirror.example/file.bin"),
        ],
    )],
    &MergeOptions::default(),
)
.into_iter()
.next()
.expect("one merged result");

// Tries the https address first, then the http mirror; returns the first
// success and records the winning address on `fetched.address`.
let fetched = registry.fetch_merged(&result, "file.bin").await?;
println!("{} bytes from {}", fetched.bytes, fetched.address.uri);
# Ok(())
# }
```

## Progress reporting

`fetch_with_progress` reports the **cumulative** number of bytes retrieved so far
to a `ProgressFn` callback (called once per received chunk), so a client can
render a progress bar. The `fetch` variants are equivalent but use a no-op
callback.

```rust
# use ptox::{FetchRegistry, Address};
# async fn run() -> Result<(), ptox::FetchError> {
let registry = FetchRegistry::with_defaults();
let address = Address::new("https", "https://example.org/big.bin");
let mut last = 0u64;
let fetched = registry
    .fetch_with_progress(&address, "big.bin", None, &|bytes| last = bytes)
    .await?;
assert_eq!(last, fetched.bytes);
# Ok(())
# }
```

The callback must be cheap and non-blocking — it runs on the retrieval task.
`Engine::fetch_with_progress` is the `MergedResult` equivalent.

Progress is kept **monotonic** even when work is retried or failed over: the
reported total never moves backwards. If one mirror fails part-way and the next
succeeds, the final report equals the size of the address that won. This is
enforced with a high-water mark, so a failing attempt's partial bytes are
reported once but never re-reported lower.

## Delegating a scheme to a command

Some networks are only reachable through an external client (for example
`aria2c` for `magnet`). [`CommandFetcher`] delegates a scheme to a program:

```rust
use ptox::{CommandFetcher, FetchRegistry};
use std::sync::Arc;

let mut registry = FetchRegistry::new();
registry.register(Arc::new(CommandFetcher::new(
    "magnet",
    vec!["magnet".to_owned()],
    "aria2c",
    vec![
        "--seed-time=0".to_owned(),
        "-d".to_owned(),
        "{dir}".to_owned(),
        "-o".to_owned(),
        "{name}".to_owned(),
        "{uri}".to_owned(),
    ],
)));
```

Each argument may contain these placeholders, substituted per fetch:

| Placeholder | Value |
|---|---|
| `{uri}` | the address URI |
| `{dest}` | the full destination path |
| `{dir}` | the destination's parent directory |
| `{name}` | the destination's file name |

### The `{dest}` contract

**The command MUST create the file at `{dest}`.** This is the whole contract, and
it is easy to get wrong: a command that writes to stdout instead (for example
`ipfs cat`) produces no file at all. In ptox 0.1.0 that looked like a successful
0-byte download; since 0.1.1, `CommandFetcher` checks that the destination exists
after a successful exit and fails with a clear error if it does not. Do not paper
over this with a shell redirect unless you are sure the tool has no native output
flag — a command that exits 0 without producing `{dest}` is an error, not a
download.

After the command exits, `CommandFetcher` measures the destination, hashes it,
and verifies it against `expected_sha256` just like `HttpFetcher`. Because the
command writes the destination in place, ptox cannot observe chunks as they
arrive; instead `fetch_with_progress` polls the destination size every 100 ms
while the command runs and reports each increase, checking once more after exit
so the final report matches the completed size.

## IPFS gateway policy

IPFS content is addressed by CID, not by host, so an `ipfs://` address gives you
no one to dial directly. [`IpfsGatewayFetcher`] bridges this by rewriting
`ipfs://<cid>[/path]` to `<gateway>/ipfs/<cid>[/path]` and delegating to
`HttpFetcher`, inheriting atomic writes, hash verification, and progress.

**The gateway sees every request, so which gateway you use is a privacy and
availability decision, not an implementation detail.** [`IpfsGatewayFetcher::with_gateways`]
takes an ordered list and fails over to the next gateway when one is
unreachable, recording the winning gateway on `Fetched::address`;
`IpfsGatewayFetcher::new` sets a single gateway. `DEFAULT_IPFS_GATEWAY`
(`https://ipfs.io`) is a public convenience for examples and tests. A real client
should pass its own gateway(s) — self-hosted, a trusted provider, or a
configurable setting — rather than the default. The rewrite does not verify that
the gateway returned content matching the CID; integrity comes from the content
hash passed as `expected_sha256`, which `fetch_merged` fills from a result's
`sha256` when present.

Malformed `ipfs://` URIs (wrong scheme, or no CID) fail with
`FetchError::InvalidAddress`.

For other value-addressed networks behind an HTTP gateway, [`GatewayFetcher`] is
the generic form: `GatewayFetcher::new(scheme, prefix, gateways)` rewrites
`<scheme>://<rest>` to `<gateway><prefix><rest>` — so FProxy is
`GatewayFetcher::new("freenet", "/", ["http://127.0.0.1:8888"])` — with the same
ordered-gateway failover.

## Local files (`file://`)

Some providers are local: a catalogue scanner, an on-disk index, or a test mock
emits addresses that already point at a file on this machine. The **`file`
feature** (off by default) adds [`FileFetcher`], which copies such a file to the
destination with the same guarantees as the HTTP fetcher — a `.part` file, a
sha256 computed while streaming, verification against `expected_sha256`, an
atomic rename, and per-chunk progress:

```toml
# Cargo.toml — opt in explicitly.
ptox = { version = "0.1", features = ["file"] }
```

```rust
use ptox::{Address, FetchRegistry, FileFetcher};
use std::sync::Arc;

// Any readable path:
let mut registry = FetchRegistry::new();
registry.register(Arc::new(FileFetcher::new()));

// Or restrict it to one data directory (recommended):
// let mut registry = FetchRegistry::new();
// registry.register(Arc::new(FileFetcher::with_allowed_roots(vec!["/srv/data".into()])));
# let _ = (
#     registry,
#     Address::new("file", "file:///srv/data/report.pdf"),
# );
```

`file://` URIs parse as `file:///abs/path` (empty authority) or
`file://localhost/abs/path`; any other authority is rejected, so a URI cannot
silently mean a remote share. `%XX` escapes are decoded, so
`file:///tmp/a%20b.txt` means `/tmp/a b.txt`. Missing paths and malformed escapes
fail with `FetchError::InvalidAddress`, and a directory (rather than a file)
fails with `FetchError::Io`.

### Local-file policy

**A `file://` address is a read capability for the whole filesystem.** A provider
that emits `file:///etc/passwd` (or any other path) makes the client copy that
file into its download directory, and `fetch_merged` will do it automatically
without asking. This is why the feature is off by default. When you enable it:

- run only **local, trusted providers** (or a scratch/lab machine); treat any
  remote provider as hostile;
- prefer `FileFetcher::with_allowed_roots([...])`, which refuses any source that
  resolves (after symlinks) outside the named directories — pass the client's
  data directory so a provider cannot escape it;
- remember that `FileFetcher::new()` allows any path the process can read, in the
  spirit of `cp`.

A path outside every configured root fails with `FetchError::AccessDenied`.
Local providers are still free to *declare* the `file` scheme — scheme support
is the client's choice, not the provider's.

## Retrying transient HTTP failures

Real mirrors are flaky. `HttpFetcher::with_retry` installs an opt-in
[`RetryPolicy`] that replays **idempotent GETs** after transient failures:

- connection errors and mid-stream read errors;
- `408 Request Timeout`, `429 Too Many Requests`, and any `5xx`.

Client errors (`4xx`) and hash mismatches are **never** retried — the request
will not improve. Retries are **off by default** (exactly one attempt).

```rust
use ptox::{HttpFetcher, RetryPolicy};
use std::time::Duration;

let fetcher = HttpFetcher::new().with_retry(
    RetryPolicy::new(3) // up to 3 total attempts
        .with_base_delay(Duration::from_millis(200))
        .with_max_delay(Duration::from_secs(5)),
);
```

Backoff is `base_delay * 2^(attempt - 1)`, capped at `max_delay`. Each attempt
starts from a clean `.part` file (partials are removed on failure), and progress
stays monotonic across retries. A custom `RetryPolicy` can also be attached to an
`IpfsGatewayFetcher` via `IpfsGatewayFetcher::with_http_fetcher`.

## Resuming interrupted downloads

A large download that dies part-way normally starts over. Opt in with
`HttpFetcher::with_resume(true)` to continue it instead:

```rust
use ptox::HttpFetcher;

let fetcher = HttpFetcher::new().with_resume(true);
```

When a fetch starts, `HttpFetcher` looks for `<destination>.part`. If one exists
and its `.part.src` sidecar records the same address, the saved prefix is
re-hashed and the rest is requested with `Range: bytes=<len>-`:

- a `206 Partial Content` with a matching `Content-Range` is appended to the
  partial;
- a `200 OK` (the server ignored the range) restarts from zero;
- a `416 Range Not Satisfiable` means the partial is already the whole file, so
  it is verified and renamed into place.

A transient failure keeps the partial (instead of deleting it), so the retry
loop, or a later process using the same destination, continues where it left
off. Resume is **off by default** because it trusts the saved prefix. Two things
keep it honest: the `.part.src` sidecar means bytes from two mirrors are never
spliced, and when `expected_sha256` is known the final digest is checked over
the **whole** file, so a corrupt partial is rejected rather than exposed. The
prefix is re-read and hashed, so resuming costs one pass over the bytes already
on disk.

## Bounding HTTP timeouts

`HttpFetcher` uses reqwest's defaults, which include **no timeout**, so a
black-holed connection or a server that stops sending mid-body can hang a fetch
forever. Two opt-in knobs bound that:

```rust
use ptox::HttpFetcher;
use std::time::Duration;

let fetcher = HttpFetcher::new()
    .with_connect_timeout(Duration::from_secs(5))   // TCP/TLS connect phase
    .with_read_timeout(Duration::from_secs(30));   // idle time between reads
```

`with_connect_timeout` bounds only the connect phase. `with_read_timeout` bounds
the wait for more bytes and **resets after every successful read**, so it catches
a stalled transfer without imposing a total deadline on a legitimately large or
slow download. Both are **off by default**; enabling them is a policy choice, and
the `read_timeout` value should be comfortably longer than the server's normal
inter-arrival time. A timed-out attempt fails with `FetchError::Network`, cleans
up any `.part` file, and — like any other transient network error — is retried
when a `RetryPolicy` is configured (see “Retrying transient HTTP failures”
above).

A total deadline (covering the whole body) is intentionally not exposed: it
would cut off large downloads. A caller that needs one can wrap the fetch in
`tokio::time::timeout`.

## The fetch policy (`max_bytes`, `deny_link_local`)

A provider's addresses are untrusted, so the fetcher can be given a policy:

```no_run
use ptox::HttpFetcher;

let fetcher = HttpFetcher::new()
    .with_max_bytes(64 * 1024 * 1024) // abort an oversized download
    .with_deny_link_local(true);      // refuse cloud-metadata hosts
```

`with_max_bytes` caps the bytes written in a single fetch, enforced on the
streamed body (so a lying `Content-Length` cannot get around it) and reported as
`FetchError::TooLarge`; the partial is discarded. `with_deny_link_local` refuses an
`http(s)` address whose host is a literal link-local/unspecified IP or a known
`metadata` name, reported as `FetchError::Denied` — the SSRF path to cloud
metadata. It sees literal hosts only (not what DNS resolves to); loopback and
private LAN hosts are allowed. Both are off by default. In config:
`[fetch] max_bytes = …` and `deny_link_local = true`.

## Proxies and overlay networks (`i2p`, `tor`, …)

Overlay networks do not expose content at a URL you can dial directly; you reach
them through a local proxy (i2pd's HTTP proxy, Tor's SOCKS port). The SDK ships
[`ProxyFetcher`]: it treats the address as `<scheme>://<host>[/path]` (e.g.
`i2p://<b32>.i2p`, `tor://<onion>/file`), rewrites it to `http://<host>[/path]`,
and routes it through the proxy — reusing all the machinery above (atomic writes,
sha256 verification, progress, retries, timeouts) instead of shelling out to
`curl`. Register one per overlay scheme:

```rust
use ptox::{FetchRegistry, ProxyFetcher};
use std::sync::Arc;

let mut registry = FetchRegistry::new();
registry.register(Arc::new(ProxyFetcher::tor()));  // socks5h://127.0.0.1:9050
registry.register(Arc::new(ProxyFetcher::i2p()));  // http://127.0.0.1:4444
```

**Addressing convention:** for an overlay scheme, `Address.uri` is the network
address — `<scheme>://<host>[/path]`, not the transport URL. The fetcher owns the
rewrite, so the same address works whether the transport is an HTTP proxy, a
SOCKS proxy, or a gateway (compare the `ipfs://` gateway fetcher). Register the
overlay fetcher under its scheme and leave the shared `http`/`https` entry
**unproxied**, so clearnet stays direct.

[`ProxyFetcher::new`] takes a scheme and a [`ProxyConfig`]; the transport is an
[`HttpFetcher`] with [`HttpFetcher::with_proxy`], so
[`ProxyFetcher::with_http_fetcher`] can add timeouts, retries, or a custom
`User-Agent`. Supported proxy schemes: `http://` / `https://` (HTTP proxy),
`socks5://` (SOCKS5, local DNS), and `socks5h://` (SOCKS5 with remote DNS —
required for overlay names). An invalid proxy URL fails the fetch with
`FetchError::Network` rather than silently going direct.

## Writing your own fetcher

Implement `Fetcher` for a new protocol and register it. Keep the same
guarantees as the built-ins: write atomically when the protocol allows it, verify
`expected_sha256`, and report what you retrieved.

```rust
use async_trait::async_trait;
use ptox::{FetchError, FetchRequest, Fetched, Fetcher};

struct MyFetcher;

#[async_trait]
impl Fetcher for MyFetcher {
    fn name(&self) -> &str {
        "my-protocol"
    }

    fn schemes(&self) -> &[String] {
        // Return a stable slice; store it on the struct in real code.
        static SCHEMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        SCHEMES.get_or_init(|| vec!["myproto".to_owned()])
    }

    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError> {
        // Resolve request.address.uri, write request.destination, then verify
        // request.expected_sha256 with `ptox::fetch::verify_sha256`.
        let _ = request;
        Err(FetchError::Unsupported("myproto".to_owned()))
    }
}
```

Only override `fetch_with_progress` if your protocol can observe progress; the
default forwards to `fetch` and ignores the callback.

## Errors

| Error | Meaning |
|---|---|
| `Unsupported(scheme)` | no fetcher registered for the scheme |
| `NoAddress` | the result had no addresses to try |
| `AllFailed { count, errors }` | every address failed; per-attempt errors preserved |
| `InvalidAddress(msg)` | malformed address for its scheme (e.g. `ipfs://` with no CID) |
| `AccessDenied(msg)` | a local-file fetch was refused by policy (outside every allowed root) |
| `Network(msg)` | transport/protocol failure |
| `Command(msg)` | a delegated helper command failed or produced no `{dest}` |
| `HashMismatch { expected, actual }` | retrieved bytes did not match the expected sha256 |
| `Io(err)` | filesystem error |

## Testing retrieval offline

The SDK's own fetch tests use local stub servers and no real network — see
`crates/ptox/tests/fetch.rs`. Follow the same pattern for a custom fetcher: put
the transport behind a small trait or a local listener, and assert on file
contents, hash verification, progress monotonicity, and failure cleanup rather
than relying on an external host. `ptox-conformance` validates providers, not
fetchers.

## Checklist

- [ ] Unknown schemes fail loudly (`Unsupported`), never silently.
- [ ] The destination is written atomically (`.part` + rename) where possible.
- [ ] `expected_sha256` is verified before the file is exposed.
- [ ] `fetch_with_progress` reports cumulative bytes and is monotonic.
- [ ] Command schemes create `{dest}` (or fail explicitly).
- [ ] IPFS clients choose a gateway they trust.
- [ ] `file://` is enabled only for trusted/local providers, preferably with
      `FileFetcher::with_allowed_roots`.
- [ ] Transient HTTP failures are retried only when opted in.
- [ ] Long downloads can be resumed (`HttpFetcher::with_resume`) or are cheap to
      restart.
- [ ] A stalled HTTP transfer or connect is bounded by a timeout, not left to
      hang forever.

## See also

- [`docs/configuration.md`](configuration.md) — the `[fetch]` TOML section.
- [`docs/writing-a-provider.md`](writing-a-provider.md) — the search side.
- `crates/ptox/tests/fetch.rs` — the retrieval test suite.
- `crates/ptox/examples/config.rs` — building a registry from config.

[`Fetcher`]: https://docs.rs/ptox/latest/ptox/fetch/trait.Fetcher.html
[`HttpFetcher`]: https://docs.rs/ptox/latest/ptox/fetch/struct.HttpFetcher.html
[`IpfsGatewayFetcher`]: https://docs.rs/ptox/latest/ptox/fetch/struct.IpfsGatewayFetcher.html
[`FileFetcher`]: https://docs.rs/ptox/latest/ptox/fetch/struct.FileFetcher.html
[`CommandFetcher`]: https://docs.rs/ptox/latest/ptox/fetch/struct.CommandFetcher.html
[`RetryPolicy`]: https://docs.rs/ptox/latest/ptox/fetch/struct.RetryPolicy.html
