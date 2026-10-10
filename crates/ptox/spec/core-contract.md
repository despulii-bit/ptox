# ptox Core Contract — v2 (locked)

Language-neutral and authoritative. Supersedes earlier internal planning drafts.

The core has **no state, no policy, no discovery, and no network I/O**. It is a
pure fan-out: *given a query and a set of injected providers, stream attributed
results.*

---

## 1. Type language

- `T?` — optional field (absent or null)
- `[T]` — list
- `{K: V}` — map
- `any` — opaque JSON value
- Timestamps, if used, are RFC 3339 UTC strings.

---

## 2. `FileQuery`

| Field | Type | Required | Notes |
|---|---|---|---|
| `text` | string | **yes** | Free-text keywords; may be `""` when another selector is present. |
| `hashes` | `{algo: value}` | no | Content-addressed lookup (e.g. `infohash`, `sha256`, `cid`). |
| `mimetypes` | `[string]` | no | Accepted media types: full (`application/pdf`), top-level (`image`), or wildcarded (`image/*`). |
| `extensions` | `[string]` | no | Accepted extensions, lowercase, no leading dot (`pdf`, `jpg`). |
| `filename` | string | no | Filename or glob pattern (best-effort). |
| `size_min` | int | no | Bytes, inclusive. |
| `size_max` | int | no | Bytes, inclusive. |
| `published_after` | string | no | RFC 3339 UTC; publish-time lower bound. |
| `published_before` | string | no | RFC 3339 UTC; publish-time upper bound. |
| `lang` | string | no | Preferred language (BCP 47). |
| `limit` | int | no | Max results **per provider** (best-effort). |
| `offset` | int | no | Results to skip **per provider** before `limit` (best-effort pagination). |
| `extras` | `{string: any}` | no | Provider-specific passthrough; keys SHOULD be namespaced. |

Providers MUST NOT mutate the query.

**Filter semantics.** Filters are OR within a field and AND across fields. They are
best-effort: a provider that cannot honor a filter MUST still return text matches
rather than an empty result.

**Paging.** `offset` and `limit` are per-provider pagination controls, not
filters: a provider skips its first `offset` own matches, then returns at most
`limit`. Both are best-effort like filters, and neither is a capability facet
(§6). The same query with a larger `offset` requests the next page from each
provider. Because results are neither ordered nor deterministic (§8.5, §8.12), a
page is meaningful within one provider/query; a client that needs a stable page
across searches must snapshot the first result set and page over that snapshot.

---

## 3. `Address`

A way to obtain the content. Struct, not a bare string — it is the seam the whole
ecosystem depends on.

| Field | Type | Required | Notes |
|---|---|---|---|
| `scheme` | string | **yes** | Dispatch key, non-empty: `http`, `https`, `magnet`, `ipfs`, `ipns`, `i2p`, `freenet`, `ftp`, … Open set. |
| `uri` | string | **yes** | Non-empty. |
| `hashes` | `{algo: value}` | no | Digest for **this** address, for verification. |
| `size` | int | no | Bytes. |
| `extras` | `{string: any}` | no | Forward-compat; ignored if unknown. |

---

## 4. `FileResult`

| Field | Type | Required | Notes |
|---|---|---|---|
| `provider` | string | **yes\*** | Attribution. Providers MAY omit it on the wire; the host MUST set it (§9). Validation requires it non-empty. |
| `title` | string | no | Display name / filename. |
| `addresses` | `[Address]` | **yes** | Non-empty. Ways to obtain it. |
| `size` | int | no | Bytes. |
| `hashes` | `{algo: value}` | no | Content digests for the result as a whole. |
| `mimetype` | string | no | Media type. |
| `lang` | string | no | BCP 47 language. |
| `published` | string | no | RFC 3339 UTC. |
| `last_seen` | string | no | RFC 3339 UTC. |
| `score` | number | no | Provider-local relevance; NOT comparable across providers. |
| `raw` | `any` | no | Untouched original payload. Never interpreted by the core. |

On the wire a provider may omit `provider`; the host fills it in with the
provider's configured name. Once filled, it must be non-empty.

---

## 5. `Failure`

Per-provider error. Stream items are `FileResult | Failure`.

| Field | Type | Required |
|---|---|---|
| `provider` | string | **yes** |
| `error` | `{code: string, message: string}` | **yes** |

Well-known `code` values (open set; unknown codes MUST be tolerated):
`timeout`, `unsupported`, `refused`, `transport`, `malformed`, `cancelled`,
`internal`.

---

## 6. `SearchProvider`

```
name             : string
capabilities()   : Capabilities   # optional; default = universal
search(query)    : Stream[FileResult]
```

- One provider per network (or per index). Community-supplied.
- All network/protocol/auth/transport detail lives inside the provider.
- A provider signals failure by raising/erroring; the engine converts it to a
  `Failure`.
- Transport (VPN/Tor/I2P/proxy) is injected at provider **construction**, not
  per-query.

### `Capabilities`

```
query    : [string]   # supported query facets (empty = any)
requires : [string]   # facets that MUST be present
schemes  : [string]   # emitted address schemes (empty = any)
```

Facets: `text`, `hashes`, `mimetypes`, `extensions`, `filename`, `size`,
`published`, `lang`. The default (empty) is **universal**. The engine skips a
provider whose capabilities reject the query instead of spawning it.

---

## 7. `fanout()`

```
fanout(query: FileQuery, providers: [SearchProvider], options?: Options)
    -> Stream[FileResult | Failure]
```

`Options` (all optional; policy, not core semantics):
`timeout_ms`, `limit` (global cap), `signal` (cancellation).

---

## 8. Required semantics

1. **No network I/O in the core.** It never fetches addresses, previews, or
   hashes. No DNS, no sockets.
2. **Never auto-fetch.** Fetching a result is explicit and external.
3. **Provider isolation.** One provider failing/timing out/hanging MUST NOT
   prevent or corrupt results from others.
4. **Streaming.** Results MUST be emitted as providers produce them, not held
   until all finish.
5. **No ordering guarantee.** Delivery order is unspecified.
6. **Attribution.** Every `FileResult` carries a non-empty `provider`.
7. **Cancellation.** Closing the stream MUST propagate teardown to providers.
8. **Limits + backpressure.** Result counts and payload sizes MUST be bounded; a
   hostile provider must not exhaust memory.
9. **No persistence, logs, or telemetry** of query content.
10. **No identity.** `FileQuery` carries no user/session identifier.
11. **Unknown fields tolerated** at every level.
12. **Determinism not promised.** The same query may return different results.
13. **Result validation at the boundary.** After attribution, a result that
    violates this contract (empty `provider`/address scheme/URI, or no
    `addresses`) MUST be rejected by the fan-out as a `Failure{code:"malformed"}`
    attributed to its provider; it MUST NOT reach the client's merge/fetch path.
    Rejecting one result MUST NOT discard the provider's other results.

---

## 9. Out-of-process wire format

Providers may run as subprocesses, speaking **NDJSON** over stdin/stdout, so
adapters can be written in any language. The host bounds each line (default
8 MiB); a longer line is treated as `malformed`, never buffered without bound.

Request:
```
{"v": 2, "id": 1, "method": "search", "params": {"query": <FileQuery>}}
```

Result line (one per result):
```
{"id": 1, "result": <FileResult>}
```
The host MUST set `provider` on each result to the provider's configured name.

Provider failure:
```
{"id": 1, "error": {"code": "timeout", "message": "…"}}
```

End of stream:
```
{"id": 1, "done": true}
```

Cancellation (host → provider):
```
{"id": 1, "cancel": true}
```

Capabilities request (optional; providers may ignore it):
```
{"v": 2, "id": 1, "method": "capabilities", "params": {}}
```

Capabilities response:
```
{"id": 1, "result": {"query": ["hashes"], "requires": ["hashes"], "schemes": ["magnet"]}}
```
A provider that does not answer is treated as universal.

---

## 10. Versioning

This is contract **v2**. Every request carries the wire protocol version in `v`;
a provider that cannot speak it SHOULD fail with `unsupported`. Changes are
additive where possible; breaking changes bump the version. Consumers MUST ignore
unknown fields. The machine-readable schema ships with the SDK at
`spec/schema/wire.schema.json`.

- **v2** — `mimetype` → `mimetypes` (list); added `extensions`, `filename`,
  `published_after`/`published_before`, `lang`, and `offset` to `FileQuery`;
  added `lang`, `published`, `last_seen`, `score` to `FileResult`.
