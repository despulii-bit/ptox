use ptox::{
    merge, result_id, suggest_filename, Address, FileResult, MergeOptions, MergedResult, ResultRef,
};
use std::collections::HashMap;

fn result(provider: &str, title: &str, sha256: Option<&str>, size: Option<u64>) -> FileResult {
    let mut result = FileResult::new(
        provider,
        vec![Address::new(
            "https",
            format!("https://{provider}/{}", title),
        )],
    );
    result.title = Some(title.to_owned());
    result.size = size;
    if let Some(value) = sha256 {
        result
            .hashes
            .get_or_insert_with(HashMap::new)
            .insert("sha256".to_owned(), value.to_owned());
    }
    result
}

fn titles(merged: &[MergedResult]) -> Vec<String> {
    merged
        .iter()
        .map(|entry| entry.title.clone().unwrap_or_default())
        .collect()
}

#[test]
fn deduplicates_by_content_hash() {
    let merged = merge(
        vec![
            result("a", "a.bin", Some("deadbeef"), Some(10)),
            result("b", "b.bin", Some("deadbeef"), Some(10)),
        ],
        &MergeOptions::default(),
    );
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].providers, vec!["a", "b"]);
    assert_eq!(merged[0].addresses.len(), 2);
}

#[test]
fn deduplicates_by_title_and_size_without_hashes() {
    let merged = merge(
        vec![
            result("a", "The File.PDF", None, Some(42)),
            result("b", "the  file.pdf", None, Some(42)),
        ],
        &MergeOptions::default(),
    );
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].providers, vec!["a", "b"]);
}

#[test]
fn keeps_different_files_separate() {
    let merged = merge(
        vec![
            result("a", "one.bin", Some("1111"), Some(1)),
            result("a", "two.bin", Some("2222"), Some(2)),
        ],
        &MergeOptions::default(),
    );
    assert_eq!(merged.len(), 2);
}

#[test]
fn multi_source_ranks_first() {
    let merged = merge(
        vec![
            result("a", "single.bin", Some("1111"), Some(1)),
            result("a", "shared.bin", Some("9999"), Some(2)),
            result("b", "shared.bin", Some("9999"), Some(2)),
        ],
        &MergeOptions::default(),
    );
    assert_eq!(titles(&merged)[0], "shared.bin");
}

#[test]
fn access_is_the_most_available_across_sources() {
    use ptox::Access;

    let mut borrow = result("a", "same", None, Some(1));
    borrow.access = Some(Access::Borrow);
    let mut download = result("b", "same", None, Some(1));
    download.access = Some(Access::Download);

    // They dedup (same title+size); access is the most available: a download.
    let merged = merge([borrow, download], &MergeOptions::default());
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].access, Some(Access::Download));

    // Undeclared stays `None`.
    let plain = result("c", "other", None, Some(2));
    let merged = merge([plain], &MergeOptions::default());
    assert_eq!(merged[0].access, None);
}

#[test]
fn provider_weights_change_order() {
    let results = vec![
        result("a", "from-a.bin", Some("1111"), Some(1)),
        result("b", "from-b.bin", Some("2222"), Some(1)),
    ];
    let weighted = MergeOptions {
        provider_weights: HashMap::from([("b".to_owned(), 5.0)]),
    };
    let merged = merge(results, &weighted);
    assert_eq!(titles(&merged)[0], "from-b.bin");
}

fn merged(title: Option<&str>, addresses: Vec<Address>, mimetype: Option<&str>) -> MergedResult {
    MergedResult {
        title: title.map(str::to_owned),
        mimetype: mimetype.map(str::to_owned),
        providers: vec!["a".to_owned()],
        addresses,
        ..MergedResult::default()
    }
}

#[test]
fn suggest_filename_prefers_title() {
    let result = merged(
        Some("Annual Report 2020"),
        vec![Address::new("https", "https://example.org/files/other.pdf")],
        None,
    );
    assert_eq!(suggest_filename(&result), "Annual Report 2020");
}

#[test]
fn suggest_filename_sanitizes_illegal_characters() {
    let result = merged(Some("a/b\\c:d*e?f\"g<h>i|j"), vec![], None);
    assert_eq!(suggest_filename(&result), "a_b_c_d_e_f_g_h_i_j");
}

#[test]
fn suggest_filename_drops_trailing_dots() {
    let result = merged(Some("report..."), vec![], None);
    assert_eq!(suggest_filename(&result), "report");
}

#[test]
fn suggest_filename_falls_back_to_preferred_address_basename() {
    let result = merged(
        None,
        vec![
            Address::new("magnet", "magnet:?xt=urn:btih:deadbeef"),
            Address::new("https", "https://example.org/dir/file.pdf?token=1#frag"),
        ],
        None,
    );
    assert_eq!(suggest_filename(&result), "file.pdf");
}

#[test]
fn suggest_filename_empty_title_uses_address() {
    let result = merged(
        Some("   "),
        vec![Address::new("https", "https://example.org/dir/a.zip")],
        None,
    );
    assert_eq!(suggest_filename(&result), "a.zip");
}

#[test]
fn suggest_filename_appends_mimetype_extension() {
    let result = merged(Some("Annual Report"), vec![], Some("application/pdf"));
    assert_eq!(suggest_filename(&result), "Annual Report.pdf");
}

#[test]
fn suggest_filename_keeps_existing_extension() {
    let result = merged(Some("report.pdf"), vec![], Some("application/pdf"));
    assert_eq!(suggest_filename(&result), "report.pdf");
}

#[test]
fn suggest_filename_last_resort() {
    let result = merged(
        None,
        vec![Address::new("magnet", "magnet:?xt=urn:btih:deadbeef")],
        None,
    );
    assert_eq!(suggest_filename(&result), "download.bin");
}

#[test]
fn persisted_merged_result_stays_deserializable() {
    // A blob shaped like what an older 0.1.x client would have stored. If this
    // stops parsing, an SDK upgrade has silently invalidated client state.
    let json = r#"{
        "title": "annual-report.pdf",
        "size": 12345,
        "mimetype": "application/pdf",
        "hashes": {"sha256": "abc"},
        "providers": ["archive-org", "wikimedia"],
        "addresses": [{"scheme": "https", "uri": "https://example.org/a.pdf"}],
        "score": 1.75
    }"#;
    let result: MergedResult = serde_json::from_str(json).expect("deserialize");
    assert_eq!(result.title.as_deref(), Some("annual-report.pdf"));
    assert_eq!(result.providers, ["archive-org", "wikimedia"]);
    assert_eq!(result.addresses.len(), 1);
    assert_eq!(result.score, 1.75);
    assert_eq!(result.hashes.get("sha256").map(String::as_str), Some("abc"));
}

#[test]
fn persisted_merged_result_tolerates_added_and_missing_fields() {
    // Forward compatibility: unknown fields are ignored.
    let forward = r#"{
        "title": "x.bin",
        "providers": ["a"],
        "addresses": [{"scheme": "http", "uri": "http://x/y"}],
        "score": 1.0,
        "future_field": {"nested": true}
    }"#;
    assert!(serde_json::from_str::<MergedResult>(forward).is_ok());

    // Backward compatibility: optional fields added later have defaults.
    let minimal = r#"{"addresses": [{"scheme": "http", "uri": "http://x/y"}]}"#;
    let result: MergedResult = serde_json::from_str(minimal).expect("deserialize minimal");
    assert!(result.providers.is_empty());
    assert_eq!(result.score, 0.0);
}

fn ref_source() -> MergedResult {
    MergedResult {
        title: Some("Annual Report".to_owned()),
        size: Some(2048),
        mimetype: Some("application/pdf".to_owned()),
        hashes: HashMap::from([("sha256".to_owned(), "abc123".to_owned())]),
        providers: vec!["archive-org".to_owned()],
        addresses: vec![
            Address::new("magnet", "magnet:?xt=urn:btih:deadbeef"),
            Address::new("https", "https://example.org/report.pdf"),
        ],
        score: 7.5,
        ..MergedResult::default()
    }
}

#[test]
fn result_id_prefers_content_hash_in_algorithm_order() {
    let result = MergedResult {
        title: Some("ignored".to_owned()),
        size: Some(1),
        hashes: HashMap::from([
            ("sha1".to_owned(), "aaa".to_owned()),
            ("sha256".to_owned(), "bbb".to_owned()),
        ]),
        ..MergedResult::default()
    };
    assert_eq!(result_id(&result), "sha256:bbb");
}

#[test]
fn result_id_falls_back_in_order() {
    let named = MergedResult {
        title: Some("The  File.PDF".to_owned()),
        size: Some(42),
        addresses: vec![Address::new("https", "https://example.org/a")],
        ..MergedResult::default()
    };
    assert_eq!(result_id(&named), "name:the file.pdf:42");

    let addressed = MergedResult {
        addresses: vec![Address::new("https", "https://example.org/a")],
        ..MergedResult::default()
    };
    assert_eq!(result_id(&addressed), "uri:https://example.org/a");

    assert_eq!(result_id(&MergedResult::default()), "anonymous");
}

#[test]
fn result_ref_round_trips_through_serde() {
    let original = ref_source();
    let result_ref = ResultRef::from_merged(&original);
    assert!(result_ref.is_current());
    assert_eq!(result_ref.id, "sha256:abc123");

    let json = serde_json::to_string(&result_ref).expect("serialize");
    let decoded: ResultRef = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, result_ref);

    let recovered = decoded.to_merged();
    assert_eq!(recovered.title, original.title);
    assert_eq!(recovered.size, original.size);
    assert_eq!(recovered.mimetype, original.mimetype);
    assert_eq!(recovered.hashes, original.hashes);
    assert_eq!(recovered.addresses, original.addresses);
    // Search-only fields are intentionally not persisted.
    assert!(recovered.providers.is_empty());
    assert_eq!(recovered.score, 0.0);
}

#[test]
fn result_ref_preserves_the_suggested_filename() {
    let original = ref_source();
    let result_ref = ResultRef::from_merged(&original);
    assert_eq!(
        suggest_filename(&result_ref.to_merged()),
        suggest_filename(&original)
    );
}

#[test]
fn result_ref_fixed_blob_stays_deserializable() {
    // A blob shaped like what a 0.1.x client would have stored. If this stops
    // parsing, an SDK upgrade has silently invalidated client state.
    let json = r#"{
        "version": 1,
        "id": "sha256:abc",
        "title": "annual-report.pdf",
        "mimetype": "application/pdf",
        "size": 12345,
        "hashes": {"sha256": "abc"},
        "addresses": [{"scheme": "https", "uri": "https://example.org/a.pdf"}]
    }"#;
    let result_ref: ResultRef = serde_json::from_str(json).expect("deserialize");
    assert!(result_ref.is_current());
    assert_eq!(result_ref.id, "sha256:abc");
    assert_eq!(result_ref.to_merged().addresses.len(), 1);
}

#[test]
fn result_ref_minimal_and_forward_blobs_are_tolerated() {
    // Missing version defaults to the current one; unknown fields are ignored.
    let minimal = r#"{"id":"anonymous","addresses":[{"scheme":"http","uri":"http://x/y"}]}"#;
    let result_ref: ResultRef = serde_json::from_str(minimal).expect("deserialize minimal");
    assert_eq!(result_ref.version, ResultRef::VERSION);
    assert!(result_ref.is_current());

    let forward = r#"{"version":1,"id":"sha256:x","addresses":[],"future":true}"#;
    assert!(serde_json::from_str::<ResultRef>(forward).is_ok());
}
