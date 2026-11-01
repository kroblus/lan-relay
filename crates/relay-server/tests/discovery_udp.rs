//! Run explicitly against an isolated local private interface:
//! LANRELAY_DISCOVERY_TEST_IP=192.168.x.y cargo test -p relay-server --test discovery_udp -- --ignored
use relay_server::discovery::{self, DISCOVERY_PORT};
use serde_json::{json, Value};
use std::{net::Ipv4Addr, time::Duration};
use tokio::{net::UdpSocket, time::timeout};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires an explicitly selected isolated private IPv4 interface and free UDP 4319"]
async fn real_udp_probe_invite_and_last_handle_shutdown() {
    let ip: Ipv4Addr = std::env::var("LANRELAY_DISCOVERY_TEST_IP")
        .expect("set LANRELAY_DISCOVERY_TEST_IP to an isolated local interface")
        .parse()
        .unwrap();
    assert!(ip.is_private() || ip.is_link_local());
    let server_id = Uuid::new_v4().to_string();
    let phone_id = Uuid::new_v4().to_string();
    let discovery = discovery::start(server_id.clone(), "隔离 UDP 测试电脑".into(), 14338)
        .await
        .unwrap();
    let phone = UdpSocket::bind((ip, 0)).await.unwrap();
    let probe = json!({"magic":"lanrelay","v":1,"kind":"probe","peer_id":phone_id,"name":"隔离测试手机","role":"android","port":0});
    phone
        .send_to(&serde_json::to_vec(&probe).unwrap(), (ip, DISCOVERY_PORT))
        .await
        .unwrap();
    let mut buffer = [0u8; 1201];
    let (n, from) = timeout(Duration::from_secs(2), phone.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_slice(&buffer[..n]).unwrap();
    assert_eq!(from.ip(), ip);
    assert_eq!(response["kind"], "announce");
    assert_eq!(response["peer_id"], server_id);
    assert_eq!(response["port"], 14338);
    let peers = discovery.peers();
    let peer = peers.iter().find(|peer| peer.peer_id == phone_id).unwrap();
    assert_eq!(peer.address, ip);
    assert_eq!(peer.name, "隔离测试手机");
    discovery.invite(&phone_id).await.unwrap();
    let (n, _) = timeout(Duration::from_secs(2), phone.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let hint: Value = serde_json::from_slice(&buffer[..n]).unwrap();
    assert_eq!(hint["kind"], "connect");
    assert_eq!(hint["peer_id"], server_id);
    assert!(hint.get("token").is_none());
    assert!(hint.get("claim_token").is_none());
    assert!(hint.get("verification_code").is_none());
    let copy = discovery.clone();
    drop(discovery);
    assert!(UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT))
        .await
        .is_err());
    drop(copy);
    tokio::task::yield_now().await;
    let rebound = timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).await {
                break socket;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    drop(rebound);
}
