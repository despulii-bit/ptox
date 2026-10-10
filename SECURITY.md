# Security policy

ptox is a **library** that processes untrusted input: provider output on the
NDJSON wire, and bytes retrieved from arbitrary addresses. It does **not** run a
network service, hold credentials, or execute retrieved content. The properties
that matter are therefore about *containing hostile input*, not protecting a
server.

## In scope

Reports are welcome for anything that breaks a ptox security invariant:

- **Bounded parsing.** A provider cannot make the host allocate without bound:
  wire lines are read with a size limit (`ptox::MAX_LINE_BYTES`) and an over-long
  line becomes a `malformed` failure, never an unbounded buffer. A panic, integer
  overflow, or unbounded allocation reachable from provider output or a
  `FileQuery` is a bug.
- **Result validation.** A structurally invalid result is turned into a
  `malformed` failure and never reaches a client's merge/fetch path.
- **No implicit actions.** Searching never fetches an `Address`, and the core
  writes no query content to disk or logs.
- **Retrieval integrity.** A supplied `sha256` is verified before a file is
  renamed into place; a mismatch removes the partial file. An atomic-write bypass
  or a verification bypass is a bug.
- **Local-file containment.** The `file://` fetcher is off by default and
  `FileFetcher::with_allowed_roots` must refuse any source that resolves (after
  symlinks) outside the configured roots.
- **No `unsafe`.** The crates forbid `unsafe` code.

## Out of scope

- **Providers and networks themselves.** Providers are separate software that
  adapt one network. A bug in a specific provider, or the legality/availability
  of the content it indexes, is that provider's concern — not this SDK's.
- **Misconfiguration.** Running a provider you don't trust, enabling `file://`
  without `allowed_roots`, pointing a gateway or fetch command somewhere unsafe,
  or delegating a scheme to a command with untrusted arguments.
- **Clearnet/overlay services.** Outages, blocks, or takedowns of a network or
  gateway.
- **Dependencies.** Report upstream issues upstream; we track updates via
  Dependabot.

## Reporting

Report vulnerabilities **privately** through GitHub's private vulnerability
reporting (the **Security → Report a vulnerability** button on this repository).
If that is unavailable, open an issue describing the *impact* without a working
exploit and ask for a private channel.

Please include the affected crate and version, a minimal reproduction, and the
invariant you believe is broken. This is a pre-release, single-maintainer
project, so response is best-effort; we aim to acknowledge within a few days.

## Supported versions

Only the latest published `0.x` release is supported. There is no long-term
support branch while the SDK is pre-1.0.
