# Contributing to ptox

Thanks for helping build a composable, multi-network file-search core.

## What belongs where

This repo is **only the core and reference providers**. The design deliberately
pushes everything else into separate software:

- **Welcome here:** core changes, new **providers** (one per network or index),
  contract/spec improvements, docs, and tests.
- **Belongs elsewhere:** frontends (CLI/TUI/GUI), the retrieval/fetch fabric,
  merge/dedup/ranking, index/crawler services, and plugin discovery. These
  *consume* the contract; they do not live in this repo.

If you're unsure whether something fits, open an issue first.

## Development setup

Rust is available through Nix (`shell.nix`):

```sh
nix-shell --run 'cargo test --workspace'
```

Any stable Rust toolchain works too — the crates are plain Cargo, so `rustup`
users need no Nix. The minimum supported version is the `rust-version` in
`Cargo.toml` (currently 1.88, set by the dependency floor of the default
`fetch` feature; CI verifies it).

Useful commands:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Layout

- `crates/ptox/spec/core-contract.md` — the locked, language-neutral contract.
- `crates/ptox/spec/conformance.md` — the behavior every implementation must satisfy.
- `crates/ptox/spec/vectors.json` — language-neutral data-validation vectors.
- `crates/ptox` — the SDK: `types`, `search`, `provider`, `fetch`, and `merge` modules.
- `crates/ptox-conformance` — provider conformance suite.
- Providers live in **separate repos** — never in the core.

## Writing a provider

A provider is **one network or index**. It turns a `FileQuery` into a stream of
`FileResult` / `Failure` and does nothing else.

The end-to-end walkthrough (capabilities, error codes, offline testing,
conformance) is [docs/writing-a-provider.md](docs/writing-a-provider.md).

### In Rust

Implement `SearchProvider`:

```rust
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;      // what you can serve
    fn search(&self, query: &FileQuery) -> ResultStream<'_>;
}
```

All protocol, proxy, auth, and transport detail lives inside the provider and is
injected at construction, never per query. See
`crates/ptox/examples/rust_provider.rs` for a complete, offline-testable example.

### In any language

Speak the NDJSON wire format (`crates/ptox/spec/core-contract.md` §9) and wrap the
program with `SubprocessProvider`. See
`crates/ptox/examples/ndjson_provider.rs` for a minimal example.

### Provider checklist

- [ ] Yields `FileResult`s with a non-empty `provider` and at least one `Address`.
- [ ] Handles unsupported queries honestly (`unsupported`, not a misleading empty result).
- [ ] Maps failures to the right `ErrorCode` (`timeout`, `transport`, `malformed`, …).
- [ ] Is offline-testable — abstract I/O behind a trait and test with a fake.
- [ ] Preserves the `raw` payload.
- [ ] Contains no secrets and performs no network access in tests.

Validate any provider (any language) with the conformance suite:

```sh
cargo run -p ptox-conformance -- -- <provider-command> [args...]
```

It checks spawning, wire-line validity, result shape, error shape, capability
negotiation, and termination, and exits nonzero on failure.

## Tests and CI

Every pull request must pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Add tests with your change and prefer **offline** tests (mock the transport or
resolver). The existing conformance suite encodes the contract — keep it green.

## Scope rules (important)

- The core has **no state, no policy, no discovery, and no network I/O**. Don't add them.
- Don't merge, dedup, or rank in the core; that is a separate layer.
- **Never auto-fetch** an `Address`.
- Unknown fields must be tolerated at every level; don't reject wire messages for
  extra keys.
- Changing the contract requires a version bump and a spec update in the same PR.

## Repository hygiene

Agent / development scaffolding is intentionally **not** in this repo; it lives in
a separate private repo. Don't commit `AGENTS.md`, `.pi/`, `.agents/`,
`.handoff/`, or `scratch/` — they are gitignored.

## Pull requests

- Keep PRs small and focused; explain the "why".
- One logical change per PR.
- Update the spec and `vectors.json` if behavior changes.
- Don't force-push to `main`.

## License

By contributing, you agree that your contributions are dual-licensed under
MIT OR Apache-2.0, as described in `LICENSE-MIT` and `LICENSE-APACHE`.
