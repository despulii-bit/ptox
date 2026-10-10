# ptox Conformance Suite

Any implementation of the core contract (`core-contract.md`) must pass these
tests. Data-validation vectors are in `vectors.json`. Behavior tests are
described here so they can be implemented in any language. Provider-facing
checks are implemented by `ptox-conformance` (`crates/ptox-conformance`).

## A. Data validation

Using `vectors.json`:

- `valid.*` — MUST be accepted and validated. Known fields MUST round-trip
  without loss; unknown fields are ignored (not an error).
- `invalid.*` — MUST be rejected with a clear validation error.
- Unknown fields at any level MUST be ignored, not rejected.

## B. Provider isolation

1. Given 3 providers where one throws on `search`, the other two MUST still
   return all their results.
2. The failing provider MUST appear as a single `Failure`, not abort the stream.

## C. Timeout

1. Given `timeout_ms`, a provider that blocks longer MUST yield
   `Failure{code: "timeout"}`.
2. Other providers MUST be unaffected.
3. The whole fanout MUST complete within roughly `timeout_ms` of the slowest
   provider (no hangs).

## D. Streaming

1. A fast provider's results MUST be observable **before** a slow provider
   finishes (do not buffer until all complete).
2. With a slow provider and no timeout, fast results still arrive immediately.

## E. Cancellation

1. Closing/aborting the result stream MUST cause providers to stop work
   (observable via teardown side effects or `Failure{code:"cancelled"}`).
2. No further items are emitted after cancellation.

## F. Attribution and shape

1. Every `FileResult` has a non-empty `provider` and at least one `Address`.
2. Every `Address` has a non-empty `scheme` and `uri`.
3. When a provider declares `Capabilities::schemes`, every emitted `Address`
   scheme MUST be among them. A provider that declares none is universal and
   may emit any scheme.

## G. Limits and hostile output

1. `offset` skips (best-effort) and `limit` bounds results per provider, and a
   global `offset`/`limit` windows the streamed sequence; a provider emitting
   unbounded results must not exhaust memory.
2. A provider returning a very large `raw` payload MUST be bounded/truncated or
   rejected, never allowed to OOM the host.
3. An output line larger than the host's limit MUST be rejected as `malformed`,
   never buffered without bound.

## H. Security invariants

1. **No network I/O:** run the core with a provider that performs no I/O; assert
   the core opens no sockets and resolves no DNS.
2. **No auto-fetch:** assert the core never fetches anything referenced by an
   `Address`.
3. **No persistence:** assert no query content is written to disk or logs.

## I. Interop

1. An out-of-process provider speaking the NDJSON wire format
   (`core-contract.md` §9) MUST be usable exactly like an in-process provider.
2. Malformed wire input MUST yield `Failure{code:"malformed"}` for that provider
   only.
3. A provider that exits non-zero **without** emitting `done` MUST be reported as
   `Failure{code:"transport"}`, never as an empty (successful) result set.
4. Capability negotiation is optional: a provider that declines a `capabilities`
   request is treated as **universal**. A provider that answers MUST return a
   well-formed capability object, and the suite MUST NOT fail one that declines.

## J. Ordering

1. No test may assume result ordering; the suite MUST pass under any delivery
   order.
