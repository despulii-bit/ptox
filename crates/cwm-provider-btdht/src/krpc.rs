//! KRPC messages (BEP 5) over bencode.

use crate::bencode::{self, Value};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub const NODE_ID_LEN: usize = 20;

pub type NodeId = [u8; NODE_ID_LEN];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub addr: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Query {
        tid: Vec<u8>,
        method: String,
        args: BTreeMap<Vec<u8>, Value>,
    },
    Response {
        tid: Vec<u8>,
        r: BTreeMap<Vec<u8>, Value>,
    },
    Error {
        tid: Vec<u8>,
        code: i64,
        message: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum KrpcError {
    #[error("bencode error: {0}")]
    Bencode(#[from] bencode::BencodeError),
    #[error("malformed krpc message")]
    Malformed,
}

pub fn query(tid: &[u8], method: &str, args: BTreeMap<Vec<u8>, Value>) -> Vec<u8> {
    let mut dict = BTreeMap::new();
    dict.insert(b"t".to_vec(), Value::bytes(tid.to_vec()));
    dict.insert(b"y".to_vec(), Value::string("q"));
    dict.insert(b"q".to_vec(), Value::string(method));
    dict.insert(b"a".to_vec(), Value::Dict(args));
    bencode::encode(&Value::Dict(dict))
}

pub fn response(tid: &[u8], r: BTreeMap<Vec<u8>, Value>) -> Vec<u8> {
    let mut dict = BTreeMap::new();
    dict.insert(b"t".to_vec(), Value::bytes(tid.to_vec()));
    dict.insert(b"y".to_vec(), Value::string("r"));
    dict.insert(b"r".to_vec(), Value::Dict(r));
    bencode::encode(&Value::Dict(dict))
}

pub fn ping_args(id: &NodeId) -> BTreeMap<Vec<u8>, Value> {
    BTreeMap::from([(b"id".to_vec(), Value::bytes(id.to_vec()))])
}

pub fn find_node_args(id: &NodeId, target: &NodeId) -> BTreeMap<Vec<u8>, Value> {
    let mut args = ping_args(id);
    args.insert(b"target".to_vec(), Value::bytes(target.to_vec()));
    args
}

pub fn get_peers_args(id: &NodeId, info_hash: &NodeId) -> BTreeMap<Vec<u8>, Value> {
    let mut args = ping_args(id);
    args.insert(b"info_hash".to_vec(), Value::bytes(info_hash.to_vec()));
    args
}

pub fn parse(input: &[u8]) -> Result<Message, KrpcError> {
    let value = bencode::decode(input)?;
    let dict = value.as_dict().ok_or(KrpcError::Malformed)?;
    let tid = dict
        .get(&b"t"[..])
        .and_then(Value::as_bytes)
        .unwrap_or_default()
        .to_vec();
    match dict.get(&b"y"[..]).and_then(Value::as_str) {
        Some("q") => {
            let method = dict
                .get(&b"q"[..])
                .and_then(Value::as_str)
                .ok_or(KrpcError::Malformed)?
                .to_owned();
            let args = dict
                .get(&b"a"[..])
                .and_then(Value::as_dict)
                .cloned()
                .ok_or(KrpcError::Malformed)?;
            Ok(Message::Query { tid, method, args })
        }
        Some("r") => {
            let r = dict
                .get(&b"r"[..])
                .and_then(Value::as_dict)
                .cloned()
                .ok_or(KrpcError::Malformed)?;
            Ok(Message::Response { tid, r })
        }
        Some("e") => {
            let list = dict
                .get(&b"e"[..])
                .and_then(Value::as_list)
                .ok_or(KrpcError::Malformed)?;
            let code = list.first().and_then(Value::as_int).unwrap_or_default();
            let message = list
                .get(1)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Ok(Message::Error { tid, code, message })
        }
        _ => Err(KrpcError::Malformed),
    }
}

pub fn response_id(response: &BTreeMap<Vec<u8>, Value>) -> Option<NodeId> {
    copy_id(response.get(&b"id"[..]).and_then(Value::as_bytes)?)
}

pub fn response_token(response: &BTreeMap<Vec<u8>, Value>) -> Option<Vec<u8>> {
    response
        .get(&b"token"[..])
        .and_then(Value::as_bytes)
        .map(<[u8]>::to_vec)
}

pub fn response_nodes(response: &BTreeMap<Vec<u8>, Value>) -> Vec<Node> {
    response
        .get(&b"nodes"[..])
        .and_then(Value::as_bytes)
        .map(decode_nodes)
        .unwrap_or_default()
}

pub fn response_values(response: &BTreeMap<Vec<u8>, Value>) -> Vec<SocketAddr> {
    response
        .get(&b"values"[..])
        .and_then(Value::as_bytes)
        .map(decode_peers)
        .unwrap_or_default()
}

pub fn decode_nodes(bytes: &[u8]) -> Vec<Node> {
    bytes
        .chunks_exact(26)
        .filter_map(|chunk| {
            let id = copy_id(&chunk[..NODE_ID_LEN])?;
            let ip = Ipv4Addr::new(chunk[20], chunk[21], chunk[22], chunk[23]);
            let port = u16::from_be_bytes([chunk[24], chunk[25]]);
            Some(Node {
                id,
                addr: SocketAddr::new(IpAddr::V4(ip), port),
            })
        })
        .collect()
}

pub fn encode_nodes(nodes: &[Node]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nodes.len() * 26);
    for node in nodes {
        out.extend_from_slice(&node.id);
        if let SocketAddr::V4(addr) = node.addr {
            out.extend_from_slice(&addr.ip().octets());
            out.extend_from_slice(&addr.port().to_be_bytes());
        }
    }
    out
}

pub fn decode_peers(bytes: &[u8]) -> Vec<SocketAddr> {
    bytes
        .chunks_exact(6)
        .map(|chunk| {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            SocketAddr::new(IpAddr::V4(ip), port)
        })
        .collect()
}

pub fn encode_peers(peers: &[SocketAddr]) -> Vec<u8> {
    let mut out = Vec::with_capacity(peers.len() * 6);
    for peer in peers {
        if let SocketAddr::V4(addr) = peer {
            out.extend_from_slice(&addr.ip().octets());
            out.extend_from_slice(&addr.port().to_be_bytes());
        }
    }
    out
}

fn copy_id(bytes: &[u8]) -> Option<NodeId> {
    if bytes.len() != NODE_ID_LEN {
        return None;
    }
    let mut id = [0u8; NODE_ID_LEN];
    id.copy_from_slice(bytes);
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_round_trip() {
        let id = [1u8; NODE_ID_LEN];
        let target = [2u8; NODE_ID_LEN];
        let bytes = query(&[0xab, 0xcd], "find_node", find_node_args(&id, &target));
        match parse(&bytes).expect("parse") {
            Message::Query { tid, method, args } => {
                assert_eq!(tid, vec![0xab, 0xcd]);
                assert_eq!(method, "find_node");
                assert_eq!(
                    args.get(&b"target"[..]).and_then(Value::as_bytes),
                    Some(&target[..])
                );
            }
            other => panic!("expected query, got {other:?}"),
        }
    }

    #[test]
    fn decodes_and_encodes_compact_nodes_and_peers() {
        let node = Node {
            id: [7u8; NODE_ID_LEN],
            addr: "127.0.0.1:6881".parse().expect("addr"),
        };
        let encoded = encode_nodes(std::slice::from_ref(&node));
        assert_eq!(encoded.len(), 26);
        assert_eq!(decode_nodes(&encoded), vec![node]);

        let peers = vec!["10.0.0.5:1234".parse::<SocketAddr>().expect("addr")];
        assert_eq!(decode_peers(&encode_peers(&peers)), peers);
    }

    #[test]
    fn parses_errors() {
        let bytes = bencode::encode(&Value::Dict(BTreeMap::from([
            (b"t".to_vec(), Value::bytes(vec![9])),
            (b"y".to_vec(), Value::string("e")),
            (
                b"e".to_vec(),
                Value::List(vec![Value::Int(201), Value::string("generic error")]),
            ),
        ])));
        match parse(&bytes).expect("parse") {
            Message::Error { code, message, .. } => {
                assert_eq!(code, 201);
                assert_eq!(message, "generic error");
            }
            other => panic!("expected error, got {other:?}"),
        }
    }
}
