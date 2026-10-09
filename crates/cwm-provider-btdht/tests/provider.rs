//! Provider behavior tested offline against a fake resolver.

use async_trait::async_trait;
use cwm_core::{ErrorCode, FileQuery, FileResult, ProviderError, SearchProvider};
use cwm_provider_btdht::{BtdhtProvider, DhtError, InfohashResolver, NodeId};
use futures::StreamExt;
use std::net::SocketAddr;
use std::sync::Arc;

struct FakeResolver {
    peers: Vec<SocketAddr>,
    fail: bool,
}

#[async_trait]
impl InfohashResolver for FakeResolver {
    async fn peers(&self, _info_hash: NodeId) -> Result<Vec<SocketAddr>, DhtError> {
        if self.fail {
            Err(DhtError::Timeout)
        } else {
            Ok(self.peers.clone())
        }
    }
}

fn provider(peers: Vec<SocketAddr>, fail: bool) -> BtdhtProvider {
    BtdhtProvider::new("btdht", Arc::new(FakeResolver { peers, fail }))
}

async fn search(
    provider: &BtdhtProvider,
    query: &FileQuery,
) -> Vec<Result<FileResult, ProviderError>> {
    let mut stream = provider.search(query);
    let mut items = Vec::new();
    while let Some(item) = stream.next().await {
        items.push(item);
    }
    items
}

fn infohash_query(hex: &str) -> FileQuery {
    let mut query = FileQuery::new("");
    query.hashes = Some(std::collections::HashMap::from([(
        "infohash".to_owned(),
        hex.to_owned(),
    )]));
    query
}

const HEX: &str = "0123456789abcdef0123456789abcdef01234567";

#[tokio::test]
async fn keyword_query_is_unsupported() {
    let items = search(&provider(vec![], false), &FileQuery::new("blade runner")).await;
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].as_ref().unwrap_err().code,
        ErrorCode::unsupported()
    );
}

#[tokio::test]
async fn infohash_resolves_to_a_magnet() {
    let peer: SocketAddr = "127.0.0.1:6881".parse().unwrap();
    let items = search(&provider(vec![peer], false), &infohash_query(HEX)).await;

    assert_eq!(items.len(), 1);
    let result = items.into_iter().next().unwrap().expect("result");
    assert_eq!(result.provider, "btdht");
    assert_eq!(result.addresses[0].scheme, "magnet");
    assert!(result.addresses[0].uri.contains(&format!("urn:btih:{HEX}")));
    assert_eq!(result.raw.unwrap()["peer_count"], 1);
}

#[tokio::test]
async fn magnet_text_is_accepted() {
    let query = FileQuery::new(format!("magnet:?xt=urn:btih:{HEX}&dn=example"));
    let items = search(&provider(vec![], false), &query).await;
    assert!(items[0].is_ok());
}

#[tokio::test]
async fn resolver_failure_becomes_a_transport_error() {
    let items = search(&provider(vec![], true), &infohash_query(HEX)).await;
    assert_eq!(items[0].as_ref().unwrap_err().code, ErrorCode::transport());
}
