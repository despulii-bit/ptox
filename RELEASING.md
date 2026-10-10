# Releasing ptox

The workspace publishes two crates. **Order matters**: `ptox-conformance` depends
on `ptox` by version, so `ptox` must be resolvable on crates.io first.

## 0. Before publishing

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo publish --dry-run -p ptox      # also run by CI
```

- Update `CHANGELOG.md`.
- Bump `version` in the root `Cargo.toml` (`[workspace.package]`).
- Make sure the working tree is clean and the commit is on `main`.

## 1. Publish the SDK

```sh
cargo login                          # token from https://crates.io/settings/tokens
cargo publish -p ptox
```

## 2. Tag the release

Tag the exact commit that was published:

```sh
git tag -a v0.1.0 -m "ptox v0.1.0"
git push origin v0.1.0
```

## 3. Publish the conformance suite (optional, after step 1)

Once `ptox` 0.1 resolves from crates.io:

```sh
cargo publish -p ptox-conformance
```

Until then, `cargo package -p ptox-conformance` fails with
`no matching package named ptox found`, which is expected.

## Publishing is forever

A published version is immutable. If a release is wrong, **yank** it and publish
a new patch — never try to replace it.

## Repository visibility

`Cargo.toml`'s `repository` field and the README badge point at
`github.com/despulii-bit/ptox`. While that repo is private, crates.io/docs.rs
source links and the badge will 404 for the public. Make the repo public before
the first release, or accept an opaque crate.
