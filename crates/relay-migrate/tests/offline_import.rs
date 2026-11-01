use relay_core::{Config, Relay, OWNER_ID};
use relay_migrate::import_legacy;
use rusqlite::{params, Connection};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};
use tempfile::TempDir;
use uuid::Uuid;

fn fixture() -> (TempDir, String, String) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("old");
    fs::create_dir(&source).unwrap();
    fs::create_dir(source.join("files")).unwrap();
    let db = Connection::open(source.join("relay.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE items(id TEXT,kind TEXT,created_at TEXT,device_name TEXT,project TEXT,data TEXT); CREATE TABLE uploads(status TEXT);").unwrap();
    let storage = Uuid::new_v4().to_string();
    let build = Uuid::new_v4().to_string();
    let attachment = Uuid::new_v4().to_string();
    let bytes = b"synthetic fixture: not an installable APK";
    fs::write(source.join("files").join(format!("{storage}.bin")), bytes).unwrap();
    let data = json!({"name":"构建.apk","mime":"application/vnd.android.package-archive","size":bytes.len(),
        "sha256":hex::encode(Sha256::digest(bytes)),"_storageId":storage,"note":"原备注","testStatus":"not-tested"});
    row(&db, &build, "file", data);
    let image_storage = Uuid::new_v4().to_string();
    let image = [1u8, 2, 3, 4];
    fs::write(
        source.join("files").join(format!("{image_storage}.bin")),
        image,
    )
    .unwrap();
    row(
        &db,
        &attachment,
        "file",
        json!({"name":"屏幕.png","mime":"image/png","size":4,
        "sha256":hex::encode(Sha256::digest(image)),"_storageId":image_storage,"buildId":build}),
    );
    row(
        &db,
        "text-one",
        "text",
        json!({"text":"中文第一行\n第二行 😀"}),
    );
    row(
        &db,
        "feedback-one",
        "feedback",
        json!({"text":"按钮遮挡","buildId":build,"attachmentIds":[attachment]}),
    );
    db.execute("INSERT INTO uploads(status) VALUES ('uploading')", [])
        .unwrap();
    drop(db);
    (root, storage, build)
}

fn row(db: &Connection, id: &str, kind: &str, data: serde_json::Value) {
    db.execute(
        "INSERT INTO items VALUES (?,?, '2026-11-01T00:00:00.000Z','我的手机','康养工作簿',?)",
        params![id, kind, data.to_string()],
    )
    .unwrap();
}

fn digest(path: &Path) -> Vec<u8> {
    Sha256::digest(fs::read(path).unwrap()).to_vec()
}

#[test]
fn imports_original_bytes_and_references_without_mutating_source() {
    let (root, storage, build) = fixture();
    let source = root.path().join("old");
    let target = root.path().join("new");
    let before = digest(&source.join("relay.sqlite"));
    let file_before = digest(&source.join("files").join(format!("{storage}.bin")));
    let report = import_legacy(&source, &target, false).unwrap();
    assert_eq!((report.files, report.texts, report.feedback), (2, 1, 1));
    assert_eq!(report.incomplete_uploads_not_imported, 1);
    assert_eq!(before, digest(&source.join("relay.sqlite")));
    assert_eq!(
        file_before,
        digest(&source.join("files").join(format!("{storage}.bin")))
    );
    let relay = Relay::open(Config::new(&target)).unwrap();
    let snapshot = relay.snapshot(OWNER_ID).unwrap();
    assert_eq!(snapshot.items.len(), 4);
    let mapped_build = &report
        .records
        .iter()
        .find(|r| r.old_id == build)
        .unwrap()
        .new_id;
    let feedback = snapshot
        .items
        .iter()
        .find(|i| i.kind == "feedback")
        .unwrap();
    assert_eq!(feedback.build_id.as_ref(), Some(mapped_build));
    assert_eq!(feedback.attachment_ids.len(), 1);
    assert!(snapshot
        .items
        .iter()
        .any(|i| i.text == "中文第一行\n第二行 😀"));
    assert!(snapshot.devices.is_empty());
    assert!(target.join("legacy-import-report.json").is_file());
}

#[test]
fn dry_run_checks_hashes_and_creates_no_destination() {
    let (root, storage, _) = fixture();
    let source = root.path().join("old");
    let target = root.path().join("new");
    assert!(import_legacy(&source, &target, true).unwrap().dry_run);
    assert!(!target.exists());
    fs::write(
        source.join("files").join(format!("{storage}.bin")),
        b"corrupted",
    )
    .unwrap();
    assert!(import_legacy(&source, &target, false).is_err());
    assert!(!target.exists());
}

#[test]
fn rejects_active_old_service_existing_target_and_unsafe_storage_id() {
    let (root, _, _) = fixture();
    let source = root.path().join("old");
    let target = root.path().join("new");
    fs::write(source.join("process-lock"), "fixture").unwrap();
    assert!(import_legacy(&source, &target, false)
        .unwrap_err()
        .to_string()
        .contains("process-lock"));
    fs::remove_file(source.join("process-lock")).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep.txt"), "must remain").unwrap();
    assert!(import_legacy(&source, &target, false).is_err());
    assert_eq!(
        fs::read_to_string(target.join("keep.txt")).unwrap(),
        "must remain"
    );
    let db = Connection::open(source.join("relay.sqlite")).unwrap();
    db.execute(
        "UPDATE items SET data=? WHERE kind='file'",
        [json!({"_storageId":"../../outside"}).to_string()],
    )
    .unwrap();
    drop(db);
    assert!(import_legacy(&source, &root.path().join("another"), false).is_err());
}

#[test]
fn unlinked_feedback_preserves_text_and_metadata_in_report() {
    let (root, _, _) = fixture();
    let source = root.path().join("old");
    let db = Connection::open(source.join("relay.sqlite")).unwrap();
    row(
        &db,
        "unlinked",
        "feedback",
        json!({"text":"没有关联构建的反馈","attachmentIds":[]}),
    );
    drop(db);
    let target = root.path().join("new");
    let report = import_legacy(&source, &target, false).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("unlinked")));
    assert!(report
        .records
        .iter()
        .any(|r| r.old_id == "unlinked" && r.original_kind == "feedback"));
    let relay = Relay::open(Config::new(target)).unwrap();
    assert!(relay
        .snapshot(OWNER_ID)
        .unwrap()
        .items
        .iter()
        .any(|i| i.text == "没有关联构建的反馈"));
}
