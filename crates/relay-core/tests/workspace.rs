use relay_core::*;
use std::{fs, io::Write};
use tempfile::TempDir;
use uuid::Uuid;

fn workspace() -> (TempDir, Relay) {
    let dir = TempDir::new().unwrap();
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    (dir, relay)
}
fn request() -> String {
    Uuid::new_v4().to_string()
}
fn paired(relay: &Relay, name: &str) -> (String, String) {
    let invite = relay.create_invite().unwrap();
    let ticket = relay
        .request_pair(NewPairRequest {
            name: name.into(),
            code: Some(invite.code),
            token: None,
        })
        .unwrap();
    assert_eq!(
        relay
            .claim_pair(ClaimRequest {
                id: ticket.id.clone(),
                claim_token: ticket.claim_token.clone()
            })
            .unwrap()
            .status,
        "pending"
    );
    relay.decide_pair(&ticket.id, true).unwrap();
    let claim = relay
        .claim_pair(ClaimRequest {
            id: ticket.id.clone(),
            claim_token: ticket.claim_token.clone(),
        })
        .unwrap();
    let retry = relay
        .claim_pair(ClaimRequest {
            id: ticket.id,
            claim_token: ticket.claim_token,
        })
        .unwrap();
    assert_eq!(claim.token, retry.token);
    (claim.device.unwrap().id, claim.token.unwrap())
}
fn upload(relay: &Relay, actor: &str, name: &str, bytes: &[u8]) -> Upload {
    relay
        .create_upload(
            actor,
            NewUpload {
                name: name.into(),
                size: bytes.len() as u64,
                mime: "application/octet-stream".into(),
                sha256: digest(bytes),
                project: "康养工作簿".into(),
                build_id: None,
                request_id: request(),
            },
        )
        .unwrap()
}
fn complete(relay: &Relay, actor: &str, name: &str, bytes: &[u8]) -> Item {
    let upload = upload(relay, actor, name, bytes);
    for chunk in bytes.chunks(upload.chunk_size as usize) {
        let state = relay.get_upload(actor, &upload.id).unwrap();
        relay
            .append_upload(actor, &upload.id, state.offset, chunk)
            .unwrap();
    }
    relay.complete_upload(actor, &upload.id).unwrap()
}

#[test]
fn pairing_requires_approval_and_revocation_invalidates_the_token() {
    let (_dir, relay) = workspace();
    let invite = relay.create_invite().unwrap();
    let ticket = relay
        .request_pair(NewPairRequest {
            name: "我的手机".into(),
            code: None,
            token: Some(invite.token.clone()),
        })
        .unwrap();
    assert!(relay
        .request_pair(NewPairRequest {
            name: "再试一次".into(),
            code: Some(invite.code),
            token: None
        })
        .is_err());
    assert!(relay
        .claim_pair(ClaimRequest {
            id: ticket.id.clone(),
            claim_token: "A".repeat(43)
        })
        .is_err());
    assert_eq!(relay.snapshot(OWNER_ID).unwrap().pair_requests.len(), 1);
    relay.decide_pair(&ticket.id, true).unwrap();
    let c = relay
        .claim_pair(ClaimRequest {
            id: ticket.id,
            claim_token: ticket.claim_token,
        })
        .unwrap();
    let token = c.token.unwrap();
    let actor = c.device.unwrap().id;
    assert_eq!(relay.authenticate(&token).unwrap(), actor);
    assert!(relay.snapshot(&actor).unwrap().pair_requests.is_empty());
    assert!(relay.authenticate("owner").is_err());
    relay.revoke_device(&actor).unwrap();
    assert!(relay.authenticate(&token).is_err());
    assert!(relay.snapshot(&actor).is_err());
}

#[test]
fn preserve_unicode_and_idempotency_without_mutating_other_devices_items() {
    let (_dir, relay) = workspace();
    let (a, _) = paired(&relay, "手机 A");
    let (b, _) = paired(&relay, "手机 B");
    let input = NewText {
        text: "中文第一行\n\n第二行 😀\r\n尾行".into(),
        project: "项目一".into(),
        request_id: request(),
    };
    let first = relay.create_text(&a, input.clone()).unwrap();
    let retry = relay.create_text(&a, input.clone()).unwrap();
    assert_eq!(first.id, retry.id);
    assert_eq!(first.text, input.text);
    let mut changed = input;
    changed.text = "另一个请求".into();
    assert_eq!(relay.create_text(&a, changed).unwrap_err().status, 409);
    assert_eq!(
        relay
            .patch_item(
                &b,
                &first.id,
                PatchItem {
                    note: Some("no".into()),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .status,
        403
    );
}

#[test]
fn approved_phone_can_record_only_manual_status_for_a_shared_apk() {
    let (_dir, relay) = workspace();
    let (phone, _) = paired(&relay, "测试手机");
    let build = complete(&relay, OWNER_ID, "手动测试.APK", b"synthetic fixture");
    assert_eq!(build.test_status, "unverified");
    let status = PatchItem {
        test_status: Some("installed".into()),
        ..Default::default()
    };
    let marked = relay.patch_item(&phone, &build.id, status.clone()).unwrap();
    assert_eq!(marked.test_status, "installed");
    assert_eq!(marked.device_id, OWNER_ID);
    assert_eq!(marked.sha256, build.sha256);
    let retry = relay.patch_item(&phone, &build.id, status).unwrap();
    assert_eq!(retry.id, marked.id);
    assert_eq!(retry.test_status, marked.test_status);
    for patch in [
        PatchItem {
            project: Some("不能改动项目".into()),
            test_status: Some("passed".into()),
            ..Default::default()
        },
        PatchItem {
            note: Some("不能改动备注".into()),
            ..Default::default()
        },
    ] {
        assert_eq!(
            relay
                .patch_item(&phone, &build.id, patch)
                .unwrap_err()
                .status,
            403
        );
    }
    assert_eq!(
        relay.file_info(&phone, &build.id).unwrap().item.test_status,
        "installed"
    );
    let image = complete(&relay, OWNER_ID, "screenshot.png", b"image");
    assert_eq!(
        relay
            .patch_item(
                &phone,
                &image.id,
                PatchItem {
                    test_status: Some("passed".into()),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .status,
        403
    );
    relay.revoke_device(&phone).unwrap();
    assert!(relay
        .patch_item(
            &phone,
            &build.id,
            PatchItem {
                test_status: Some("passed".into()),
                ..Default::default()
            }
        )
        .is_err());
}

#[test]
fn durable_offset_truncates_uncommitted_tail_after_restart() {
    let (dir, relay) = workspace();
    let bytes = b"first-second-third";
    let u = upload(&relay, OWNER_ID, "原图.png", bytes);
    relay
        .append_upload(OWNER_ID, &u.id, 0, &bytes[..6])
        .unwrap();
    assert_eq!(
        relay
            .append_upload(OWNER_ID, &u.id, 0, &bytes[..6])
            .unwrap_err()
            .code,
        "OFFSET_MISMATCH"
    );
    drop(relay);
    let temp = dir.path().join("temp").join(format!("{}.part", u.id));
    fs::OpenOptions::new()
        .append(true)
        .open(&temp)
        .unwrap()
        .write_all(b"uncommitted-tail")
        .unwrap();
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    assert_eq!(fs::metadata(&temp).unwrap().len(), 6);
    assert_eq!(relay.get_upload(OWNER_ID, &u.id).unwrap().offset, 6);
    relay
        .append_upload(OWNER_ID, &u.id, 6, &bytes[6..])
        .unwrap();
    let item = relay.complete_upload(OWNER_ID, &u.id).unwrap();
    assert_eq!(relay.complete_upload(OWNER_ID, &u.id).unwrap().id, item.id);
    assert_eq!(
        fs::read(relay.file_info(OWNER_ID, &item.id).unwrap().path).unwrap(),
        bytes
    );
    assert_eq!(
        relay.cancel_upload(OWNER_ID, &u.id).unwrap_err().code,
        "UPLOAD_COMPLETE"
    );
}

#[test]
fn missing_confirmed_bytes_fail_without_publishing_or_reserving_space() {
    let (dir, relay) = workspace();
    let u = upload(&relay, OWNER_ID, "file.bin", b"12345678");
    relay.append_upload(OWNER_ID, &u.id, 0, b"1234").unwrap();
    drop(relay);
    let temp = dir.path().join("temp").join(format!("{}.part", u.id));
    fs::write(&temp, b"12").unwrap();
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    let state = relay.snapshot(OWNER_ID).unwrap();
    assert_eq!(state.uploads[0].status, "failed");
    assert_eq!(state.used_bytes, 0);
    assert!(state.items.is_empty());
}

#[test]
fn recovery_publishes_file_renamed_before_database_commit() {
    let (dir, relay) = workspace();
    let bytes = b"confirmed bytes";
    let u = upload(&relay, OWNER_ID, "video.mp4", bytes);
    relay.append_upload(OWNER_ID, &u.id, 0, bytes).unwrap();
    drop(relay);
    fs::rename(
        dir.path().join("temp").join(format!("{}.part", u.id)),
        dir.path().join("files").join(format!("{}.bin", u.id)),
    )
    .unwrap();
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    let item = relay.complete_upload(OWNER_ID, &u.id).unwrap();
    assert_eq!(item.sha256, digest(bytes));
    assert_eq!(relay.snapshot(OWNER_ID).unwrap().items.len(), 1);
}

#[test]
fn integrity_failure_releases_quota_and_never_publishes() {
    let (_dir, relay) = workspace();
    let u = upload(&relay, OWNER_ID, "bad.apk", b"correct");
    relay.append_upload(OWNER_ID, &u.id, 0, b"WRONG!!").unwrap();
    assert_eq!(
        relay.complete_upload(OWNER_ID, &u.id).unwrap_err().code,
        "HASH_MISMATCH"
    );
    let snapshot = relay.snapshot(OWNER_ID).unwrap();
    assert!(snapshot.items.is_empty());
    assert_eq!(snapshot.used_bytes, 0);
}

#[test]
fn quotas_count_reservations_and_tasks_are_private() {
    let dir = TempDir::new().unwrap();
    let mut config = Config::new(dir.path());
    config.max_file_bytes = 10;
    config.max_storage_bytes = 10;
    let relay = Relay::open(config).unwrap();
    let (a, _) = paired(&relay, "A");
    let (b, _) = paired(&relay, "B");
    let u = upload(&relay, &a, "a.bin", b"12345678");
    assert_eq!(relay.get_upload(&b, &u.id).unwrap_err().status, 403);
    let input = NewUpload {
        name: "b.bin".into(),
        size: 3,
        mime: "application/octet-stream".into(),
        sha256: digest(b"123"),
        project: String::new(),
        build_id: None,
        request_id: request(),
    };
    assert_eq!(
        relay.create_upload(&b, input.clone()).unwrap_err().code,
        "QUOTA_EXCEEDED"
    );
    relay.cancel_upload(&a, &u.id).unwrap();
    assert!(relay.create_upload(&b, input).is_ok());
}

#[test]
fn feedback_exports_exact_build_hash_and_validated_attachments() {
    let (_dir, relay) = workspace();
    let build = complete(&relay, OWNER_ID, "demo.apk", b"synthetic, not installable");
    let image = complete(&relay, OWNER_ID, "截图.png", b"image fixture");
    assert_eq!(build.apk.as_ref().unwrap().status, "unavailable");
    let feedback = relay
        .create_feedback(
            OWNER_ID,
            NewFeedback {
                build_id: build.id.clone(),
                text: "步骤一\n实际：按钮没有响应".into(),
                project: String::new(),
                attachment_ids: vec![image.id.clone()],
                request_id: request(),
            },
        )
        .unwrap();
    let md = relay.export_feedback(OWNER_ID, &feedback.id).unwrap();
    assert!(md.contains(&build.sha256));
    assert!(md.contains(&image.sha256));
    assert!(md.contains("NOT RUN"));
    assert!(!md.contains("server-key"));
    assert!(relay
        .create_feedback(
            OWNER_ID,
            NewFeedback {
                build_id: image.id,
                text: "bad build".into(),
                project: String::new(),
                attachment_ids: vec![],
                request_id: request()
            }
        )
        .is_err());
}

#[test]
fn pagination_covers_identical_timestamps_and_exclusive_workspace() {
    let (dir, relay) = workspace();
    for i in 0..23 {
        relay
            .create_text(
                OWNER_ID,
                NewText {
                    text: format!("记录 {i}"),
                    project: String::new(),
                    request_id: request(),
                },
            )
            .unwrap();
    }
    let db = rusqlite::Connection::open(dir.path().join("relay-v2.sqlite")).unwrap();
    db.execute(
        "UPDATE items SET created_at=1000,body=json_set(body,'$.created_at',1000)",
        [],
    )
    .unwrap();
    drop(db);
    let mut before = None;
    let mut ids = std::collections::HashSet::new();
    loop {
        let page = relay.list_items(OWNER_ID, before.as_deref(), 5).unwrap();
        for item in page.items {
            assert!(ids.insert(item.id));
        }
        before = page.next_cursor;
        if before.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 23);
    assert!(Relay::open(Config::new(dir.path())).is_err());
}

#[test]
fn legacy_directory_is_left_untouched_and_empty_file_can_complete() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("relay.sqlite"), b"legacy").unwrap();
    assert!(Relay::open(Config::new(dir.path())).is_err());
    assert_eq!(
        fs::read(dir.path().join("relay.sqlite")).unwrap(),
        b"legacy"
    );
    let (_dir, relay) = workspace();
    let item = complete(&relay, OWNER_ID, "empty.txt", b"");
    assert_eq!(item.size, 0);
    assert_eq!(item.sha256, digest(b""));
}

#[test]
fn future_schema_is_rejected_without_downgrading() {
    let (dir, relay) = workspace();
    drop(relay);
    let path = dir.path().join("relay-v2.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    assert!(Relay::open(Config::new(dir.path())).is_err());
    let db = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        99
    );
}

#[test]
fn upload_request_id_cannot_be_reused_for_a_different_file() {
    let (_dir, relay) = workspace();
    let mut input = NewUpload {
        name: "a.bin".into(),
        size: 3,
        mime: "application/octet-stream".into(),
        sha256: digest(b"abc"),
        project: String::new(),
        build_id: None,
        request_id: request(),
    };
    let first = relay.create_upload(OWNER_ID, input.clone()).unwrap();
    assert_eq!(
        relay.create_upload(OWNER_ID, input.clone()).unwrap().id,
        first.id
    );
    input.sha256 = digest(b"xyz");
    assert_eq!(
        relay.create_upload(OWNER_ID, input).unwrap_err().code,
        "REQUEST_REUSED"
    );
}

#[test]
fn native_import_resumes_with_stable_request_id_and_rejects_changed_source() {
    let (dir, relay) = workspace();
    let source = dir.path().join("source.bin");
    let bytes = b"abcdefghi";
    fs::write(&source, bytes).unwrap();
    let rid = request();
    let task = relay
        .create_upload(
            OWNER_ID,
            NewUpload {
                name: "source.bin".into(),
                size: bytes.len() as u64,
                mime: "application/octet-stream".into(),
                sha256: digest(bytes),
                project: "same project".into(),
                build_id: None,
                request_id: rid.clone(),
            },
        )
        .unwrap();
    relay
        .append_upload(OWNER_ID, &task.id, 0, &bytes[..3])
        .unwrap();
    drop(relay);
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    let item = relay
        .import_file_with_request_id(&source, "same project", &rid, |_, _| {})
        .unwrap();
    assert_eq!(item.id, task.id);
    assert_eq!(
        relay
            .import_file_with_request_id(&source, "same project", &rid, |_, _| {})
            .unwrap()
            .id,
        item.id
    );
    fs::write(&source, b"different").unwrap();
    assert_eq!(
        relay
            .import_file_with_request_id(&source, "same project", &rid, |_, _| {})
            .unwrap_err()
            .code,
        "REQUEST_REUSED"
    );
}

#[cfg(unix)]
#[test]
fn new_workspace_storage_is_private_to_its_unix_user() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("new-v2");
    let relay = Relay::open(Config::new(&path)).unwrap();
    for p in [&path, &path.join("temp"), &path.join("files")] {
        assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o700);
    }
    for name in ["relay-v2.sqlite", "server-key", "workspace.lock"] {
        assert_eq!(
            fs::metadata(path.join(name)).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let u = upload(&relay, OWNER_ID, "private.bin", b"private");
    assert_eq!(
        fs::metadata(path.join("temp").join(format!("{}.part", u.id)))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
