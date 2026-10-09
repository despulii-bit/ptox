//! # cwm-provider-btdht
//!
//! A BitTorrent DHT provider for cwm.
//!
//! **Scope:** a real Kademlia DHT client that resolves an infohash (or magnet) to
//! peers and returns a magnet [`cwm_core::Address`]. The DHT is a hash table, not
//! a keyword index, so keyword-only queries yield `unsupported`; keyword search
//! over BitTorrent requires a crawler + metadata fetcher + index (separate
//! software) that would sit on top of this DHT node.

pub mod bencode;
pub mod krpc;
mod node;
mod provider;

pub use krpc::{Node, NodeId};
pub use node::{Dht, DhtError, DhtTransport, GetPeers, InfohashResolver, UdpTransport};
pub use provider::{extract_infohash, parse_infohash, BtdhtProvider};
