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

#[test]
fn dry_run_and_import_agree_on_invalid_feedback_types_and_reasons() {
    for case in 0..7 {
        let (root, storage, build) = fixture();
        let source = root.path().join("old");
        let target = root.path().join("new");
        let db = Connection::open(source.join("relay.sqlite")).unwrap();
        let image: String = db
            .query_row(
                "SELECT id FROM items WHERE kind='file' AND json_extract(data,'$.name')='屏幕.png'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let (metadata, reason) = match case {
            0 => (
                json!({"text":"保留反馈\n第二行","buildId":image,"attachmentIds":[]}),
                "构建引用不是 APK 文件",
            ),
            1 => (
                json!({"text":"保留反馈\n第二行","buildId":build,"attachmentIds":["text-one"]}),
                "附件引用不是文件",
            ),
            2 => (
                json!({"text":"保留反馈\n第二行","buildId":build,"attachmentIds":["missing-attachment"]}),
                "附件引用缺失：missing-attachment",
            ),
            3 => (
                json!({"text":"保留反馈\n第二行","buildId":build,"attachmentIds":[42]}),
                "第 1 个附件引用格式无效",
            ),
            4 => (
                json!({"text":"保留反馈\n第二行","buildId":build,"attachmentIds":vec![image;21]}),
                "附件数量 21 超过当前上限 20",
            ),
            5 => (
                json!({"text":"保留反馈\n第二行","buildId":"missing-build","attachmentIds":[]}),
                "构建引用缺失：missing-build",
            ),
            _ => (
                json!({"text":"保留反馈\n第二行","buildId":build,"attachmentIds":"not-an-array"}),
                "附件列表格式无效",
            ),
        };
        db.execute(
            "UPDATE items SET data=? WHERE id='feedback-one'",
            [metadata.to_string()],
        )
        .unwrap();
        drop(db);
        let before_db = digest(&source.join("relay.sqlite"));
        let original_file = source.join("files").join(format!("{storage}.bin"));
        let before_file = digest(&original_file);
        let dry = import_legacy(&source, &target, true).unwrap();
        assert!(!target.exists());
        let actual = import_legacy(&source, &target, false).unwrap();
        assert_eq!((dry.files, dry.texts, dry.feedback), (2, 2, 0));
        assert_eq!(
            (actual.files, actual.texts, actual.feedback),
            (dry.files, dry.texts, dry.feedback)
        );
        assert_eq!(dry.feedback_converted_to_text, 1);
        assert_eq!(actual.feedback_converted_to_text, 1);
        assert_eq!(dry.warnings, actual.warnings);
        assert!(actual.warnings.iter().any(|w| w.contains(reason)));
        let record = actual
            .records
            .iter()
            .find(|r| r.old_id == "feedback-one")
            .unwrap();
        assert_eq!(record.original_kind, "feedback");
        assert_eq!(record.original_metadata, metadata);
        let relay = Relay::open(Config::new(&target)).unwrap();
        let snapshot = relay.snapshot(OWNER_ID).unwrap();
        let converted = snapshot
            .items
            .iter()
            .find(|item| item.id == record.new_id)
            .unwrap();
        assert_eq!(converted.kind, "text");
        assert_eq!(converted.text, "保留反馈\n第二行");
        assert_eq!(before_db, digest(&source.join("relay.sqlite")));
        assert_eq!(before_file, digest(&original_file));
    }
}

#[test]
fn invalid_file_build_reference_and_cycles_fail_before_import() {
    for cyclic in [false, true] {
        let (root, _, build) = fixture();
        let source = root.path().join("old");
        let target = root.path().join("new");
        let db = Connection::open(source.join("relay.sqlite")).unwrap();
        let reference = if cyclic { build.as_str() } else { "text-one" };
        db.execute(
            "UPDATE items SET data=json_set(data,'$.buildId',?) WHERE id=?",
            params![reference, build],
        )
        .unwrap();
        drop(db);
        for dry_run in [true, false] {
            let error = import_legacy(&source, &target, dry_run)
                .unwrap_err()
                .to_string();
            assert!(error.contains(if cyclic { "循环" } else { "不是 APK" }));
            assert!(!target.exists());
        }
    }
}

#[cfg(unix)]
fn link_directory(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn link_directory(target: &Path, link: &Path) {
    let result = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "junction fixture failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
#[cfg(any(unix, windows))]
fn whole_files_directory_cannot_escape_source_root_through_symlink_or_junction() {
    let (root, storage, _) = fixture();
    let source = root.path().join("old");
    let target = root.path().join("new");
    let outside = root.path().join("outside-files");
    fs::rename(source.join("files"), &outside).unwrap();
    link_directory(&outside, &source.join("files"));
    let outside_file = outside.join(format!("{storage}.bin"));
    let before_file = digest(&outside_file);
    let before_db = digest(&source.join("relay.sqlite"));
    for dry_run in [true, false] {
        let error = import_legacy(&source, &target, dry_run)
            .unwrap_err()
            .to_string();
        assert!(error.contains("旧 files 目录越过旧数据根目录"));
        assert!(!target.exists());
    }
    assert_eq!(before_file, digest(&outside_file));
    assert_eq!(before_db, digest(&source.join("relay.sqlite")));
}

#[test]
#[cfg(any(unix, windows))]
fn files_directory_alias_inside_source_root_is_supported() {
    let (root, _, _) = fixture();
    let source = root.path().join("old");
    let retained = source.join("retained-files");
    fs::rename(source.join("files"), &retained).unwrap();
    link_directory(&retained, &source.join("files"));
    let target = root.path().join("new");
    assert_eq!(import_legacy(&source, &target, true).unwrap().files, 2);
    assert_eq!(import_legacy(&source, &target, false).unwrap().files, 2);
}

#[test]
#[cfg(unix)]
fn individual_file_symlink_outside_files_is_rejected() {
    let (root, storage, _) = fixture();
    let source = root.path().join("old");
    let file = source.join("files").join(format!("{storage}.bin"));
    let outside = root.path().join("outside.bin");
    fs::rename(&file, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &file).unwrap();
    let before = digest(&outside);
    for dry_run in [true, false] {
        let target = root.path().join("new");
        let error = import_legacy(&source, &target, dry_run)
            .unwrap_err()
            .to_string();
        assert!(error.contains("旧文件位置不在受控文件目录内"));
        assert!(!target.exists());
    }
    assert_eq!(before, digest(&outside));
}

#[test]
#[cfg(unix)]
fn database_symlink_outside_source_is_rejected() {
    let (root, _, _) = fixture();
    let source = root.path().join("old");
    let database = source.join("relay.sqlite");
    let outside = root.path().join("outside.sqlite");
    fs::rename(&database, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &database).unwrap();
    let before = digest(&outside);
    for dry_run in [true, false] {
        let target = root.path().join("new");
        let error = import_legacy(&source, &target, dry_run)
            .unwrap_err()
            .to_string();
        assert!(error.contains("旧数据库位置越过旧数据根目录"));
        assert!(!target.exists());
    }
    assert_eq!(before, digest(&outside));
}
