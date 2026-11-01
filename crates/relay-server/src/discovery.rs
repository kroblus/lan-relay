//! Foreground LAN address discovery. Nothing received here grants trust.
//! The HTTP pairing state machine still requires explicit local approval.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{net::UdpSocket, task::JoinHandle};

pub const DISCOVERY_PORT: u16 = 4319;
const MAX_PACKET: usize = 1200;
const MAX_PEERS: usize = 64;
const PEER_TTL: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub struct DiscoveredPeer {
    /// An untrusted installation identifier, not the authenticated device ID.
    pub peer_id: String,
    pub name: String,
    pub role: String,
    pub address: Ipv4Addr,
    pub http_port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Packet {
    magic: String,
    v: u8,
    kind: String,
    peer_id: String,
    name: String,
    role: String,
    port: u16,
}

impl Packet {
    fn valid(&self) -> bool {
        self.magic == "lanrelay"
            && self.v == 1
            && matches!(self.kind.as_str(), "probe" | "announce" | "connect")
            && valid_id(&self.peer_id)
            && !self.name.trim().is_empty()
            && self.name.chars().count() <= 80
            && !self.name.chars().any(char::is_control)
            && match self.role.as_str() {
                "android" => self.port == 0 && self.kind != "connect",
                "desktop" => self.port != 0,
                _ => false,
            }
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn private_source(source: SocketAddr) -> Option<SocketAddrV4> {
    match source {
        SocketAddr::V4(source) if source.port() != 0 && private_ipv4(*source.ip()) => Some(source),
        _ => None,
    }
}

/// Match Android's LAN origin policy, including shared address space used by
/// private overlay networks. This classification is not authentication.
pub(crate) fn private_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private() || ip.is_link_local() || (octets[0] == 100 && (64..=127).contains(&octets[1]))
}

fn decode(bytes: &[u8], source: SocketAddr, own_id: &str) -> Option<(Packet, SocketAddrV4)> {
    if bytes.len() > MAX_PACKET {
        return None;
    }
    let source = private_source(source)?;
    let packet: Packet = serde_json::from_slice(bytes).ok()?;
    (packet.valid() && packet.peer_id != own_id).then_some((packet, source))
}

#[derive(Clone)]
struct Entry {
    peer: DiscoveredPeer,
    source: SocketAddrV4,
    seen: Instant,
}

#[derive(Default)]
struct Registry {
    peers: HashMap<String, Entry>,
    replied: HashMap<Ipv4Addr, Instant>,
}

impl Registry {
    fn prune(&mut self, now: Instant) {
        self.peers
            .retain(|_, e| now.duration_since(e.seen) < PEER_TTL);
        self.replied
            .retain(|_, time| now.duration_since(*time) < Duration::from_secs(1));
    }

    fn observe(&mut self, packet: &Packet, source: SocketAddrV4, now: Instant) -> bool {
        self.prune(now);
        // A click/connection hint does not establish presence or consent on the
        // receiving desktop. Android separately asks the user before pairing.
        if packet.kind == "connect" {
            return false;
        }
        if self.peers.len() < MAX_PEERS || self.peers.contains_key(&packet.peer_id) {
            self.peers.insert(
                packet.peer_id.clone(),
                Entry {
                    peer: DiscoveredPeer {
                        peer_id: packet.peer_id.clone(),
                        name: packet.name.trim().to_owned(),
                        role: packet.role.clone(),
                        address: *source.ip(),
                        http_port: packet.port,
                    },
                    source,
                    seen: now,
                },
            );
        }
        if packet.kind != "probe"
            || self.replied.contains_key(source.ip())
            || self.replied.len() >= 256
        {
            return false;
        }
        self.replied.insert(*source.ip(), now);
        true
    }
}

struct Lifetime(Mutex<Option<JoinHandle<()>>>);
impl Drop for Lifetime {
    fn drop(&mut self) {
        if let Ok(task) = self.0.get_mut() {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
    }
}

#[derive(Clone)]
pub struct DiscoveryHandle {
    socket: Arc<UdpSocket>,
    peers: Arc<Mutex<Registry>>,
    packet: Packet,
    _lifetime: Arc<Lifetime>,
}

impl DiscoveryHandle {
    /// Cheap in-memory snapshot; expired devices disappear without HTTP/SQLite.
    pub fn peers(&self) -> Vec<DiscoveredPeer> {
        let Ok(mut registry) = self.peers.lock() else {
            return Vec::new();
        };
        registry.prune(Instant::now());
        let mut peers: Vec<_> = registry
            .peers
            .values()
            .map(|entry| entry.peer.clone())
            .collect();
        peers.sort_by(|a, b| a.name.cmp(&b.name).then(a.peer_id.cmp(&b.peer_id)));
        peers
    }

    /// Only asks the discovered phone to show a connection prompt. It sends no
    /// invitation/session secret and cannot approve a peer or start a transfer.
    pub async fn invite(&self, peer_id: &str) -> io::Result<()> {
        let source = {
            let mut registry = self
                .peers
                .lock()
                .map_err(|_| io::Error::other("设备发现需要重启"))?;
            registry.prune(Instant::now());
            let entry = registry
                .peers
                .get(peer_id)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "设备已离线，请重新查找"))?;
            if entry.peer.role != "android" {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "此版本请从手机发起与电脑的连接",
                ));
            }
            entry.source
        };
        let mut packet = self.packet.clone();
        packet.kind = "connect".into();
        let bytes = serde_json::to_vec(&packet)?;
        self.socket.send_to(&bytes, source).await?;
        Ok(())
    }
}

/// Start once per desktop instance. Failure to bind discovery must not prevent
/// ordinary transfers or manual/QR connection. IPv6 remains a manual fallback.
pub async fn start(server_id: String, name: String, http_port: u16) -> io::Result<DiscoveryHandle> {
    let packet = Packet {
        magic: "lanrelay".into(),
        v: 1,
        kind: "probe".into(),
        peer_id: server_id,
        name,
        role: "desktop".into(),
        port: http_port,
    };
    if !packet.valid() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "设备发现名称或地址不正确",
        ));
    }
    let socket = Arc::new(UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).await?);
    socket.set_broadcast(true)?;
    let registry = Arc::new(Mutex::new(Registry::default()));
    let task_socket = socket.clone();
    let task_registry = registry.clone();
    let own_id = packet.peer_id.clone();
    let probe = serde_json::to_vec(&packet)?;
    let mut announcement = packet.clone();
    announcement.kind = "announce".into();
    let announcement = serde_json::to_vec(&announcement)?;
    let task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // One extra byte lets us reliably reject a truncated oversized datagram.
        let mut buffer = [0u8; MAX_PACKET + 1];
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let _ = task_socket.send_to(&probe, (Ipv4Addr::BROADCAST, DISCOVERY_PORT)).await;
                    if let Ok(mut peers) = task_registry.lock() { peers.prune(Instant::now()); }
                }
                received = task_socket.recv_from(&mut buffer) => {
                    let Ok((length, source)) = received else { break };
                    let Some((packet, source)) = decode(&buffer[..length], source, &own_id) else { continue };
                    let respond = match task_registry.lock() {
                        Ok(mut peers) => peers.observe(&packet, source, Instant::now()),
                        Err(_) => false,
                    };
                    if respond { let _ = task_socket.send_to(&announcement, source).await; }
                }
            }
        }
    });
    Ok(DiscoveryHandle {
        socket,
        peers: registry,
        packet,
        _lifetime: Arc::new(Lifetime(Mutex::new(Some(task)))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "15cb420e-b6d6-4567-9208-81a8190a1d61";
    const OWN: &str = "5da5a0bb-0274-4a18-8dd6-7155752d5cc1";
    fn phone() -> Packet {
        Packet {
            magic: "lanrelay".into(),
            v: 1,
            kind: "probe".into(),
            peer_id: ID.into(),
            name: "我的手机".into(),
            role: "android".into(),
            port: 0,
        }
    }

    #[test]
    fn untrusted_packets_cannot_supply_urls_or_public_endpoints() {
        let bytes = serde_json::to_vec(&phone()).unwrap();
        let source = "192.168.7.9:50612".parse().unwrap();
        assert!(decode(&bytes, source, OWN).is_some());
        assert!(decode(&bytes, "100.64.0.1:4319".parse().unwrap(), OWN).is_some());
        assert!(decode(&bytes, "100.127.255.254:4319".parse().unwrap(), OWN).is_some());
        assert!(decode(&bytes, "100.128.0.1:4319".parse().unwrap(), OWN).is_none());
        assert!(decode(&bytes, source, ID).is_none());
        for ip in [
            "8.8.8.8:4319",
            "127.0.0.1:4319",
            "0.0.0.0:4319",
            "224.0.0.1:4319",
            "[::1]:4319",
        ] {
            assert!(decode(&bytes, ip.parse().unwrap(), OWN).is_none());
        }
        let mut packet = serde_json::to_value(phone()).unwrap();
        packet["origin"] = "https://example.com/".into();
        assert!(decode(&serde_json::to_vec(&packet).unwrap(), source, OWN).is_none());
        assert!(decode(&vec![b' '; MAX_PACKET + 1], source, OWN).is_none());
        let mut packet = phone();
        packet.v = 2;
        assert!(decode(&serde_json::to_vec(&packet).unwrap(), source, OWN).is_none());
        packet.v = 1;
        packet.role = "owner".into();
        assert!(!packet.valid());
    }

    #[test]
    fn duplicate_and_stale_peers_and_reply_rate_are_bounded() {
        let mut registry = Registry::default();
        let now = Instant::now();
        let source = "192.168.7.9:50612".parse().unwrap();
        assert!(registry.observe(&phone(), source, now));
        assert!(!registry.observe(&phone(), source, now));
        assert_eq!(registry.peers.len(), 1);
        assert_eq!(registry.peers[ID].peer.address, *source.ip());
        for n in 0..1000 {
            let mut packet = phone();
            packet.peer_id = format!("{n:08x}-0000-4000-8000-000000000000");
            registry.observe(&packet, source, now);
        }
        assert_eq!(registry.peers.len(), MAX_PEERS);
        registry.prune(now + PEER_TTL);
        assert!(registry.peers.is_empty());
        assert!(registry.replied.is_empty());
    }

    #[test]
    fn desktop_is_discoverable_but_connect_packet_never_trusts_anything() {
        let mut desktop = phone();
        desktop.role = "desktop".into();
        desktop.port = 4318;
        let source = "10.12.3.9:4319".parse().unwrap();
        let mut registry = Registry::default();
        assert!(desktop.valid());
        registry.observe(&desktop, source, Instant::now());
        assert_eq!(registry.peers[ID].peer.http_port, 4318);
        desktop.kind = "connect".into();
        let mut fresh = Registry::default();
        assert!(!fresh.observe(&desktop, source, Instant::now()));
        assert!(fresh.peers.is_empty());
    }
}
