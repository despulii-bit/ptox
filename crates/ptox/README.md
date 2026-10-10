# ptox

A language-neutral **SDK for building file-search clients across many networks**
(clearnet, I2P, Freenet/Hyphanet, IPFS, BitTorrent, …).

It provides the contracts, engines, and tooling a client builds on:

- **Contracts** — `FileQuery`, `FileResult`, `Address`, `SearchProvider`,
  `Fetcher`, `Capabilities`.
- **Engines** — `Engine` (search + retrieve), `fanout`, `merge` (deduplicate +
  rank), `FetchRegistry`.
- **Bridge** — the NDJSON wire format and `SubprocessProvider`, so providers can
  be written in any language.

```rust
# async fn run(providers: Vec<std::sync::Arc<dyn ptox::SearchProvider>>) {
let engine = ptox::Engine::new(providers);
let report = engine.search(ptox::FileQuery::new("annual report")).await;
for result in &report.results {
    println!("{:?} from {:?}", result.title, result.providers);
}
# }
```

Retrieval (the `fetch` feature, on by default) turns a result's `Address` into
bytes: `Engine::fetch(&result, "out.bin").await`. The `ipfs` feature (also on by
default) adds an `ipfs://` gateway fetcher, `IpfsGatewayFetcher`, which resolves
CIDs through a configurable HTTP gateway and delegates to the HTTP fetcher.
`HttpFetcher::with_retry` can replay transient GET failures (`5xx`, `408`, `429`,
connection/stream errors) with exponential backoff; retries are off by default.

Providers (network plugins) and clients (frontends) are separate software. The
contract, JSON Schema, and conformance suite live in the repository:
<https://github.com/despulii-bit/ptox>.

`MergedResult` is a persistence contract: its serialized shape only gains fields
within a major version, so a client can store search results between a search and
a later fetch and still read them after an SDK upgrade. For on-disk client state,
prefer the smaller versioned `ResultRef` (`ResultRef::from_merged` / `to_merged`),
which keeps only the fetch-essential fields and carries its own schema version.
`suggest_filename(&result)`
suggests a safe name for the file (the title, else the preferred address, with
illegal characters replaced).

Client wiring has a canonical, versioned TOML schema in `ptox::config` (feature
`config`, on by default): `Config::load` / `from_toml_str` parse and validate it,
and `Config::providers()` / `fetch_registry()` / `merge_options()` build the
pieces a client needs. See `docs/configuration.md` in the repository.

## Writing a provider

Implement the `SearchProvider` trait and pass it to `fanout` or `Engine`. The
repository ships a complete, runnable example — trait impl, `Capabilities`, error
codes, and an offline test with a fake transport:

```sh
cargo run -p ptox --example rust_provider
```

A provider in any other language can speak the NDJSON wire format and be wrapped
with `SubprocessProvider`. Validate either kind with the `ptox-conformance`
suite. See `docs/writing-a-provider.md` in the repository for the end-to-end
guide.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
