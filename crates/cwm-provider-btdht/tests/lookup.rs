//! Iterative DHT lookup tested offline against a scripted transport.

use async_trait::async_trait;
use cwm_provider_btdht::bencode::Value;
use cwm_provider_btdht::krpc::{self, Message, Node};
use cwm_provider_btdht::{Dht, DhtError, DhtTransport, InfohashResolver};
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

struct MockTransport {
    state: Mutex<MockState>,
}

struct MockState {
    replies: VecDeque<BTreeMap<Vec<u8>, Value>>,
    sent: Vec<Vec<u8>>,
}

impl MockTransport {
    fn new(replies: Vec<BTreeMap<Vec<u8>, Value>>) -> Self {
        Self {
            state: Mutex::new(MockState {
                replies: replies.into(),
                sent: Vec::new(),
            }),
        }
    }
}

#[async_trait]
impl DhtTransport for MockTransport {
    async fn send(&self, _addr: SocketAddr, message: &[u8]) -> Result<(), DhtError> {
        self.state.lock().unwrap().sent.push(message.to_vec());
        Ok(())
    }

    async fn recv(&self) -> Result<(Vec<u8>, SocketAddr), DhtError> {
        let mut state = self.state.lock().unwrap();
        let last = state.sent.last().cloned().ok_or(DhtError::Timeout)?;
        let tid =
            match krpc::parse(&last).map_err(|error| DhtError::Transport(error.to_string()))? {
                Message::Query { tid, .. } => tid,
                _ => return Err(DhtError::Transport("expected a query".to_owned())),
            };
        let reply = state.replies.pop_front().ok_or(DhtError::Timeout)?;
        Ok((
            krpc::response(&tid, reply),
            "127.0.0.1:6881".parse().unwrap(),
        ))
    }
}

fn dict(pairs: Vec<(&str, Value)>) -> BTreeMap<Vec<u8>, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.as_bytes().to_vec(), value))
        .collect()
}

#[tokio::test]
async fn iterative_lookup_bootstraps_and_collects_peers() {
    let node1 = Node {
        id: [1u8; 20],
        addr: "127.0.0.1:1".parse().unwrap(),
    };
    let node2 = Node {
        id: [2u8; 20],
        addr: "127.0.0.1:2".parse().unwrap(),
    };
    let peer: SocketAddr = "127.0.0.1:9999".parse().unwrap();

    let replies = vec![
        // bootstrap find_node
        dict(vec![
            ("id", Value::bytes([9u8; 20].to_vec())),
            (
                "nodes",
                Value::bytes(krpc::encode_nodes(std::slice::from_ref(&node1))),
            ),
        ]),
        // get_peers -> node1 returns a peer and a closer node
        dict(vec![
            ("id", Value::bytes(node1.id.to_vec())),
            ("token", Value::bytes(b"tok".to_vec())),
            ("values", Value::bytes(krpc::encode_peers(&[peer]))),
            (
                "nodes",
                Value::bytes(krpc::encode_nodes(std::slice::from_ref(&node2))),
            ),
        ]),
        // get_peers -> node2 returns no peers
        dict(vec![
            ("id", Value::bytes(node2.id.to_vec())),
            ("token", Value::bytes(b"tok".to_vec())),
            ("nodes", Value::bytes(Vec::<u8>::new())),
        ]),
    ];

    let dht = Dht::with_id(
        [7u8; 20],
        MockTransport::new(replies),
        vec![("127.0.0.1".to_owned(), 6881)],
        Duration::from_millis(100),
    );

    let peers = dht.peers([3u8; 20]).await.expect("lookup");
    assert!(peers.contains(&peer), "expected {peer}, got {peers:?}");
    assert_eq!(dht.table_len(), 2);
}
