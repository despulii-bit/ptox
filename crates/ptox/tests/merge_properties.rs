//! Deterministic property tests for [`ptox::merge`].
//!
//! `merge` is the pure post-processor every client depends on: it deduplicates
//! and ranks the results from many providers, and [`ResultRef`] is the handle
//! clients persist between a search and a later fetch. These tests drive both
//! with pseudo-random inputs and assert the invariants a client relies on. There
//! is deliberately no fuzzing dependency: the PRNG is a fixed splitmix64, so any
//! failure is reproducible from the seed in its test.

use ptox::{
    merge, result_id, Address, FileResult, HashAlgos, MergeOptions, MergedResult, ResultRef,
};
use std::collections::HashMap;

/// A tiny deterministic PRNG (splitmix64). `below(0)` is defined as 0.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, upper: usize) -> usize {
        if upper == 0 {
            0
        } else {
            (self.next_u64() % upper as u64) as usize
        }
    }

    /// A short lowercase word. Unique prefixes keep generated identities stable
    /// and collision-free where a test needs distinct values.
    fn word(&mut self, max_len: usize) -> String {
        (0..1 + self.below(max_len))
            .map(|_| char::from_u32(0x61 + self.below(26) as u32).expect("lowercase ascii"))
            .collect()
    }

    /// A deterministic Fisher-Yates shuffle.
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

fn result(
    provider: &str,
    title: Option<&str>,
    sha256: Option<&str>,
    size: Option<u64>,
) -> FileResult {
    let mut result = FileResult::new(
        provider,
        vec![Address::new("https", format!("https://{provider}/file"))],
    );
    result.title = title.map(str::to_owned);
    result.size = size;
    if let Some(value) = sha256 {
        result
            .hashes
            .get_or_insert_with(HashAlgos::new)
            .insert("sha256".to_owned(), value.to_owned());
    }
    result
}

fn titles(merged: &[MergedResult]) -> Vec<Option<String>> {
    merged.iter().map(|entry| entry.title.clone()).collect()
}

// ---------------------------------------------------------------------------
// Deduplication: a content hash wins over title + size
// ---------------------------------------------------------------------------

#[test]
fn dedup_by_content_hash_ignores_title_and_size() {
    let mut rng = Rng::new(0xDED0_0001);
    for _ in 0..1_000 {
        let hash = format!("{:x}", rng.next_u64());
        let first = result("a", Some(&rng.word(12)), Some(&hash), Some(rng.next_u64()));
        let second = result("b", Some(&rng.word(12)), Some(&hash), Some(rng.next_u64()));

        let merged = merge(vec![first, second], &MergeOptions::default());
        assert_eq!(
            merged.len(),
            1,
            "results sharing a sha256 must dedup regardless of title/size"
        );
        assert_eq!(merged[0].providers, ["a", "b"]);
    }
}

#[test]
fn hash_identity_is_transitive_across_algorithms() {
    let mut rng = Rng::new(0xDED0_0002);
    for _ in 0..1_000 {
        // a shares sha256 with b; b shares sha1 with c; a and c share nothing
        // directly. Hash identity must chain them into one cluster.
        let sha256 = format!("{:x}", rng.next_u64());
        let sha1 = format!("{:x}", rng.next_u64());

        let a = result("a", Some("a.bin"), Some(&sha256), Some(1));
        let mut b = result("b", Some("b.bin"), Some(&sha256), Some(2));
        b.hashes
            .as_mut()
            .expect("b has a hash")
            .insert("sha1".to_owned(), sha1.clone());
        let mut c = result("c", Some("c.bin"), None, Some(3));
        c.hashes = Some(HashMap::from([("sha1".to_owned(), sha1)]));

        let merged = merge(vec![a, b, c], &MergeOptions::default());
        assert_eq!(merged.len(), 1, "hash identity must be transitive");
        assert_eq!(merged[0].providers, ["a", "b", "c"]);
    }
}

#[test]
fn same_name_and_size_without_hashes_dedups() {
    let mut rng = Rng::new(0xDED0_0003);
    for _ in 0..1_000 {
        let title = rng.word(12);
        let size = rng.next_u64();
        // `merge` normalizes title (trim, case-fold, collapse whitespace).
        let spelling = format!("  {}  ", title.to_uppercase());

        let merged = merge(
            vec![
                result("a", Some(&title), None, Some(size)),
                result("b", Some(&spelling), None, Some(size)),
            ],
            &MergeOptions::default(),
        );
        assert_eq!(merged.len(), 1, "same title+size with no hashes must dedup");
    }
}

#[test]
fn different_hashes_never_dedup_even_with_matching_name_and_size() {
    let mut rng = Rng::new(0xDED0_0004);
    for _ in 0..1_000 {
        let title = rng.word(12);
        let size = rng.next_u64();
        let one = format!("{:x}", rng.next_u64());
        let mut two = one.clone();
        while two == one {
            two = format!("{:x}", rng.next_u64());
        }

        let merged = merge(
            vec![
                result("a", Some(&title), Some(&one), Some(size)),
                result("b", Some(&title), Some(&two), Some(size)),
            ],
            &MergeOptions::default(),
        );
        assert_eq!(
            merged.len(),
            2,
            "different hashes must stay separate even when name+size match"
        );
    }
}

#[test]
fn a_content_hash_suppresses_name_dedup() {
    let mut rng = Rng::new(0xDED0_0005);
    for _ in 0..1_000 {
        let title = rng.word(12);
        let size = rng.next_u64();
        let hash = format!("{:x}", rng.next_u64());

        // The hashed result keys on its hash; the hash-less one keys on
        // name+size. No key overlaps, so they must not merge.
        let merged = merge(
            vec![
                result("a", Some(&title), Some(&hash), Some(size)),
                result("b", Some(&title), None, Some(size)),
            ],
            &MergeOptions::default(),
        );
        assert_eq!(
            merged.len(),
            2,
            "a present hash must take precedence over name+size"
        );
    }
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

#[test]
fn merge_result_order_is_independent_of_input_order() {
    let mut rng = Rng::new(0x0DDE_0001);
    for _ in 0..300 {
        let files = 2 + rng.below(5);
        let mut results = Vec::new();
        for i in 0..files {
            // Unique hash and title per file, so dedup is exact and the
            // (score, title) sort key is unique.
            let title = format!("file-{i}-{}", rng.word(6));
            let hash = format!("h{i}-{:x}", rng.next_u64());
            let size = rng.next_u64();
            for provider in 0..1 + rng.below(3) {
                results.push(result(
                    &format!("p{provider}"),
                    Some(&title),
                    Some(&hash),
                    Some(size),
                ));
            }
        }

        let forward = merge(results.clone(), &MergeOptions::default());
        rng.shuffle(&mut results);
        let shuffled = merge(results, &MergeOptions::default());

        assert_eq!(titles(&forward), titles(&shuffled));
        // The output is sorted by score, descending.
        for window in forward.windows(2) {
            assert!(
                window[0].score >= window[1].score,
                "merge output must be ordered by descending score"
            );
        }
    }
}

#[test]
fn score_is_monotonic_in_agreeing_providers() {
    let mut previous = None;
    for agreeing in 1..=8usize {
        let results: Vec<_> = (0..agreeing)
            .map(|provider| {
                result(
                    &format!("p{provider}"),
                    Some("shared.bin"),
                    Some("deadbeef"),
                    Some(42),
                )
            })
            .collect();
        let merged = merge(results, &MergeOptions::default());
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].providers.len(), agreeing);

        let score = merged[0].score;
        if let Some(previous) = previous {
            assert!(
                score > previous,
                "score must grow with each agreeing provider"
            );
            assert_eq!(score - previous, 1.0);
        }
        previous = Some(score);
    }
}

#[test]
fn repeated_provider_does_not_inflate_score() {
    let one = merge(
        vec![result("p", Some("t"), Some("h"), Some(1))],
        &MergeOptions::default(),
    );
    let three = merge(
        vec![
            result("p", Some("t"), Some("h"), Some(1)),
            result("p", Some("t"), Some("h"), Some(1)),
            result("p", Some("t"), Some("h"), Some(1)),
        ],
        &MergeOptions::default(),
    );
    assert_eq!(three.len(), 1);
    assert_eq!(three[0].providers, ["p"]);
    assert_eq!(three[0].score, one[0].score);
}

#[test]
fn score_adds_each_agreeing_providers_weight() {
    let mut rng = Rng::new(0x0DDE_0002);
    for _ in 0..500 {
        let weight_a = rng.below(1_000) as f64 / 100.0;
        let weight_b = rng.below(1_000) as f64 / 100.0;
        let options = MergeOptions {
            provider_weights: HashMap::from([
                ("a".to_owned(), weight_a),
                ("b".to_owned(), weight_b),
            ]),
        };

        let merged = merge(
            vec![
                result("a", Some("t"), Some("h"), Some(1)),
                result("b", Some("t"), Some("h"), Some(1)),
            ],
            &options,
        );
        assert_eq!(merged.len(), 1);
        // Two agreeing weighted providers plus the hash (+0.5) and size (+0.25)
        // bonuses.
        assert!(
            (merged[0].score - (weight_a + weight_b + 0.75)).abs() < 1e-9,
            "score {} != {}",
            merged[0].score,
            weight_a + weight_b + 0.75
        );
    }
}

// ---------------------------------------------------------------------------
// ResultRef: the persisted fetch handle
// ---------------------------------------------------------------------------

fn random_merged(rng: &mut Rng) -> MergedResult {
    let mut result = MergedResult::default();
    if rng.below(2) == 0 {
        result.title = Some(rng.word(12));
    }
    if rng.below(2) == 0 {
        result.size = Some(rng.next_u64());
    }
    if rng.below(2) == 0 {
        result.mimetype = Some(rng.word(8));
    }
    if rng.below(2) == 0 {
        let mut hashes = HashAlgos::new();
        for _ in 0..1 + rng.below(3) {
            hashes.insert(rng.word(6), format!("{:x}", rng.next_u64()));
        }
        result.hashes = hashes;
    }
    for _ in 0..rng.below(4) {
        result.addresses.push(Address::new(
            rng.word(5),
            format!("{}://{}", rng.word(3), rng.word(8)),
        ));
    }
    result
}

#[test]
fn result_ref_round_trips_fetch_essential_fields() {
    let mut rng = Rng::new(0x02EF_0001);
    for _ in 0..2_000 {
        let original = random_merged(&mut rng);
        let reference = ResultRef::from_merged(&original);
        assert_eq!(reference.id, result_id(&original));

        let restored = reference.to_merged();
        assert_eq!(restored.title, original.title);
        assert_eq!(restored.size, original.size);
        assert_eq!(restored.mimetype, original.mimetype);
        assert_eq!(restored.hashes, original.hashes);
        assert_eq!(restored.addresses, original.addresses);
        // Search-only fields are intentionally not part of the handle.
        assert!(restored.providers.is_empty());
        assert_eq!(restored.score, 0.0);

        // The id is derived, so reconverting yields the same handle, and the
        // handle survives the JSON round trip a client actually persists.
        assert_eq!(ResultRef::from_merged(&restored), reference);
        let json = serde_json::to_string(&reference).expect("serialize");
        let decoded: ResultRef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, reference);
    }
}
