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

Providers (network plugins) and clients (frontends) are separate software. The
contract, JSON Schema, and conformance suite live in the repository:
<https://github.com/despulii-bit/ptox>.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
