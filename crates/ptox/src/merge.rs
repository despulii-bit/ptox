//! Deduplicate and rank results from many providers into one list.
//!
//! A pure post-processor: no network, no state. Feed it the `FileResult`s from
//! [`crate::fanout`] and get a deduplicated, ranked list a client can display
//! and fetch. Ranking weights are caller-supplied policy.

use crate::types::{Address, FileResult, HashAlgos};
use std::collections::HashMap;

/// One file, possibly seen by several providers, with the best address first.
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
    pub providers: Vec<String>,
    /// Every way to obtain it (deduplicated by URI).
    pub addresses: Vec<Address>,
    /// Heuristic rank. Higher is better; only comparable within one `merge`.
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
