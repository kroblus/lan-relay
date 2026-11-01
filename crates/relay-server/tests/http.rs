use relay_core::*;
use reqwest::{Client, StatusCode};
use std::{sync::Arc, time::Duration};
use tempfile::TempDir;
use uuid::Uuid;

async fn fixture() -> (
    TempDir,
    Arc<Relay>,
    relay_server::ServerHandle,
    Client,
    String,
    String,
) {
    let dir = TempDir::new().unwrap();
    let mut config = Config::new(dir.path());
    config.chunk_size = 64 * 1024;
    let core = Arc::new(Relay::open(config).unwrap());
    let server = relay_server::start(core.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let base = format!("http://{}", server.addr);
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let invite = core.create_invite().unwrap();
    let ticket: PairTicket = client
        .post(format!("{base}/api/v2/pair/requests"))
        .json(&NewPairRequest {
            name: "HTTP 测试手机".into(),
            code: None,
            token: Some(invite.token),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let claim_body = ClaimRequest {
        id: ticket.id.clone(),
        claim_token: ticket.claim_token.clone(),
    };
    let pending: PairClaim = client
        .post(format!("{base}/api/v2/pair/claim"))
        .json(&claim_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pending.status, "pending");
    core.decide_pair(&ticket.id, true).unwrap();
    let approved: PairClaim = client
        .post(format!("{base}/api/v2/pair/claim"))
        .json(&claim_body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    (dir, core, server, client, base, approved.token.unwrap())
}

#[tokio::test]
async fn native_pairing_unicode_and_origin_boundaries() {
    let (_dir, core, server, client, base, token) = fixture().await;
    assert_eq!(
        client
            .get(format!("{base}/api/v2/state"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(format!("{base}/api/v2/hello"))
            .header("Origin", "https://unrelated.example")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let input = NewText {
        text: "中文\n多行 😀\n\n原样保留".into(),
        project: "项目".into(),
        request_id: Uuid::new_v4().to_string(),
    };
    let first: Item = client
        .post(format!("{base}/api/v2/text"))
        .bearer_auth(&token)
        .json(&input)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let second: Item = client
        .post(format!("{base}/api/v2/text"))
        .bearer_auth(&token)
        .json(&input)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(first.text, input.text);
    let state: Snapshot = client
        .get(format!("{base}/api/v2/state"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(state.pair_requests.is_empty());
    let actor = core.authenticate(&token).unwrap();
    core.revoke_device(&actor).unwrap();
    assert_eq!(
        client
            .get(format!("{base}/api/v2/state"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    server.shutdown().await;
}

#[tokio::test]
async fn upload_retry_download_head_range_and_integrity() {
    let (_dir, core, server, client, base, token) = fixture().await;
    let bytes = (0..256 * 1024 + 17)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<_>>();
    let input = NewUpload {
        name: "中文原图.png".into(),
        size: bytes.len() as u64,
        mime: "image/png".into(),
        sha256: digest(&bytes),
        project: "HTTP".into(),
        build_id: None,
        request_id: Uuid::new_v4().to_string(),
    };
    let u: Upload = client
        .post(format!("{base}/api/v2/uploads"))
        .bearer_auth(&token)
        .json(&input)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let retry: Upload = client
        .post(format!("{base}/api/v2/uploads"))
        .bearer_auth(&token)
        .json(&input)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(u.id, retry.id);
    let target = format!("{base}/api/v2/uploads/{}", u.id);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + u.chunk_size as usize).min(bytes.len());
        let next: Upload = client
            .patch(&target)
            .bearer_auth(&token)
            .header("Content-Type", "application/octet-stream")
            .header("Upload-Offset", offset)
            .body(bytes[offset..end].to_vec())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(next.offset, end as u64);
        offset = end;
    }
    assert_eq!(
        client
            .patch(&target)
            .bearer_auth(&token)
            .header("Content-Type", "application/octet-stream")
            .header("Upload-Offset", "0")
            .body(vec![1])
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let item: Item = client
        .post(format!("{target}/complete"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let retry: Item = client
        .post(format!("{target}/complete"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(item.id, retry.id);
    let download = format!("{base}/api/v2/files/{}", item.id);
    let head = client
        .head(&download)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()["content-length"], bytes.len().to_string());
    assert!(head.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .contains("filename*=UTF-8''"));
    let etag = head.headers()["etag"].to_str().unwrap().to_owned();
    assert!(head.bytes().await.unwrap().is_empty());
    let full = client
        .get(&download)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(digest(&full), digest(&bytes));
    let range = client
        .get(&download)
        .bearer_auth(&token)
        .header("Range", "bytes=17-333")
        .header("If-Range", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(&range.bytes().await.unwrap()[..], &bytes[17..334]);
    let suffix = client
        .get(&download)
        .bearer_auth(&token)
        .header("Range", "bytes=-7")
        .send()
        .await
        .unwrap();
    assert_eq!(
        &suffix.bytes().await.unwrap()[..],
        &bytes[bytes.len() - 7..]
    );
    let ignored = client
        .get(&download)
        .bearer_auth(&token)
        .header("Range", "bytes=1-3")
        .header("If-Range", "\"wrong\"")
        .send()
        .await
        .unwrap();
    assert_eq!(ignored.status(), StatusCode::OK);
    assert_eq!(
        client
            .get(&download)
            .bearer_auth(&token)
            .header("Range", "bytes=9999999-")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::RANGE_NOT_SATISFIABLE
    );
    assert_eq!(
        client
            .delete(&target)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(core.snapshot(OWNER_ID).unwrap().items.len(), 1);
    server.shutdown().await;
}

#[tokio::test]
async fn truncated_http_body_never_advances_offset() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (_dir, core, server, client, base, token) = fixture().await;
    let input = NewUpload {
        name: "partial.bin".into(),
        size: 8,
        mime: "application/octet-stream".into(),
        sha256: digest(b"12345678"),
        project: String::new(),
        build_id: None,
        request_id: Uuid::new_v4().to_string(),
    };
    let upload: Upload = client
        .post(format!("{base}/api/v2/uploads"))
        .bearer_auth(&token)
        .json(&input)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut socket = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    let header=format!("PATCH /api/v2/uploads/{} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/octet-stream\r\nUpload-Offset: 0\r\nContent-Length: 8\r\nConnection: close\r\n\r\n123",upload.id,server.addr,token);
    socket.write_all(header.as_bytes()).await.unwrap();
    socket.shutdown().await.unwrap();
    let mut ignored = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut ignored)).await;
    let actor = core.authenticate(&token).unwrap();
    assert_eq!(core.get_upload(&actor, &upload.id).unwrap().offset, 0);
    server.shutdown().await;
}

#[tokio::test]
async fn slow_request_body_cannot_outlive_session_expiry_or_revocation() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for expire in [false, true] {
        let (dir, core, server, _client, _base, token) = fixture().await;
        let actor = core.authenticate(&token).unwrap();
        let upload = core
            .create_upload(
                &actor,
                NewUpload {
                    name: "slow.bin".into(),
                    size: 8,
                    mime: "application/octet-stream".into(),
                    sha256: digest(b"12345678"),
                    project: String::new(),
                    build_id: None,
                    request_id: Uuid::new_v4().to_string(),
                },
            )
            .unwrap();
        let expires_at = now_ms() + 2000;
        if expire {
            let db = rusqlite::Connection::open(dir.path().join("relay-v2.sqlite")).unwrap();
            db.execute(
                "UPDATE sessions SET expires_at=? WHERE token_hash=?",
                rusqlite::params![expires_at, digest(token.as_bytes())],
            )
            .unwrap();
        }
        let mut socket = tokio::net::TcpStream::connect(server.addr).await.unwrap();
        let header = format!(
            "PATCH /api/v2/uploads/{} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/octet-stream\r\nUpload-Offset: 0\r\nContent-Length: 8\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
            upload.id, server.addr, token
        );
        socket.write_all(header.as_bytes()).await.unwrap();
        // Hyper emits 100 Continue when the authenticated handler starts polling
        // the body. This is a barrier, rather than a timing guess about auth.
        let mut provisional = vec![0_u8; b"HTTP/1.1 100 Continue\r\n\r\n".len()];
        tokio::time::timeout(Duration::from_secs(3), socket.read_exact(&mut provisional))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(provisional, b"HTTP/1.1 100 Continue\r\n\r\n");
        if expire {
            let remaining = (expires_at - now_ms()).max(0) as u64 + 20;
            tokio::time::sleep(Duration::from_millis(remaining)).await;
        } else {
            core.revoke_device(&actor).unwrap();
        }
        socket.write_all(b"12345678").await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), socket.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 401"));
        assert_eq!(core.get_upload(OWNER_ID, &upload.id).unwrap().offset, 0);
        assert_eq!(
            std::fs::metadata(dir.path().join("temp").join(format!("{}.part", upload.id)))
                .unwrap()
                .len(),
            0
        );
        server.shutdown().await;
    }
}

#[tokio::test]
async fn feedback_export_is_authenticated_attachment() {
    let (_dir, core, server, client, base, token) = fixture().await;
    let u = core
        .create_upload(
            OWNER_ID,
            NewUpload {
                name: "test.apk".into(),
                size: 3,
                mime: "application/vnd.android.package-archive".into(),
                sha256: digest(b"apk"),
                project: "测试".into(),
                build_id: None,
                request_id: Uuid::new_v4().to_string(),
            },
        )
        .unwrap();
    core.append_upload(OWNER_ID, &u.id, 0, b"apk").unwrap();
    let build = core.complete_upload(OWNER_ID, &u.id).unwrap();
    let item: Item = client
        .post(format!("{base}/api/v2/feedback"))
        .bearer_auth(&token)
        .json(&NewFeedback {
            build_id: build.id.clone(),
            text: "真实安装未执行".into(),
            project: String::new(),
            attachment_ids: vec![],
            request_id: Uuid::new_v4().to_string(),
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let url = format!("{base}/api/v2/feedback/{}/export", item.id);
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let head = client.head(&url).bearer_auth(&token).send().await.unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert!(head.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment;"));
    let text = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.contains(&build.sha256));
    assert!(text.contains("真实安装未执行"));
    server.shutdown().await;
}

#[tokio::test]
async fn phone_can_mark_desktop_apk_status_without_editing_other_metadata() {
    let (_dir, core, server, client, base, token) = fixture().await;
    let upload = core
        .create_upload(
            OWNER_ID,
            NewUpload {
                name: "manual-status.apk".into(),
                size: 3,
                mime: "application/vnd.android.package-archive".into(),
                sha256: digest(b"apk"),
                project: "桌面构建".into(),
                build_id: None,
                request_id: Uuid::new_v4().to_string(),
            },
        )
        .unwrap();
    core.append_upload(OWNER_ID, &upload.id, 0, b"apk").unwrap();
    let build = core.complete_upload(OWNER_ID, &upload.id).unwrap();
    assert_eq!(build.test_status, "unverified");
    let url = format!("{base}/api/v2/items/{}", build.id);
    assert_eq!(
        client
            .patch(&url)
            .json(&serde_json::json!({"test_status":"installed"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..2 {
        let marked: Item = client
            .patch(&url)
            .bearer_auth(&token)
            .json(&serde_json::json!({"test_status":"installed"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(marked.test_status, "installed");
        assert_eq!(marked.project, build.project);
        assert_eq!(marked.sha256, build.sha256);
        assert_eq!(marked.device_id, OWNER_ID);
    }
    assert_eq!(
        client
            .patch(&url)
            .bearer_auth(&token)
            .json(&serde_json::json!({"test_status":"passed","project":"unauthorized"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let item = core.file_info(OWNER_ID, &build.id).unwrap().item;
    assert_eq!(item.test_status, "installed");
    assert_eq!(item.project, build.project);
    core.revoke_device(&core.authenticate(&token).unwrap())
        .unwrap();
    assert_eq!(
        client
            .patch(&url)
            .bearer_auth(&token)
            .json(&serde_json::json!({"test_status":"passed"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    server.shutdown().await;
}

#[tokio::test]
async fn revocation_interrupts_an_active_download() {
    let (dir, core, server, client, base, token) = fixture().await;
    let source = dir.path().join("large-source.bin");
    std::fs::File::create(&source)
        .unwrap()
        .set_len(32 * 1024 * 1024)
        .unwrap();
    let clone = core.clone();
    let item = tokio::task::spawn_blocking(move || clone.import_file(&source, "撤销测试").unwrap())
        .await
        .unwrap();
    let mut response = client
        .get(format!("{base}/api/v2/files/{}", item.id))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    core.revoke_device(&core.authenticate(&token).unwrap())
        .unwrap();
    let mut received = 0usize;
    let mut interrupted = false;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => received += chunk.len(),
            Ok(None) => break,
            Err(_) => {
                interrupted = true;
                break;
            }
        }
    }
    assert!(
        interrupted,
        "revoked download unexpectedly completed {received} bytes"
    );
    assert!(received < item.size as usize);
    server.shutdown().await;
}

#[tokio::test]
async fn malformed_json_is_a_structured_protocol_error() {
    let (_dir, _core, server, client, base, token) = fixture().await;
    let response = client
        .post(format!("{base}/api/v2/text"))
        .bearer_auth(&token)
        .header("Content-Type", "application/json")
        .body("{bad")
        .send()
        .await
        .unwrap();
    assert!(response.status().is_client_error());
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "INVALID_REQUEST");
    server.shutdown().await;
}

#[tokio::test]
async fn peer_session_and_partial_upload_resume_after_http_restart() {
    let (dir, core, server, client, base, token) = fixture().await;
    let input = NewUpload {
        name: "restart.bin".into(),
        size: 8,
        mime: "application/octet-stream".into(),
        sha256: digest(b"12345678"),
        project: String::new(),
        build_id: None,
        request_id: Uuid::new_v4().to_string(),
    };
    let upload: Upload = client
        .post(format!("{base}/api/v2/uploads"))
        .bearer_auth(&token)
        .json(&input)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    client
        .patch(format!("{base}/api/v2/uploads/{}", upload.id))
        .bearer_auth(&token)
        .header("Content-Type", "application/octet-stream")
        .header("Upload-Offset", 0)
        .body("1234")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let server_id = core.hello().server_id;
    server.shutdown().await;
    drop(core);
    let core = Arc::new(Relay::open(Config::new(dir.path())).unwrap());
    assert_eq!(core.hello().server_id, server_id);
    let server = relay_server::start(core.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let base = format!("http://{}", server.addr);
    let target = format!("{base}/api/v2/uploads/{}", upload.id);
    let resumed: Upload = client
        .get(&target)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resumed.offset, 4);
    assert_eq!(resumed.chunk_size, upload.chunk_size);
    assert!(resumed.chunk_size < core.hello().chunk_size);
    client
        .patch(&target)
        .bearer_auth(&token)
        .header("Content-Type", "application/octet-stream")
        .header("Upload-Offset", 4)
        .body("5678")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let item: Item = client
        .post(format!("{target}/complete"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(item.sha256, digest(b"12345678"));
    server.shutdown().await;
}
