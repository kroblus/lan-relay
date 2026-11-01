use relay_core::*;
use tempfile::TempDir;
use uuid::Uuid;

fn request(relay: &Relay, peer_id: String) -> NewDiscoveryPairRequest {
    NewDiscoveryPairRequest {
        name: "我的手机".into(),
        peer_id,
        server_id: relay.hello().server_id,
    }
}

#[test]
fn discovery_only_creates_pending_pair_and_keeps_claim_secret_private() {
    let dir = TempDir::new().unwrap();
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    let peer_id = Uuid::new_v4().to_string();
    let ticket = relay
        .request_discovery_pair(request(&relay, peer_id.clone()))
        .unwrap();
    let code = ticket.verification_code.as_ref().unwrap();
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|c| c.is_ascii_digit()));
    let state = relay.snapshot(OWNER_ID).unwrap();
    assert!(state.devices.is_empty());
    assert_eq!(
        state.pair_requests[0].verification_code,
        ticket.verification_code
    );
    assert_eq!(state.pair_requests[0].peer_id.as_ref(), Some(&peer_id));
    assert!(!serde_json::to_string(&state)
        .unwrap()
        .contains(&ticket.claim_token));
    assert!(relay.authenticate(code).is_err());
    let claim = ClaimRequest {
        id: ticket.id.clone(),
        claim_token: ticket.claim_token.clone(),
    };
    let pending = relay.claim_pair(claim.clone()).unwrap();
    assert_eq!(pending.status, "pending");
    assert!(pending.token.is_none());
    assert_eq!(
        relay
            .request_discovery_pair(request(&relay, peer_id))
            .unwrap_err()
            .code,
        "PAIR_ALREADY_PENDING"
    );
    relay.decide_pair(&ticket.id, true).unwrap();
    let approved = relay.claim_pair(claim.clone()).unwrap();
    let actor = relay
        .authenticate(approved.token.as_ref().unwrap())
        .unwrap();
    assert!(relay.snapshot(&actor).unwrap().pair_requests.is_empty());
    assert_eq!(approved.token, relay.claim_pair(claim).unwrap().token);
    relay.revoke_device(&actor).unwrap();
    assert!(relay
        .authenticate(approved.token.as_ref().unwrap())
        .is_err());
}

#[test]
fn discovery_checks_target_identity_and_does_not_reuse_approved_device_identity() {
    let dir = TempDir::new().unwrap();
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    let peer_id = Uuid::new_v4().to_string();
    let mut wrong = request(&relay, peer_id.clone());
    wrong.server_id = Uuid::new_v4().to_string();
    assert_eq!(
        relay.request_discovery_pair(wrong).unwrap_err().code,
        "SERVER_ID_CHANGED"
    );
    assert_eq!(
        relay
            .request_discovery_pair(request(&relay, "owner".into()))
            .unwrap_err()
            .code,
        "INVALID_PEER_ID"
    );
    let first = relay
        .request_discovery_pair(request(&relay, peer_id.clone()))
        .unwrap();
    relay.decide_pair(&first.id, true).unwrap();
    let second = relay
        .request_discovery_pair(request(&relay, peer_id))
        .unwrap();
    let claimed = relay
        .claim_pair(ClaimRequest {
            id: second.id,
            claim_token: second.claim_token,
        })
        .unwrap();
    assert_eq!(claimed.status, "pending");
    assert!(claimed.device.is_none());
    assert!(claimed.token.is_none());
    assert_eq!(relay.snapshot(OWNER_ID).unwrap().devices.len(), 1);
}

#[test]
fn discovery_reject_restart_and_queue_limits_preserve_v2_workspace() {
    let dir = TempDir::new().unwrap();
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    let ticket = relay
        .request_discovery_pair(request(&relay, Uuid::new_v4().to_string()))
        .unwrap();
    let before = relay.hello().server_id;
    drop(relay);
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    assert_eq!(relay.hello().server_id, before);
    assert_eq!(
        relay.snapshot(OWNER_ID).unwrap().pair_requests[0].verification_code,
        ticket.verification_code
    );
    relay.decide_pair(&ticket.id, false).unwrap();
    assert_eq!(
        relay
            .claim_pair(ClaimRequest {
                id: ticket.id,
                claim_token: ticket.claim_token
            })
            .unwrap()
            .status,
        "rejected"
    );
    for _ in 0..20 {
        relay
            .request_discovery_pair(request(&relay, Uuid::new_v4().to_string()))
            .unwrap();
    }
    assert_eq!(
        relay
            .request_discovery_pair(request(&relay, Uuid::new_v4().to_string()))
            .unwrap_err()
            .code,
        "PAIR_QUEUE_FULL"
    );
    assert!(relay.snapshot(OWNER_ID).unwrap().devices.is_empty());
}

#[test]
fn expired_or_forged_discovery_claim_never_creates_a_session() {
    let dir = TempDir::new().unwrap();
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    let ticket = relay
        .request_discovery_pair(request(&relay, Uuid::new_v4().to_string()))
        .unwrap();
    let forged = ClaimRequest {
        id: ticket.id.clone(),
        claim_token: "x".repeat(43),
    };
    assert_eq!(relay.claim_pair(forged).unwrap_err().code, "INVALID_CLAIM");
    let db = rusqlite::Connection::open(dir.path().join("relay-v2.sqlite")).unwrap();
    db.execute("UPDATE pairs SET expires_at=0 WHERE id=?", [&ticket.id])
        .unwrap();
    assert_eq!(
        relay.decide_pair(&ticket.id, true).unwrap_err().code,
        "PAIR_EXPIRED"
    );
    let expired = relay
        .claim_pair(ClaimRequest {
            id: ticket.id,
            claim_token: ticket.claim_token,
        })
        .unwrap();
    assert_eq!(expired.status, "expired");
    assert!(expired.token.is_none());
    assert!(relay.snapshot(OWNER_ID).unwrap().devices.is_empty());
}

#[test]
fn only_claim_holder_can_cancel_pending_pair_then_retry_without_losing_approved_sessions() {
    let dir = TempDir::new().unwrap();
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    let peer = Uuid::new_v4().to_string();
    let first = relay
        .request_discovery_pair(request(&relay, peer.clone()))
        .unwrap();
    assert_eq!(
        relay
            .cancel_pair(ClaimRequest {
                id: first.id.clone(),
                claim_token: "x".repeat(43)
            })
            .unwrap_err()
            .code,
        "INVALID_CLAIM"
    );
    let claim = ClaimRequest {
        id: first.id.clone(),
        claim_token: first.claim_token.clone(),
    };
    assert_eq!(relay.cancel_pair(claim.clone()).unwrap().status, "rejected");
    assert_eq!(relay.cancel_pair(claim.clone()).unwrap().status, "rejected");
    assert!(relay.claim_pair(claim).unwrap().token.is_none());
    let next = relay.request_discovery_pair(request(&relay, peer)).unwrap();
    relay.decide_pair(&next.id, true).unwrap();
    let claim = ClaimRequest {
        id: next.id,
        claim_token: next.claim_token,
    };
    assert_eq!(
        relay.cancel_pair(claim.clone()).unwrap_err().code,
        "PAIR_ALREADY_APPROVED"
    );
    let approved = relay.claim_pair(claim).unwrap();
    assert!(relay.authenticate(approved.token.as_ref().unwrap()).is_ok());
}
