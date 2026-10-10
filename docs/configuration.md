# Configuring a ptox client

A ptox **client** (CLI, GUI, agent) assembles providers and fetchers. This page
defines the canonical, versioned TOML shape for that wiring, so clients,
examples, and tooling can agree on one format instead of each inventing their
own.

The schema lives in the SDK behind the opt-in `config` feature:

```rust
use ptox::Config;

let config = Config::load("/home/me/.config/ptox/providers.toml")?;
```

A runnable walkthrough is
[`crates/ptox/examples/config.rs`](../crates/ptox/examples/config.rs); run it
with `cargo run -p ptox --example config`.

## Example

```toml
# Optional. Defaults to the current schema version; a newer explicit version
# is rejected rather than misread.
version = 1

# One entry per external provider command (an NDJSON adapter). These are
# separate programs — the SDK never bundles a network.
[[provider]]
name = "archive-org"
program = "node"
args = ["/opt/ptox/adapters/archive_org.js"]
env = { ARCHIVE_ORG_TIMEOUT = "10" }

[[provider]]
name = "local"
program = "node"
args = ["/opt/ptox/adapters/local.js"]

# Per-provider ranking weight, consumed by `ptox::merge`. Unlisted providers
# default to 1.0.
[ranking]
weights = { "archive-org" = 1.2 }

# Delegate a whole scheme to an external command. The command MUST create
# `{dest}`; the other placeholders are `{uri}`, `{dir}`, and `{name}`.
[fetch.commands.magnet]
program = "aria2c"
args = ["--seed-time=0", "-d", "{dir}", "-o", "{name}", "{uri}"]
```

## Schema reference

### `version` (optional, integer)

The schema version. Omitted means the current version ([`CONFIG_VERSION`], `1`).
A value newer than the build understands fails validation instead of being
silently misread. Fields are only added within a major schema version, new
fields carry defaults, and unknown fields are ignored — so an older build can
still read a newer file.

### `[[provider]]` (array of tables)

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | Stable provider name. Used for attribution, capability routing, and ranking weights. Must be unique and non-empty. |
| `program` | yes | The executable to run. Must be non-empty. |
| `args` | no | Command-line arguments. Paths are not rewritten; use absolute paths or paths relative to the working directory. |
| `env` | no | Extra environment variables for the child process (proxies, routers, data dirs, credentials). |

Each entry becomes a [`SubprocessProvider`] via `Config::providers()`. Provider
configuration that is not a simple argument or env var (e.g. a config file the
adapter reads) belongs to the adapter, not this schema.

### `[ranking]`

| Field | Meaning |
|---|---|
| `weights` | Map of provider name to a finite, non-negative multiplier. |

`Config::merge_options()` returns these as `MergeOptions` for `ptox::merge` /
`Engine::with_weights`.

### `[fetch.commands.<scheme>]` (table of tables)

Each key is an address scheme (e.g. `magnet`, `ipfs`) delegated to an external
client.

| Field | Required | Meaning |
|---|---|---|
| `program` | yes | The executable to run. Must be non-empty. |
| `args` | no | Arguments. `{uri}`, `{dest}`, `{dir}`, and `{name}` are substituted per fetch. |

The command **must create `{dest}`**. A command that writes elsewhere (for
example `ipfs cat` writes to stdout) will otherwise look like a successful
0-byte download or fail; see [`CommandFetcher`] for the contract.

`Config::fetch_registry()` builds a registry from the config: built-in
`http`/`https` (and the `ipfs://` gateway when the `ipfs` feature is on, and the
local `file://` fetcher when the off-by-default `file` feature is on) plus one
`CommandFetcher` per entry. Configured commands are registered **first**, so an
explicitly configured scheme wins over a built-in handler for the same scheme.

## Building an engine

The canonical `load → search` path is one call. `Config::engine()` wires the
configured providers, ranking weights, and fetch registry into a ready-to-run
[`Engine`]:

```rust
let config = Config::load("/home/me/.config/ptox/providers.toml")?;
let engine = config.engine();
let report = engine.search(ptox::FileQuery::new("annual report")).await;
```

Use `Config::engine_with_options(Options)` to set client-side search policy
(provider timeout, global limit/offset, concurrency) that the config
deliberately does not carry. When a client needs the individual pieces — for
example to add an in-process provider that is not a subprocess, or to register
extra fetchers — build them from `providers()`, `merge_options()`, and
`fetch_registry()` and assemble the `Engine` directly.

## Validation

`Config::from_toml_str` and `Config::load` parse **and** validate. `validate`
checks what TOML cannot express:

- the schema `version` is supported;
- provider names are unique and non-empty;
- provider and fetch-command programs are non-empty;
- fetch-command schemes are non-empty;
- ranking weights are finite and non-negative.

Unknown fields are allowed (forward compatibility). Construct a config in code
and call `Config::validate()` before use if you do not load it from TOML.

## Stability and features

`Config`'s serialized shape is a stability contract, like `MergedResult`: fields
are only added within a schema version, with `serde(default)`, and are omitted
from output when empty. Use `Config::to_toml_string()` to write a config back out
(for example a `config init` command); it round-trips through
`Config::from_toml_str`.

Enable the `config` feature to use this module (opt-in; it implies `fetch`). The
default build omits the TOML loader and its `toml` dependency.

[`CommandFetcher`]: https://docs.rs/ptox/latest/ptox/fetch/struct.CommandFetcher.html
[`CONFIG_VERSION`]: https://docs.rs/ptox/latest/ptox/config/constant.CONFIG_VERSION.html
[`Engine`]: https://docs.rs/ptox/latest/ptox/struct.Engine.html
[`SubprocessProvider`]: https://docs.rs/ptox/latest/ptox/provider/struct.SubprocessProvider.html
