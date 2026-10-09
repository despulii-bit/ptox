//! A minimal Kademlia DHT client: bootstrap, `find_node`, iterative `get_peers`.

use crate::bencode::Value;
use crate::krpc::{self, Message, Node, NodeId, NODE_ID_LEN};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::{timeout, Instant};

const ALPHA: usize = 3;
const MAX_ROUNDS: usize = 8;
const TABLE_CAP: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum DhtError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("rpc error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("timed out")]
    Timeout,
    #[error("unexpected message")]
    Unexpected,
}

/// Abstraction over the datagram transport so the lookup logic is testable
/// offline.
#[async_trait]
pub trait DhtTransport: Send + Sync {
    async fn send(&self, addr: SocketAddr, message: &[u8]) -> Result<(), DhtError>;
    async fn recv(&self) -> Result<(Vec<u8>, SocketAddr), DhtError>;
}

/// Production UDP transport.
pub struct UdpTransport {
    socket: tokio::net::UdpSocket,
}

impl UdpTransport {
    pub async fn bind(addr: impl tokio::net::ToSocketAddrs) -> Result<Self, DhtError> {
        let socket = tokio::net::UdpSocket::bind(addr)
            .await
            .map_err(|error| DhtError::Transport(error.to_string()))?;
        Ok(Self { socket })
    }
}

#[async_trait]
impl DhtTransport for UdpTransport {
    async fn send(&self, addr: SocketAddr, message: &[u8]) -> Result<(), DhtError> {
        self.socket
            .send_to(message, addr)
            .await
            .map(|_| ())
            .map_err(|error| DhtError::Transport(error.to_string()))
    }

    async fn recv(&self) -> Result<(Vec<u8>, SocketAddr), DhtError> {
        let mut buf = vec![0u8; 65536];
        let (len, addr) = self
            .socket
            .recv_from(&mut buf)
            .await
            .map_err(|error| DhtError::Transport(error.to_string()))?;
        buf.truncate(len);
        Ok((buf, addr))
    }
}

/// Resolves an infohash to a set of peer addresses.
#[async_trait]
pub trait InfohashResolver: Send + Sync {
    async fn peers(&self, info_hash: NodeId) -> Result<Vec<SocketAddr>, DhtError>;
}

pub struct Dht<T: DhtTransport> {
    pub id: NodeId,
    transport: T,
    routers: Vec<(String, u16)>,
    table: Mutex<Vec<Node>>,
    timeout: Duration,
    counter: AtomicU64,
}

impl<T: DhtTransport> Dht<T> {
    pub fn new(transport: T, routers: Vec<(String, u16)>, timeout: Duration) -> Self {
        Self::with_id(random_id(), transport, routers, timeout)
    }

    pub fn with_id(
        id: NodeId,
        transport: T,
        routers: Vec<(String, u16)>,
        timeout: Duration,
    ) -> Self {
        Self {
            id,
            transport,
            routers,
            table: Mutex::new(Vec::new()),
            timeout,
            counter: AtomicU64::new(0),
        }
    }

    pub fn table_len(&self) -> usize {
        self.table.lock().expect("table lock").len()
    }

    fn next_tid(&self) -> Vec<u8> {
        self.counter
            .fetch_add(1, Ordering::Relaxed)
            .to_be_bytes()
            .to_vec()
    }

    async fn rpc(
        &self,
        addr: SocketAddr,
        method: &str,
        args: BTreeMap<Vec<u8>, Value>,
    ) -> Result<BTreeMap<Vec<u8>, Value>, DhtError> {
        let tid = self.next_tid();
        let message = krpc::query(&tid, method, args);
        self.transport.send(addr, &message).await?;

        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(DhtError::Timeout);
            }
            let (data, _from) = timeout(remaining, self.transport.recv())
                .await
                .map_err(|_| DhtError::Timeout)??;
            match krpc::parse(&data) {
                Ok(Message::Response {
                    tid: response_tid,
                    r,
                }) if response_tid == tid => return Ok(r),
                Ok(Message::Error {
                    tid: response_tid,
                    code,
                    message,
                }) if response_tid == tid => return Err(DhtError::Rpc { code, message }),
                _ => continue,
            }
        }
    }

    /// Populate the routing table from the configured routers, once.
    pub async fn bootstrap(&self) -> Result<(), DhtError> {
        if self.table_len() > 0 {
            return Ok(());
        }
        for (host, port) in &self.routers {
            let Ok(mut resolved) = tokio::net::lookup_host((host.as_str(), *port)).await else {
                continue;
            };
            let Some(addr) = resolved.next() else {
                continue;
            };
            let args = krpc::find_node_args(&self.id, &self.id);
            if let Ok(response) = self.rpc(addr, "find_node", args).await {
                for node in krpc::response_nodes(&response) {
                    self.insert_node(node);
                }
            }
        }
        Ok(())
    }

    pub async fn get_peers(
        &self,
        addr: SocketAddr,
        info_hash: &NodeId,
    ) -> Result<GetPeers, DhtError> {
        let args = krpc::get_peers_args(&self.id, info_hash);
        let response = self.rpc(addr, "get_peers", args).await?;
        Ok(GetPeers {
            token: krpc::response_token(&response),
            values: krpc::response_values(&response),
            nodes: krpc::response_nodes(&response),
        })
    }

    fn insert_node(&self, node: Node) {
        if node.addr.port() == 0 {
            return;
        }
        let mut table = self.table.lock().expect("table lock");
        if table.len() >= TABLE_CAP {
            return;
        }
        if table
            .iter()
            .any(|existing| existing.id == node.id || existing.addr == node.addr)
        {
            return;
        }
        table.push(node);
    }

    fn closest(&self, target: &NodeId, count: usize) -> Vec<Node> {
        let mut nodes = self.table.lock().expect("table lock").clone();
        nodes.sort_by_key(|node| xor_distance(&node.id, target));
        nodes.truncate(count);
        nodes
    }

    async fn lookup(&self, info_hash: &NodeId) -> Result<Vec<SocketAddr>, DhtError> {
        self.bootstrap().await?;
        let mut queried: HashSet<SocketAddr> = HashSet::new();
        let mut peers: Vec<SocketAddr> = Vec::new();

        for _ in 0..MAX_ROUNDS {
            let candidates: Vec<SocketAddr> = self
                .closest(info_hash, 32)
                .into_iter()
                .map(|node| node.addr)
                .filter(|addr| !queried.contains(addr))
                .take(ALPHA)
                .collect();
            if candidates.is_empty() {
                break;
            }
            for addr in candidates {
                queried.insert(addr);
                if let Ok(response) = self.get_peers(addr, info_hash).await {
                    for peer in response.values {
                        if !peers.contains(&peer) {
                            peers.push(peer);
                        }
                    }
                    for node in response.nodes {
                        self.insert_node(node);
                    }
                }
            }
        }
        Ok(peers)
    }
}

#[async_trait]
impl<T: DhtTransport + 'static> InfohashResolver for Dht<T> {
    async fn peers(&self, info_hash: NodeId) -> Result<Vec<SocketAddr>, DhtError> {
        self.lookup(&info_hash).await
    }
}

pub struct GetPeers {
    pub token: Option<Vec<u8>>,
    pub values: Vec<SocketAddr>,
    pub nodes: Vec<Node>,
}

fn xor_distance(a: &NodeId, b: &NodeId) -> [u8; NODE_ID_LEN] {
    let mut distance = [0u8; NODE_ID_LEN];
    for (index, (left, right)) in a.iter().zip(b.iter()).enumerate() {
        distance[index] = left ^ right;
    }
    distance
}

fn random_id() -> NodeId {
    let mut id = [0u8; NODE_ID_LEN];
    getrandom::getrandom(&mut id).expect("os randomness");
    id
}
