use cwm::{merge, Address, FileResult, MergeOptions, MergedResult};
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
