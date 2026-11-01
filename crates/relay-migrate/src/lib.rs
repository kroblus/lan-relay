//! Offline, non-destructive import of completed V1 content through V2 core rules.
use relay_core::{Config, Item, NewFeedback, NewText, NewUpload, PatchItem, Relay, OWNER_ID};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    error::Error,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug, Clone)]
struct LegacyItem {
    id: String,
    kind: String,
    created_at: String,
    device_name: String,
    project: String,
    data: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImportedRecord {
    pub old_id: String,
    pub new_id: String,
    pub original_kind: String,
    pub original_created_at: String,
    pub original_device_name: String,
    pub original_metadata: Value,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ImportReport {
    pub protocol_version: u32,
    pub dry_run: bool,
    pub files: usize,
    pub texts: usize,
    pub feedback: usize,
    pub incomplete_uploads_not_imported: usize,
    pub warnings: Vec<String>,
    pub records: Vec<ImportedRecord>,
}

fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("旧记录缺少字段 {key}").into())
}

fn source_file(source: &Path, item: &LegacyItem) -> Result<PathBuf> {
    let id = required(&item.data, "_storageId")?;
    Uuid::parse_str(id).map_err(|_| "旧文件存储 ID 无效，拒绝访问目录外路径")?;
    let files = source.join("files").canonicalize()?;
    let file = files.join(format!("{id}.bin")).canonicalize()?;
    if !file.starts_with(&files) || !file.is_file() {
        return Err("旧文件位置不在受控文件目录内".into());
    }
    Ok(file)
}

fn verify_file(source: &Path, item: &LegacyItem) -> Result<()> {
    let path = source_file(source, item)?;
    let size = item
        .data
        .get("size")
        .and_then(Value::as_u64)
        .ok_or("旧文件大小无效")?;
    if fs::metadata(&path)?.len() != size {
        return Err(format!("旧文件 {} 大小不一致，未开始导入", item.id).into());
    }
    let mut reader = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if hex::encode(hash.finalize()) != required(&item.data, "sha256")? {
        return Err(format!("旧文件 {} SHA-256 不一致，未开始导入", item.id).into());
    }
    Ok(())
}

/// `destination` must not exist. Source must be an offline V1 directory with no
/// process lock or uncheckpointed WAL. Source DB is copied before it is queried;
/// neither SQLite nor the import core ever opens the original DB for writing.
pub fn import_legacy(source: &Path, destination: &Path, dry_run: bool) -> Result<ImportReport> {
    let source = source.canonicalize()?;
    if source.join("process-lock").exists() {
        return Err("旧服务仍有 process-lock。请正常停止旧服务后导入；本工具不会删除锁。".into());
    }
    if fs::metadata(source.join("relay.sqlite-wal"))
        .map(|m| m.len() > 0)
        .unwrap_or(false)
    {
        return Err("旧数据库仍有未归并 WAL。请由旧服务正常关闭并完成 checkpoint 后重试。".into());
    }
    if destination.exists() {
        return Err("目标目录已经存在，请选择全新目录；不会覆盖已有数据。".into());
    }
    let destination_parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = destination_parent.canonicalize()?;
    let leaf = destination.file_name().ok_or("目标目录名称无效")?;
    let destination = parent.join(leaf);
    if destination.starts_with(&source) || source.starts_with(&destination) {
        return Err("新旧目录不能相同或互相包含".into());
    }
    let snapshot = tempfile::Builder::new()
        .prefix("lan-relay-v1-read-")
        .tempdir_in(&parent)?;
    let snapshot_db = snapshot.path().join("legacy.sqlite");
    fs::copy(source.join("relay.sqlite"), &snapshot_db)?;
    let db = Connection::open_with_flags(&snapshot_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let (count, bytes): (u64, u64) = db.query_row(
        "SELECT COUNT(*),COALESCE(SUM(length(data)),0) FROM items",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count > 100_000 || bytes > 128 * 1024 * 1024 {
        return Err("旧记录超过单次导入的元数据预算，请先分批归档。".into());
    }
    let mut stmt = db.prepare(
        "SELECT id,kind,created_at,device_name,project,data FROM items ORDER BY created_at,id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
        ))
    })?;
    let mut items = Vec::new();
    for row in rows {
        let (id, kind, created_at, device_name, project, data) = row?;
        if !matches!(kind.as_str(), "file" | "text" | "feedback") {
            return Err(format!("不支持的旧记录类型 {kind}，未开始导入").into());
        }
        items.push(LegacyItem {
            id,
            kind,
            created_at,
            device_name,
            project,
            data: serde_json::from_str(&data)?,
        });
    }
    let incomplete: usize = db.query_row(
        "SELECT COUNT(*) FROM uploads WHERE status='uploading'",
        [],
        |r| r.get(0),
    )?;
    let mut report = ImportReport {
        protocol_version: 2,
        dry_run,
        incomplete_uploads_not_imported: incomplete,
        warnings: vec![
            "不会迁移旧会话、设备授权或未完成上传；新版手机需要重新配对。".into(),
            "新记录使用导入时间和本机来源；原时间、设备名称及完整元数据保存在本报告。".into(),
        ],
        ..Default::default()
    };
    for item in &items {
        match item.kind.as_str() {
            "file" => {
                verify_file(&source, item)?;
                report.files += 1;
            }
            "text" => {
                required(&item.data, "text")?;
                report.texts += 1;
            }
            _ => {
                required(&item.data, "text")?;
                report.feedback += 1;
            }
        }
    }
    if dry_run {
        return Ok(report);
    }
    let staging = tempfile::Builder::new()
        .prefix("lan-relay-v2-import-")
        .tempdir_in(&parent)?;
    let relay = Relay::open(Config::new(staging.path()))?;
    let mut mapping: HashMap<String, String> = HashMap::new();
    // Files are imported before content referencing them. Build dependencies are
    // topologically resolved; unresolved or cyclic references abort promotion.
    let mut pending: Vec<&LegacyItem> = items.iter().filter(|x| x.kind == "file").collect();
    while !pending.is_empty() {
        let before = pending.len();
        let mut remaining = Vec::new();
        for old in pending {
            let old_build = old.data.get("buildId").and_then(Value::as_str);
            if old_build.is_some_and(|id| !mapping.contains_key(id)) {
                remaining.push(old);
                continue;
            }
            let mut upload = relay.create_upload(
                OWNER_ID,
                NewUpload {
                    name: required(&old.data, "name")?.into(),
                    size: old
                        .data
                        .get("size")
                        .and_then(Value::as_u64)
                        .ok_or("旧文件大小无效")?,
                    mime: old
                        .data
                        .get("mime")
                        .and_then(Value::as_str)
                        .unwrap_or("application/octet-stream")
                        .into(),
                    sha256: required(&old.data, "sha256")?.into(),
                    project: old.project.clone(),
                    build_id: old_build.and_then(|id| mapping.get(id)).cloned(),
                    request_id: Uuid::new_v4().to_string(),
                },
            )?;
            let mut file = fs::File::open(source_file(&source, old)?)?;
            let mut buffer = vec![0u8; upload.chunk_size as usize];
            loop {
                let size = file.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                upload =
                    relay.append_upload(OWNER_ID, &upload.id, upload.offset, &buffer[..size])?;
            }
            let item = relay.complete_upload(OWNER_ID, &upload.id)?;
            restore_note(&relay, old, &item)?;
            mapping.insert(old.id.clone(), item.id.clone());
            record(&mut report, old, &item);
        }
        if remaining.len() == before {
            return Err("旧文件的构建引用缺失或循环，未发布新目录。".into());
        }
        pending = remaining;
    }
    for old in items.iter().filter(|x| x.kind != "file") {
        let text = required(&old.data, "text")?.to_owned();
        let build = old
            .data
            .get("buildId")
            .and_then(Value::as_str)
            .and_then(|id| mapping.get(id))
            .cloned();
        let old_attachments = old
            .data
            .get("attachmentIds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let attachments: Vec<String> = old_attachments
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|id| mapping.get(id).cloned())
            .collect();
        let item = if old.kind == "feedback"
            && build.is_some()
            && attachments.len() == old_attachments.len()
            && attachments.len() <= 20
        {
            relay.create_feedback(
                OWNER_ID,
                NewFeedback {
                    build_id: build.unwrap_or_default(),
                    text,
                    attachment_ids: attachments,
                    project: old.project.clone(),
                    request_id: Uuid::new_v4().to_string(),
                },
            )?
        } else {
            if old.kind == "feedback" {
                report.warnings.push(format!(
                    "旧反馈 {} 无有效构建或附件超出当前限制，原文导入为文字；原引用保存在报告中。",
                    old.id
                ));
            }
            relay.create_text(
                OWNER_ID,
                NewText {
                    text,
                    project: old.project.clone(),
                    request_id: Uuid::new_v4().to_string(),
                },
            )?
        };
        restore_note(&relay, old, &item)?;
        mapping.insert(old.id.clone(), item.id.clone());
        record(&mut report, old, &item);
    }
    drop(relay);
    fs::write(
        staging.path().join("legacy-import-report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    if destination.exists() {
        return Err("导入期间目标目录出现，已停止，未覆盖该目录。".into());
    }
    fs::rename(staging.path(), &destination)?;
    Ok(report)
}

fn record(report: &mut ImportReport, old: &LegacyItem, item: &Item) {
    report.records.push(ImportedRecord {
        old_id: old.id.clone(),
        new_id: item.id.clone(),
        original_kind: old.kind.clone(),
        original_created_at: old.created_at.clone(),
        original_device_name: old.device_name.clone(),
        original_metadata: old.data.clone(),
    });
}

fn restore_note(relay: &Relay, old: &LegacyItem, item: &Item) -> Result<()> {
    let status = match old.data.get("testStatus").and_then(Value::as_str) {
        Some("user-confirmed-installed") => Some("installed"),
        Some("user-confirmed-passed") => Some("passed"),
        Some("issue-found") => Some("failed"),
        Some("not-tested") => Some("unverified"),
        _ => None,
    };
    let note = old
        .data
        .get("note")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if note.is_none() && status.is_none() {
        return Ok(());
    }
    relay.patch_item(
        OWNER_ID,
        &item.id,
        PatchItem {
            project: None,
            note,
            test_status: status.map(str::to_owned),
        },
    )?;
    Ok(())
}
