//! Deduplicate and rank results from many providers into one list.
//!
//! A pure post-processor: no network, no state. Feed it the `FileResult`s from
//! [`crate::fanout`] and get a deduplicated, ranked list a client can display
//! and fetch. Ranking weights are caller-supplied policy.

use crate::types::{Address, FileResult, HashAlgos};
use std::collections::HashMap;

/// One file, possibly seen by several providers, with the best address first.
///
/// # Persistence
///
/// Clients commonly serialize a `MergedResult` (with `serde`) and store it
/// between a search and a later fetch. The serialized shape is therefore a
/// **stability contract**:
///
/// - fields are only ever **added** within a major SDK version; a new field is
///   `#[serde(default)]` and omitted from the output when empty, so an old blob
///   still deserializes after an upgrade, and a new blob stays readable by
///   older readers (unknown fields are ignored);
/// - a field is never removed, renamed, or retyped without a new major version
///   and a changelog note.
///
/// In other words, an upgrade within `0.x`/`1.x` must not invalidate persisted
/// client state. Treat the JSON keys as the interface, not the Rust field order.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MergedResult {
    /// Human-readable title, taken from the first source that had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Size in bytes, taken from the first source that had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Media type of the content (e.g. `application/pdf`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mimetype: Option<String>,
    /// Language of the content (BCP 47).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// When the content was published (RFC 3339 UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    /// When a provider last saw it available (RFC 3339 UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    /// Union of the sources' content hashes, keyed by algorithm name.
    #[serde(default, skip_serializing_if = "HashAlgos::is_empty")]
    pub hashes: HashAlgos,
    /// Every provider that returned this file (sorted, deduplicated).
    #[serde(default)]
    pub providers: Vec<String>,
    /// Every way to obtain it (deduplicated by URI).
    #[serde(default)]
    pub addresses: Vec<Address>,
    /// Heuristic rank. Higher is better; only comparable within one `merge`.
    #[serde(default)]
    pub score: f64,
}

/// Ranking policy for [`merge`], supplied by the caller.
#[derive(Debug, Clone, Default)]
pub struct MergeOptions {
    /// Per-provider ranking weight (default `1.0`). Client-supplied policy.
    pub provider_weights: HashMap<String, f64>,
}

/// Deduplicate and rank results. See [`MergeOptions`].
///
/// Deduplication keys, in order of preference: a content hash (`sha256`,
/// `sha1`, `infohash`, `md5`, `cid`), otherwise the normalized title plus size.
/// The default score rewards multi-source agreement and metadata completeness.
pub fn merge<I>(results: I, options: &MergeOptions) -> Vec<MergedResult>
where
    I: IntoIterator<Item = FileResult>,
{
    let mut clusters: Vec<MergedResult> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for result in results {
        let keys = dedup_keys(&result);
        let position = match keys.iter().find_map(|key| index.get(key).copied()) {
            Some(position) => position,
            None => {
                clusters.push(MergedResult::default());
                clusters.len() - 1
            }
        };
        absorb(&mut clusters[position], result);
        for key in keys {
            index.insert(key, position);
        }
    }

    for cluster in &mut clusters {
        cluster.providers.sort();
        cluster.providers.dedup();
        cluster.score = score(cluster, &options.provider_weights);
    }
    clusters.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.title.cmp(&b.title))
    });
    clusters
}

/// Suggest a filename for a retrieved [`MergedResult`].
///
/// A client that has no destination of its own can use this to name the file it
/// is about to fetch. It is deterministic, offline, and performs no I/O. The
/// name is chosen in order:
///
/// 1. the result's [`title`](MergedResult::title);
/// 2. otherwise the last path segment of its preferred address's URI
///    (`https://host/dir/file.pdf` → `file.pdf`; authorities, queries, and
///    fragments are not treated as names);
/// 3. otherwise `"download.bin"`.
///
/// Characters that are illegal in a filename on common platforms (`/`, `\`,
/// NUL, `:`, `*`, `?`, `"`, `<`, `>`, `|`, and control characters) are replaced
/// with `_`, and trailing dots are dropped. If the chosen name has no extension
/// and the result declares a known [`mimetype`](MergedResult::mimetype), that
/// extension is appended (`application/pdf` → `.pdf`).
///
/// The result is a bare file name, never a path: it never contains a directory
/// separator, so it is safe to join onto a destination directory. Callers are
/// still responsible for collision handling.
pub fn suggest_filename(result: &MergedResult) -> String {
    let name = result
        .title
        .as_deref()
        .map(sanitize_filename)
        .filter(|name| !name.is_empty())
        .or_else(|| {
            preferred_address(result)
                .and_then(|address| uri_basename(&address.uri))
                .map(|name| sanitize_filename(&name))
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "download.bin".to_owned());
    with_mimetype_extension(name, result.mimetype.as_deref())
}

/// The preferred address of a result: `https` > `http` > `ipfs` > `magnet` >
/// other, then registration order.
///
/// `merge` cannot depend on the optional `fetch` feature, so this is the shared
/// implementation; `fetch::preferred_address` is a public wrapper over it.
pub(crate) fn preferred_address(result: &MergedResult) -> Option<&Address> {
    result
        .addresses
        .iter()
        .min_by_key(|address| match address.scheme.as_str() {
            "https" => 0,
            "http" => 1,
            "ipfs" => 2,
            "magnet" => 3,
            _ => 4,
        })
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| match character {
            '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect();
    cleaned.trim().trim_end_matches('.').to_owned()
}

/// The last path segment of `uri`, ignoring query/fragment and the authority.
fn uri_basename(uri: &str) -> Option<String> {
    let without_query = uri.split(['?', '#']).next().unwrap_or(uri);
    let after_scheme = without_query
        .split_once("://")
        .map_or(without_query, |(_, rest)| rest);
    let (_authority, path) = after_scheme.split_once('/')?;
    let segment = path.trim_end_matches('/').rsplit('/').next()?;
    (!segment.is_empty()).then(|| segment.to_owned())
}

fn with_mimetype_extension(name: String, mimetype: Option<&str>) -> String {
    if std::path::Path::new(&name).extension().is_some() {
        return name;
    }
    match mimetype.and_then(extension_for_mimetype) {
        Some(extension) => format!("{name}.{extension}"),
        None => name,
    }
}

fn extension_for_mimetype(mimetype: &str) -> Option<&'static str> {
    let mimetype = mimetype
        .split(';')
        .next()
        .unwrap_or(mimetype)
        .trim()
        .to_ascii_lowercase();
    match mimetype.as_str() {
        "application/pdf" => Some("pdf"),
        "application/epub+zip" => Some("epub"),
        "application/zip" => Some("zip"),
        "application/gzip" | "application/x-gzip" => Some("gz"),
        "application/x-tar" => Some("tar"),
        "application/json" => Some("json"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/svg+xml" => Some("svg"),
        "audio/mpeg" => Some("mp3"),
        "audio/ogg" => Some("ogg"),
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "text/plain" => Some("txt"),
        _ => None,
    }
}

fn dedup_keys(result: &FileResult) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(hashes) = &result.hashes {
        for algorithm in ["sha256", "sha1", "infohash", "md5", "cid"] {
            if let Some(value) = hashes.get(algorithm) {
                keys.push(format!("{algorithm}:{value}"));
            }
        }
    }
    if keys.is_empty() {
        if let (Some(title), Some(size)) = (&result.title, result.size) {
            keys.push(format!("name:{}:{size}", normalize(title)));
        }
    }
    keys
}

fn normalize(title: &str) -> String {
    title
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn absorb(cluster: &mut MergedResult, result: FileResult) {
    if cluster.title.is_none() {
        cluster.title = result.title;
    }
    if cluster.size.is_none() {
        cluster.size = result.size;
    }
    if cluster.mimetype.is_none() {
        cluster.mimetype = result.mimetype;
    }
    if cluster.lang.is_none() {
        cluster.lang = result.lang;
    }
    if cluster.published.is_none() {
        cluster.published = result.published;
    }
    if cluster.last_seen.is_none() {
        cluster.last_seen = result.last_seen;
    }
    if let Some(hashes) = result.hashes {
        for (algorithm, value) in hashes {
            cluster.hashes.entry(algorithm).or_insert(value);
        }
    }
    if !cluster.providers.contains(&result.provider) {
        cluster.providers.push(result.provider);
    }
    for address in result.addresses {
        if !cluster
            .addresses
            .iter()
            .any(|existing| existing.uri == address.uri)
        {
            cluster.addresses.push(address);
        }
    }
}

fn score(cluster: &MergedResult, weights: &HashMap<String, f64>) -> f64 {
    let mut score: f64 = cluster
        .providers
        .iter()
        .map(|provider| weights.get(provider).copied().unwrap_or(1.0))
        .sum();
    if !cluster.hashes.is_empty() {
        score += 0.5;
    }
    if cluster.mimetype.is_some() {
        score += 0.25;
    }
    if cluster.size.is_some() {
        score += 0.25;
    }
    if cluster.published.is_some() {
        score += 0.1;
    }
    score
}
