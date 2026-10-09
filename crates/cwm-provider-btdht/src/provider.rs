//! The cwm `SearchProvider` for BitTorrent DHT.

use crate::krpc::NodeId;
use crate::node::{DhtError, InfohashResolver};
use async_stream::stream;
use cwm_core::{
    Address, ErrorCode, FileQuery, FileResult, ProviderError, ResultStream, SearchProvider,
};
use std::collections::HashMap;
use std::sync::Arc;

pub struct BtdhtProvider {
    name: String,
    resolver: Arc<dyn InfohashResolver>,
}

impl BtdhtProvider {
    pub fn new(name: impl Into<String>, resolver: Arc<dyn InfohashResolver>) -> Self {
        Self {
            name: name.into(),
            resolver,
        }
    }
}

impl SearchProvider for BtdhtProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn search(&self, query: &FileQuery) -> ResultStream<'_> {
        let name = self.name.clone();
        let resolver = self.resolver.clone();
        let info_hash = extract_infohash(query);

        Box::pin(stream! {
            let Some(info_hash) = info_hash else {
                yield Err(ProviderError::new(
                    ErrorCode::unsupported(),
                    "BitTorrent DHT requires an infohash; keyword search needs an index",
                ));
                return;
            };

            match resolver.peers(info_hash).await {
                Ok(peers) => {
                    let hex = hex_encode(&info_hash);
                    let mut hashes = HashMap::new();
                    hashes.insert("infohash".to_owned(), hex.clone());

                    let mut address = Address::new("magnet", format!("magnet:?xt=urn:btih:{hex}"));
                    address.hashes = Some(hashes.clone());

                    let mut result = FileResult::new(name.clone(), vec![address]);
                    result.title = Some(hex);
                    result.hashes = Some(hashes);
                    result.raw = Some(serde_json::json!({
                        "peer_count": peers.len(),
                        "peers": peers.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    }));
                    yield Ok(result);
                }
                Err(error) => {
                    yield Err(ProviderError::new(ErrorCode::transport(), error.to_string()));
                }
            }
        })
    }
}

impl From<DhtError> for ProviderError {
    fn from(error: DhtError) -> Self {
        ProviderError::new(ErrorCode::transport(), error.to_string())
    }
}

/// Pull an infohash out of a query: from `hashes` or from a magnet / 40-hex text.
pub fn extract_infohash(query: &FileQuery) -> Option<NodeId> {
    if let Some(hashes) = &query.hashes {
        for key in ["infohash", "btih", "sha1"] {
            if let Some(id) = hashes.get(key).and_then(|value| parse_infohash(value)) {
                return Some(id);
            }
        }
        for value in hashes.values() {
            if let Some(id) = parse_infohash(value) {
                return Some(id);
            }
        }
    }
    parse_infohash(&query.text)
}

/// Parse a 40-hex infohash or a magnet URI containing `xt=urn:btih:<hex>`.
pub fn parse_infohash(text: &str) -> Option<NodeId> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("magnet:?") {
        for param in rest.split('&') {
            if let Some(value) = param.strip_prefix("xt=urn:btih:") {
                return parse_hex40(value);
            }
        }
        return None;
    }
    parse_hex40(text)
}

fn parse_hex40(text: &str) -> Option<NodeId> {
    if text.len() != 40 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut id = [0u8; 20];
    for (index, chunk) in text.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(chunk).ok()?;
        id[index] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(id)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
