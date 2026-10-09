# cwm-fetch

The **retrieval half of cwm**: turn a [`cwm-types`](../cwm-types) `Address` into bytes
on disk. The search fabric emits addresses; this fabric consumes them.

Retrieval is pluggable per scheme, exactly like search is pluggable per provider.

## Contract

```rust
#[async_trait]
pub trait Fetcher: Send + Sync {
    fn name(&self) -> &str;
    fn schemes(&self) -> &'static [&'static str];
    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError>;
}
```

`FetchRegistry` dispatches an `Address` to the fetcher registered for its
`scheme`, so `Unknown { scheme }` results fail cleanly with `Unsupported`.

## Built-in fetchers

- **`HttpFetcher`** — `http`/`https`, atomic write (`.part` → rename), sha256 verification.
- **`CommandFetcher`** — delegates a scheme to an external command (e.g. `aria2c`
  for `magnet`); `{uri}` and `{dest}` are substituted into the arguments.

## Example

```rust
let registry = cwm_fetch::FetchRegistry::with_defaults();
let address = cwm_core::Address::new("https", "https://example.org/file.bin");
let fetched = registry.fetch(&address, "file.bin", None).await?;
```

## Status

Early. HTTP + command delegation implemented and tested. IPFS, I2P, Freenet
fetchers are future work (or community plugins).

## Dependency note

`cwm-types` is a **path** dependency inside this workspace.

## License

Dual-licensed under MIT OR Apache-2.0.
