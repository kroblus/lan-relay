//! Shared, persistent business rules used by both the native UI and LAN server.
//! Methods perform blocking disk work; callers must run them off their UI/async workers.
mod apk;
mod error;
mod models;

pub use error::{RelayError, Result};
pub use models::*;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use fs2::FileExt;
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, Rng, RngCore};
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard, Weak,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;
use uuid::Uuid;

const INVITE_LIFETIME: i64 = 5 * 60 * 1000;
const CLAIM_LIFETIME: i64 = 10 * 60 * 1000;
const SESSION_LIFETIME: i64 = 90 * 24 * 60 * 60 * 1000;
const MAX_TEXT: usize = 100 * 1024;

/// Authentication result scoped to a single request, never a session cache.
/// Reading a slow request body must not extend the credential's expiry.
#[derive(Clone)]
pub struct AuthenticatedSession {
    actor: String,
    expires_at: i64,
}

impl AuthenticatedSession {
    /// Core operations separately check that this device has not been revoked.
    pub fn actor(&self) -> Result<&str> {
        if self.expires_at <= now_ms() {
            return Err(RelayError::new(
                401,
                "UNAUTHENTICATED",
                "连接已失效，请重新配对",
            ));
        }
        Ok(&self.actor)
    }
}

pub struct Relay {
    config: Config,
    server_id: String,
    master: [u8; 32],
    db: Mutex<Connection>,
    _workspace_lock: File,
    upload_locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    events: broadcast::Sender<()>,
    authorization_generation: AtomicU64,
}

impl Relay {
    pub fn open(config: Config) -> Result<Self> {
        if config.chunk_size == 0
            || config.chunk_size > 16 * 1024 * 1024
            || config.max_file_bytes > i64::MAX as u64
            || config.max_storage_bytes > i64::MAX as u64
            || config.max_file_bytes > config.max_storage_bytes
        {
            return Err(bad("INVALID_CONFIG", "分片或存储上限配置不正确"));
        }
        short_text(&config.name, 80, "电脑名称")?;
        private_directory(&config.data_dir)?;
        if config.data_dir.join("relay.sqlite").exists() {
            return Err(RelayError::new(
                409,
                "LEGACY_WORKSPACE",
                "检测到旧版数据。请选择独立的 V2 数据目录；旧目录不会自动升级或删除。",
            ));
        }
        let lock = private_file(&config.data_dir.join("workspace.lock"), false)?;
        lock.try_lock_exclusive()
            .map_err(|_| RelayError::new(409, "WORKSPACE_BUSY", "此数据目录已有随手传服务运行"))?;
        private_directory(&config.data_dir.join("files"))?;
        private_directory(&config.data_dir.join("temp"))?;
        let master = load_master(&config.data_dir.join("server-key"))?;
        let database_path = config.data_dir.join("relay-v2.sqlite");
        if !database_path.exists() {
            private_file(&database_path, true)?.sync_all()?;
        }
        let db = Connection::open(database_path)?;
        let schema_version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let table_count: i64 = db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if (schema_version != 0 && schema_version != 2) || (schema_version == 0 && table_count > 0)
        {
            return Err(RelayError::new(
                409,
                "UNSUPPORTED_SCHEMA",
                "数据库版本不受支持，请使用对应版本或恢复备份；未修改原数据库结构",
            ));
        }
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS devices(id TEXT PRIMARY KEY,name TEXT NOT NULL,created_at INTEGER NOT NULL,last_seen INTEGER NOT NULL,revoked INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS sessions(token_hash TEXT PRIMARY KEY,device_id TEXT NOT NULL,expires_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS invitations(id INTEGER PRIMARY KEY CHECK(id=1),token_hash TEXT NOT NULL,code_hash TEXT NOT NULL,expires_at INTEGER NOT NULL,used INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS pairs(id TEXT PRIMARY KEY,name TEXT NOT NULL,claim_hash TEXT NOT NULL,status TEXT NOT NULL,device_id TEXT,created_at INTEGER NOT NULL,expires_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS discovery_pairs(pair_id TEXT PRIMARY KEY REFERENCES pairs(id) ON DELETE CASCADE,peer_id TEXT NOT NULL,verification_code TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS uploads(id TEXT PRIMARY KEY,device_id TEXT NOT NULL,request_id TEXT NOT NULL,body TEXT NOT NULL,UNIQUE(device_id,request_id));
            CREATE TABLE IF NOT EXISTS items(id TEXT PRIMARY KEY,device_id TEXT NOT NULL,kind TEXT NOT NULL,request_id TEXT NOT NULL,request_fingerprint TEXT NOT NULL,created_at INTEGER NOT NULL,body TEXT NOT NULL,UNIQUE(device_id,kind,request_id));
            CREATE INDEX IF NOT EXISTS items_cursor ON items(created_at DESC,id DESC);
            PRAGMA user_version=2;")?;
        let server_id: String = match db
            .query_row("SELECT value FROM meta WHERE key='server_id'", [], |r| {
                r.get(0)
            })
            .optional()?
        {
            Some(id) => id,
            None => {
                let id = new_id();
                db.execute("INSERT INTO meta(key,value) VALUES('server_id',?)", [&id])?;
                id
            }
        };
        let (events, _) = broadcast::channel(32);
        let relay = Self {
            config,
            server_id,
            master,
            db: Mutex::new(db),
            _workspace_lock: lock,
            upload_locks: Mutex::new(HashMap::new()),
            events,
            authorization_generation: AtomicU64::new(0),
        };
        relay.recover()?;
        Ok(relay)
    }

    fn db(&self) -> Result<MutexGuard<'_, Connection>> {
        self.db
            .lock()
            .map_err(|_| RelayError::new(500, "STATE_LOCK", "服务状态需要重新启动"))
    }

    pub fn hello(&self) -> Hello {
        Hello {
            protocol_version: PROTOCOL_VERSION,
            version: env!("CARGO_PKG_VERSION").into(),
            server_id: self.server_id.clone(),
            name: self.config.name.clone(),
            chunk_size: self.config.chunk_size,
            max_file_bytes: self.config.max_file_bytes,
            max_storage_bytes: self.config.max_storage_bytes,
            capabilities: [
                "resumable_upload",
                "range_download",
                "text",
                "feedback",
                "native_owner",
                "events",
                "discovery_pairing",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }

    pub fn data_dir(&self) -> &Path {
        &self.config.data_dir
    }
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.events.subscribe()
    }
    /// Changes only when authorization is revoked. File progress must not force
    /// unrelated download streams to perform a database lookup for every chunk.
    pub fn authorization_generation(&self) -> u64 {
        self.authorization_generation.load(Ordering::Acquire)
    }
    fn changed(&self) {
        let _ = self.events.send(());
    }

    fn ensure_actor(db: &Connection, actor: &str) -> Result<()> {
        if actor == OWNER_ID {
            return Ok(());
        }
        let active: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM devices WHERE id=? AND revoked=0)",
            [actor],
            |r| r.get(0),
        )?;
        if !active {
            return Err(RelayError::new(
                401,
                "UNAUTHENTICATED",
                "连接已失效，请重新配对",
            ));
        }
        Ok(())
    }

    pub fn authenticate(&self, token: &str) -> Result<String> {
        Ok(self.authenticate_session(token)?.actor()?.to_owned())
    }

    pub fn authenticate_session(&self, token: &str) -> Result<AuthenticatedSession> {
        if token.len() != 43 {
            return Err(RelayError::new(
                401,
                "UNAUTHENTICATED",
                "需要有效的设备凭据",
            ));
        }
        let db = self.db()?;
        let authenticated: Option<(String, i64)> = db.query_row("SELECT s.device_id,s.expires_at FROM sessions s JOIN devices d ON d.id=s.device_id WHERE s.token_hash=? AND s.expires_at>? AND d.revoked=0", params![digest(token.as_bytes()), now_ms()], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let (actor, expires_at) = authenticated
            .ok_or_else(|| RelayError::new(401, "UNAUTHENTICATED", "连接已失效，请重新配对"))?;
        db.execute(
            "UPDATE devices SET last_seen=? WHERE id=? AND last_seen<?",
            params![now_ms(), actor, now_ms() - 60_000],
        )?;
        Ok(AuthenticatedSession { actor, expires_at })
    }

    pub fn create_invite(&self) -> Result<Invite> {
        let invite = Invite {
            code: format!("{:06}", OsRng.gen_range(0..1_000_000)),
            token: random_token(),
            expires_at: now_ms() + INVITE_LIFETIME,
        };
        self.db()?.execute("INSERT OR REPLACE INTO invitations(id,token_hash,code_hash,expires_at,used) VALUES(1,?,?,?,0)", params![digest(invite.token.as_bytes()),digest(invite.code.as_bytes()),invite.expires_at])?;
        self.changed();
        Ok(invite)
    }

    pub fn request_pair(&self, request: NewPairRequest) -> Result<PairTicket> {
        short_text(&request.name, 80, "设备名称")?;
        let credential = match (&request.code, &request.token) {
            (Some(code), None) if code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()) => {
                (false, code.as_str())
            }
            (None, Some(token)) if token.len() == 43 => (true, token.as_str()),
            _ => {
                return Err(bad(
                    "INVALID_INVITE",
                    "请提供当前有效的六位连接码或扫码凭据",
                ))
            }
        };
        let mut db = self.db()?;
        let tx = db.transaction()?;
        let invite: Option<(String, String, i64, i64)> = tx
            .query_row(
                "SELECT token_hash,code_hash,expires_at,used FROM invitations WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let invite = invite
            .filter(|i| i.2 > now_ms() && i.3 == 0)
            .ok_or_else(|| bad("INVITE_EXPIRED", "连接码已失效，请在电脑重新生成"))?;
        if digest(credential.1.as_bytes()) != if credential.0 { invite.0 } else { invite.1 } {
            return Err(bad("INVITE_INVALID", "连接码不正确或已失效"));
        }
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pairs WHERE status='pending' AND expires_at>?",
            [now_ms()],
            |r| r.get(0),
        )?;
        if count >= 20 {
            return Err(RelayError::new(
                429,
                "PAIR_QUEUE_FULL",
                "等待批准的设备过多，请在电脑处理后重试",
            ));
        }
        let ticket = PairTicket {
            id: new_id(),
            claim_token: random_token(),
            expires_at: now_ms() + CLAIM_LIFETIME,
            verification_code: None,
        };
        tx.execute("INSERT INTO pairs(id,name,claim_hash,status,created_at,expires_at) VALUES(?,?,?,'pending',?,?)", params![ticket.id,request.name.trim(),digest(ticket.claim_token.as_bytes()),now_ms(),ticket.expires_at])?;
        tx.execute("UPDATE invitations SET used=1 WHERE id=1", [])?;
        tx.commit()?;
        self.changed();
        Ok(ticket)
    }

    pub fn request_discovery_pair(&self, request: NewDiscoveryPairRequest) -> Result<PairTicket> {
        short_text(&request.name, 80, "设备名称")?;
        if request.server_id != self.server_id {
            return Err(RelayError::new(
                409,
                "SERVER_ID_CHANGED",
                "电脑身份已改变，请重新查找设备",
            ));
        }
        if Uuid::parse_str(&request.peer_id).is_err() || request.peer_id.len() != 36 {
            return Err(bad("INVALID_PEER_ID", "附近设备标识不正确"));
        }
        let mut db = self.db()?;
        let tx = db.transaction()?;
        let pending: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pairs WHERE status='pending' AND expires_at>?",
            [now_ms()],
            |r| r.get(0),
        )?;
        if pending >= 20 {
            return Err(RelayError::new(
                429,
                "PAIR_QUEUE_FULL",
                "等待确认的设备过多，请在电脑处理后重试",
            ));
        }
        // The installation ID is untrusted discovery metadata, never an actor.
        // Do not return a previous claim secret for a repeated/spoofed ID.
        let already_pending: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM discovery_pairs d JOIN pairs p ON p.id=d.pair_id WHERE d.peer_id=? AND p.status='pending' AND p.expires_at>?)",
            params![request.peer_id, now_ms()], |r| r.get(0),
        )?;
        if already_pending {
            return Err(RelayError::new(
                409,
                "PAIR_ALREADY_PENDING",
                "这个设备已有待确认请求，请继续原请求或稍后重试",
            ));
        }
        let verification_code = format!("{:06}", OsRng.gen_range(0..1_000_000));
        let ticket = PairTicket {
            id: new_id(),
            claim_token: random_token(),
            expires_at: now_ms() + INVITE_LIFETIME,
            verification_code: Some(verification_code.clone()),
        };
        tx.execute(
            "INSERT INTO pairs(id,name,claim_hash,status,created_at,expires_at) VALUES(?,?,?,'pending',?,?)",
            params![ticket.id, request.name.trim(), digest(ticket.claim_token.as_bytes()), now_ms(), ticket.expires_at],
        )?;
        tx.execute(
            "INSERT INTO discovery_pairs(pair_id,peer_id,verification_code) VALUES(?,?,?)",
            params![ticket.id, request.peer_id, verification_code],
        )?;
        tx.commit()?;
        self.changed();
        Ok(ticket)
    }

    pub fn decide_pair(&self, id: &str, approve: bool) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction()?;
        let p: Option<(String, String, i64)> = tx
            .query_row(
                "SELECT name,status,expires_at FROM pairs WHERE id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (name, status, expires_at) = p.ok_or_else(|| missing("配对请求"))?;
        if expires_at <= now_ms() {
            return Err(bad("PAIR_EXPIRED", "配对请求已过期"));
        }
        let desired = if approve { "approved" } else { "rejected" };
        if status == desired {
            return Ok(());
        }
        if status != "pending" {
            return Err(RelayError::new(409, "PAIR_DECIDED", "此请求已处理"));
        }
        if approve {
            let device_id = new_id();
            tx.execute(
                "INSERT INTO devices(id,name,created_at,last_seen) VALUES(?,?,?,?)",
                params![device_id, name, now_ms(), now_ms()],
            )?;
            tx.execute(
                "UPDATE pairs SET status='approved',device_id=? WHERE id=?",
                params![device_id, id],
            )?;
        } else {
            tx.execute("UPDATE pairs SET status='rejected' WHERE id=?", [id])?;
        }
        tx.commit()?;
        self.changed();
        Ok(())
    }

    /// A requester may cancel only its own still-pending first-trust request.
    /// A discovery ID or verification number is not enough, and cancellation
    /// never revokes an already approved device.
    pub fn cancel_pair(&self, request: ClaimRequest) -> Result<PairClaim> {
        if request.claim_token.len() != 43 {
            return Err(bad("INVALID_CLAIM", "配对领取凭据不正确"));
        }
        let mut db = self.db()?;
        let tx = db.transaction()?;
        let row: Option<(String, String, i64)> = tx
            .query_row(
                "SELECT claim_hash,status,expires_at FROM pairs WHERE id=?",
                [&request.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (hash, status, expiry) = row.ok_or_else(|| missing("配对请求"))?;
        if digest(request.claim_token.as_bytes()) != hash {
            return Err(RelayError::new(403, "INVALID_CLAIM", "配对领取凭据不正确"));
        }
        if status == "approved" {
            return Err(RelayError::new(
                409,
                "PAIR_ALREADY_APPROVED",
                "电脑已确认连接，请完成连接或在电脑撤销设备",
            ));
        }
        let result = if expiry <= now_ms() {
            "expired"
        } else {
            "rejected"
        };
        if status == "pending" {
            tx.execute(
                "UPDATE pairs SET status='rejected' WHERE id=?",
                [&request.id],
            )?;
        }
        tx.commit()?;
        self.changed();
        Ok(PairClaim {
            status: result.into(),
            device: None,
            token: None,
        })
    }

    pub fn claim_pair(&self, request: ClaimRequest) -> Result<PairClaim> {
        if request.claim_token.len() != 43 {
            return Err(bad("INVALID_CLAIM", "配对领取凭据不正确"));
        }
        let db = self.db()?;
        let row: Option<(String, String, Option<String>, i64)> = db
            .query_row(
                "SELECT claim_hash,status,device_id,expires_at FROM pairs WHERE id=?",
                [&request.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let (claim_hash, status, device_id, expires_at) = row.ok_or_else(|| missing("配对请求"))?;
        if digest(request.claim_token.as_bytes()) != claim_hash {
            return Err(RelayError::new(403, "INVALID_CLAIM", "配对领取凭据不正确"));
        }
        if expires_at <= now_ms() {
            return Ok(PairClaim {
                status: "expired".into(),
                token: None,
                device: None,
            });
        }
        if status != "approved" {
            return Ok(PairClaim {
                status,
                token: None,
                device: None,
            });
        }
        let device_id = device_id.ok_or_else(|| bad("INVALID_PAIR_STATE", "配对状态不正确"))?;
        let device = device(&db, &device_id)?;
        if device.revoked {
            return Ok(PairClaim {
                status: "rejected".into(),
                token: None,
                device: None,
            });
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.master).expect("fixed HMAC key length");
        mac.update(b"lan-relay-v2-session\0");
        mac.update(request.id.as_bytes());
        mac.update(request.claim_token.as_bytes());
        let token = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        db.execute(
            "INSERT OR IGNORE INTO sessions(token_hash,device_id,expires_at) VALUES(?,?,?)",
            params![
                digest(token.as_bytes()),
                device_id,
                now_ms() + SESSION_LIFETIME
            ],
        )?;
        Ok(PairClaim {
            status,
            token: Some(token),
            device: Some(device),
        })
    }

    pub fn revoke_device(&self, id: &str) -> Result<()> {
        if id == OWNER_ID {
            return Err(bad("OWNER_LOCAL", "电脑身份由本地应用管理"));
        }
        let mut db = self.db()?;
        let tx = db.transaction()?;
        if tx.execute("UPDATE devices SET revoked=1 WHERE id=?", [id])? == 0 {
            return Err(missing("设备"));
        }
        tx.execute("DELETE FROM sessions WHERE device_id=?", [id])?;
        tx.commit()?;
        self.authorization_generation
            .fetch_add(1, Ordering::Release);
        self.changed();
        Ok(())
    }

    pub fn snapshot(&self, actor: &str) -> Result<Snapshot> {
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        let mut devices_stmt = db.prepare("SELECT id,name,created_at,last_seen,revoked FROM devices ORDER BY created_at DESC LIMIT 200")?;
        let devices = devices_stmt
            .query_map([], device_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let pair_requests = if actor == OWNER_ID {
            let mut s = db.prepare("SELECT p.id,p.name,p.status,p.created_at,p.expires_at,d.verification_code,d.peer_id FROM pairs p LEFT JOIN discovery_pairs d ON d.pair_id=p.id WHERE p.expires_at>? ORDER BY p.created_at DESC LIMIT 50")?;
            let rows = s
                .query_map([now_ms()], |r| {
                    Ok(PairRequest {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        status: r.get(2)?,
                        created_at: r.get(3)?,
                        expires_at: r.get(4)?,
                        verification_code: r.get(5)?,
                        peer_id: r.get(6)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        } else {
            Vec::new()
        };
        let mut s = db.prepare("SELECT body FROM uploads WHERE (?='owner' OR device_id=?) ORDER BY CASE WHEN json_extract(body,'$.status')='uploading' THEN 0 ELSE 1 END,rowid DESC LIMIT 200")?;
        let uploads = s
            .query_map(params![actor, actor], |r| r.get::<_, String>(0))?
            .map(|r| decode::<Upload>(&r?))
            .collect::<Result<Vec<_>>>()?;
        let items = item_page(&db, None, 100)?.items;
        let used_bytes = storage_bytes(&db)?;
        Ok(Snapshot {
            hello: self.hello(),
            devices,
            pair_requests,
            uploads,
            items,
            used_bytes,
            max_storage_bytes: self.config.max_storage_bytes,
        })
    }

    pub fn list_items(&self, actor: &str, before: Option<&str>, limit: usize) -> Result<ItemPage> {
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        item_page(&db, before, limit.clamp(1, 100))
    }

    pub fn create_text(&self, actor: &str, input: NewText) -> Result<Item> {
        check_text(&input.text)?;
        short_text_allow_empty(&input.project, 128, "项目名称")?;
        request_id(&input.request_id)?;
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        let fingerprint = fingerprint(&input)?;
        if let Some(existing) = existing_item(&db, actor, "text", &input.request_id, &fingerprint)?
        {
            return Ok(existing);
        }
        let mut item = empty_item(actor, "text", &input.project);
        item.text = input.text;
        insert_item(&db, &item, &input.request_id, &fingerprint)?;
        self.changed();
        Ok(item)
    }

    pub fn create_feedback(&self, actor: &str, input: NewFeedback) -> Result<Item> {
        check_text(&input.text)?;
        request_id(&input.request_id)?;
        if input.attachment_ids.len() > 20 {
            return Err(bad("TOO_MANY_ATTACHMENTS", "最多关联 20 个附件"));
        }
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        let fingerprint = fingerprint(&input)?;
        if let Some(existing) =
            existing_item(&db, actor, "feedback", &input.request_id, &fingerprint)?
        {
            return Ok(existing);
        }
        let build = get_item(&db, &input.build_id)?;
        if build.kind != "file" || !build.name.to_ascii_lowercase().ends_with(".apk") {
            return Err(bad("INVALID_BUILD", "请选择已完成传输的 APK 构建"));
        }
        for id in &input.attachment_ids {
            if get_item(&db, id)?.kind != "file" {
                return Err(bad("INVALID_ATTACHMENT", "反馈附件必须是已完成的文件"));
            }
        }
        let mut item = empty_item(actor, "feedback", &build.project);
        item.text = input.text;
        item.build_id = Some(build.id);
        item.attachment_ids = input.attachment_ids;
        insert_item(&db, &item, &input.request_id, &fingerprint)?;
        self.changed();
        Ok(item)
    }

    pub fn patch_item(&self, actor: &str, id: &str, patch: PatchItem) -> Result<Item> {
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        let mut item = get_item(&db, id)?;
        let shared_apk_status = item.kind == "file"
            && item.name.to_ascii_lowercase().ends_with(".apk")
            && patch.test_status.is_some()
            && patch.project.is_none()
            && patch.note.is_none();
        if actor != OWNER_ID && item.device_id != actor && !shared_apk_status {
            return Err(RelayError::new(403, "FORBIDDEN", "只能编辑自己创建的内容"));
        }
        if let Some(project) = patch.project {
            short_text_allow_empty(&project, 128, "项目名称")?;
            item.project = project;
        }
        if let Some(note) = patch.note {
            if note.len() > MAX_TEXT {
                return Err(bad("TEXT_TOO_LONG", "备注过长"));
            }
            item.note = note;
        }
        if let Some(status) = patch.test_status {
            if !["unverified", "installed", "passed", "failed", "blocked"]
                .contains(&status.as_str())
            {
                return Err(bad("INVALID_TEST_STATUS", "测试状态不正确"));
            }
            item.test_status = status;
        }
        db.execute(
            "UPDATE items SET body=? WHERE id=?",
            params![serde_json::to_string(&item)?, id],
        )?;
        self.changed();
        Ok(item)
    }

    pub fn export_feedback(&self, actor: &str, id: &str) -> Result<String> {
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        let feedback = get_item(&db, id)?;
        if feedback.kind != "feedback" {
            return Err(bad("NOT_FEEDBACK", "请选择反馈记录导出"));
        }
        let build = get_item(
            &db,
            feedback
                .build_id
                .as_deref()
                .ok_or_else(|| bad("INVALID_BUILD", "反馈缺少构建引用"))?,
        )?;
        let apk = build.apk.as_ref();
        let known = |value: Option<&str>| {
            value
                .filter(|s| !s.trim().is_empty())
                .map(markdown)
                .unwrap_or_else(|| "未知（未解析）".into())
        };
        let mut out = format!(
            "# 随手传 · 构建反馈\n\n\
            - 项目：{}\n- 构建文件：{}\n- 构建 ID：{}\n- SHA-256：{}\n\
            - APK 包名（package_name）：{}\n- APK 版本名称（version_name）：{}\n\
            - APK 版本号（version_code）：{}\n- APK 元数据解析状态：{}\n\
            - 测试状态（人工确认）：{}\n- 反馈 ID：{}\n- 反馈时间（Unix ms）：{}\n\n\
            ## 问题描述\n\n{}\n\n## 附件\n\n",
            markdown(&build.project),
            markdown(&build.name),
            build.id,
            build.sha256,
            known(apk.and_then(|m| m.package_name.as_deref())),
            known(apk.and_then(|m| m.version_name.as_deref())),
            known(apk.and_then(|m| m.version_code.as_deref())),
            known(apk.map(|m| m.status.as_str())),
            build.test_status,
            feedback.id,
            feedback.created_at,
            quoted(&feedback.text),
        );
        for attachment in feedback.attachment_ids {
            let item = get_item(&db, &attachment)?;
            out.push_str(&format!(
                "- {} · {} bytes · SHA-256 `{}` · ID `{}`\n",
                markdown(&item.name),
                item.size,
                item.sha256,
                item.id
            ));
        }
        out.push_str("\n## 验收记录\n\n- 设备与系统：待人工填写\n- 复现步骤：待补充\n- 预期结果：待补充\n- 实际结果：见描述与附件\n- 自动化 / Windows 桌面 / Android 真机结果分别记录。未执行项目标记 NOT RUN，阻塞项目标记 BLOCKED。\n- 下载不等于安装或测试通过。\n");
        Ok(out)
    }

    pub fn file_info(&self, actor: &str, id: &str) -> Result<FileInfo> {
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        let item = get_item(&db, id)?;
        if item.kind != "file" {
            return Err(missing("文件"));
        }
        let path = self.final_path(&item.id);
        let metadata = fs::metadata(&path)?;
        if !metadata.is_file() || metadata.len() != item.size {
            return Err(RelayError::new(
                409,
                "FILE_CORRUPTED",
                "已保存文件缺失或大小改变",
            ));
        }
        Ok(FileInfo { path, item })
    }

    fn temp_path(&self, id: &str) -> PathBuf {
        self.config.data_dir.join("temp").join(format!("{id}.part"))
    }
    fn final_path(&self, id: &str) -> PathBuf {
        self.config.data_dir.join("files").join(format!("{id}.bin"))
    }

    pub fn create_upload(&self, actor: &str, mut input: NewUpload) -> Result<Upload> {
        validate_upload(&input, &self.config)?;
        input.mime = input.mime.trim().to_owned();
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        let existing: Option<String> = db
            .query_row(
                "SELECT body FROM uploads WHERE device_id=? AND request_id=?",
                params![actor, input.request_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(body) = existing {
            let upload: Upload = decode(&body)?;
            if upload.name != input.name
                || upload.size != input.size
                || upload.sha256 != input.sha256
                || upload.mime != input.mime
                || upload.project != input.project
                || upload.build_id != input.build_id
            {
                return Err(RelayError::new(
                    409,
                    "REQUEST_REUSED",
                    "请求标识已用于其他文件",
                ));
            }
            return Ok(upload);
        }
        if let Some(id) = &input.build_id {
            let build = get_item(&db, id)?;
            if build.kind != "file" || !build.name.to_ascii_lowercase().ends_with(".apk") {
                return Err(bad("INVALID_BUILD", "关联构建必须是已完成的 APK 文件"));
            }
        }
        let active:i64=db.query_row("SELECT COUNT(*) FROM uploads WHERE device_id=? AND json_extract(body,'$.status')='uploading'",[actor],|r|r.get(0))?;
        if active >= 50 {
            return Err(RelayError::new(
                429,
                "TOO_MANY_UPLOADS",
                "最多保留 50 个未完成任务，请先完成或取消",
            ));
        }
        if storage_bytes(&db)?
            .checked_add(input.size)
            .filter(|v| *v <= self.config.max_storage_bytes)
            .is_none()
        {
            return Err(RelayError::new(
                507,
                "QUOTA_EXCEEDED",
                "收件箱空间不足，请在电脑管理文件",
            ));
        }
        let upload = Upload {
            id: new_id(),
            device_id: actor.into(),
            name: input.name,
            size: input.size,
            mime: input.mime,
            sha256: input.sha256,
            project: input.project,
            build_id: input.build_id,
            offset: 0,
            status: "uploading".into(),
            chunk_size: self.config.chunk_size,
            created_at: now_ms(),
            item_id: None,
            request_id: input.request_id,
        };
        let path = self.temp_path(&upload.id);
        private_file(&path, true)?.sync_all()?;
        sync_directory(&self.config.data_dir.join("temp"))?;
        if let Err(error) = db.execute(
            "INSERT INTO uploads(id,device_id,request_id,body) VALUES(?,?,?,?)",
            params![
                upload.id,
                upload.device_id,
                upload.request_id,
                serde_json::to_string(&upload)?
            ],
        ) {
            let _ = fs::remove_file(path);
            return Err(error.into());
        }
        self.changed();
        Ok(upload)
    }

    fn upload_lock(&self, id: &str) -> Result<Arc<Mutex<()>>> {
        let mut locks = self
            .upload_locks
            .lock()
            .map_err(|_| RelayError::new(500, "STATE_LOCK", "传输状态需要重新启动"))?;
        locks.retain(|_, v| v.strong_count() > 0);
        if let Some(lock) = locks.get(id).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(id.into(), Arc::downgrade(&lock));
        Ok(lock)
    }

    pub fn get_upload(&self, actor: &str, id: &str) -> Result<Upload> {
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        upload_for(&db, actor, id)
    }

    pub fn append_upload(
        &self,
        actor: &str,
        id: &str,
        offset: u64,
        bytes: &[u8],
    ) -> Result<Upload> {
        let lock = self.upload_lock(id)?;
        let _guard = lock
            .lock()
            .map_err(|_| bad("UPLOAD_LOCK", "传输状态异常"))?;
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        let mut upload = upload_for(&db, actor, id)?;
        if upload.status != "uploading" {
            return Err(RelayError::new(409, "UPLOAD_NOT_ACTIVE", "此传输已结束"));
        }
        if offset != upload.offset {
            return Err(RelayError::new(
                409,
                "OFFSET_MISMATCH",
                "传输位置已改变，请重新获取已确认位置",
            ));
        }
        if bytes.is_empty()
            || bytes.len() as u64 > upload.chunk_size
            || bytes.len() as u64 > upload.size - upload.offset
        {
            return Err(bad("INVALID_CHUNK", "分片长度不正确"));
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.temp_path(id))?;
        let stored_length = file.metadata()?.len();
        if stored_length < offset {
            upload.status = "failed".into();
            save_upload(&db, &upload)?;
            return Err(RelayError::new(
                409,
                "UPLOAD_CORRUPTED",
                "已确认的文件内容缺失，请重新开始",
            ));
        }
        // Only truncate an unacknowledged tail; resizing an already correct file
        // needlessly dirties its metadata on every durable append.
        if stored_length > offset {
            file.set_len(offset)?;
        }
        file.seek(SeekFrom::Start(offset))?;
        let result = (|| -> Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            upload.offset += bytes.len() as u64;
            save_upload(&db, &upload)?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = file.set_len(offset);
            let _ = file.sync_all();
            return Err(e);
        }
        self.changed();
        Ok(upload)
    }

    pub fn complete_upload(&self, actor: &str, id: &str) -> Result<Item> {
        let lock = self.upload_lock(id)?;
        let _guard = lock
            .lock()
            .map_err(|_| bad("UPLOAD_LOCK", "传输状态异常"))?;
        let upload = {
            let db = self.db()?;
            Self::ensure_actor(&db, actor)?;
            upload_for(&db, actor, id)?
        };
        if upload.status == "complete" {
            let db = self.db()?;
            return get_item(&db, upload.item_id.as_deref().unwrap_or(id));
        }
        if upload.status != "uploading" || upload.offset != upload.size {
            return Err(RelayError::new(
                409,
                "UPLOAD_INCOMPLETE",
                "文件尚未完整接收",
            ));
        }
        self.finish_file(actor, upload)
    }

    fn finish_file(&self, actor: &str, mut upload: Upload) -> Result<Item> {
        let temp = self.temp_path(&upload.id);
        let final_path = self.final_path(&upload.id);
        let candidate = if temp.exists() {
            temp.as_path()
        } else {
            final_path.as_path()
        };
        let valid = fs::metadata(candidate)
            .map(|m| m.len() == upload.size)
            .unwrap_or(false)
            && hash_file(candidate, |_, _| {})? == upload.sha256;
        if !valid {
            upload.status = "failed".into();
            save_upload(&*self.db()?, &upload)?;
            let _ = fs::remove_file(&temp);
            let _ = fs::remove_file(&final_path);
            self.changed();
            return Err(RelayError::new(
                422,
                "HASH_MISMATCH",
                "文件完整性校验失败，请重新传输",
            ));
        }
        let mut item = empty_item(&upload.device_id, "file", &upload.project);
        item.id = upload.id.clone();
        item.name = upload.name.clone();
        item.mime = upload.mime.clone();
        item.size = upload.size;
        item.sha256 = upload.sha256.clone();
        item.build_id = upload.build_id.clone();
        if item.name.to_ascii_lowercase().ends_with(".apk") {
            item.apk = Some(apk::metadata(candidate));
        }
        let mut db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        let tx = db.transaction()?;
        if temp.exists() {
            fs::rename(&temp, &final_path)?;
            sync_directory(&self.config.data_dir.join("files"))?;
        }
        item.created_at = now_ms();
        insert_item(&tx, &item, &upload.request_id, &upload.sha256)?;
        upload.status = "complete".into();
        upload.item_id = Some(item.id.clone());
        save_upload(&tx, &upload)?;
        tx.commit()?;
        self.changed();
        Ok(item)
    }

    pub fn cancel_upload(&self, actor: &str, id: &str) -> Result<()> {
        let lock = self.upload_lock(id)?;
        let _guard = lock
            .lock()
            .map_err(|_| bad("UPLOAD_LOCK", "传输状态异常"))?;
        let db = self.db()?;
        Self::ensure_actor(&db, actor)?;
        let mut upload = upload_for(&db, actor, id)?;
        if upload.status == "complete" {
            return Err(RelayError::new(
                409,
                "UPLOAD_COMPLETE",
                "已完成文件不能通过取消任务删除",
            ));
        }
        upload.status = "cancelled".into();
        save_upload(&db, &upload)?;
        remove_if_exists(&self.temp_path(id))?;
        remove_if_exists(&self.final_path(id))?;
        self.changed();
        Ok(())
    }

    pub fn import_file(&self, path: &Path, project: &str) -> Result<Item> {
        self.import_file_with_progress(path, project, |_, _| {})
    }

    pub fn import_file_with_progress(
        &self,
        path: &Path,
        project: &str,
        progress: impl FnMut(u64, u64),
    ) -> Result<Item> {
        self.import_file_with_request_id(path, project, &new_id(), progress)
    }

    pub fn import_file_with_request_id(
        &self,
        path: &Path,
        project: &str,
        request_id: &str,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<Item> {
        let metadata = fs::metadata(path)?;
        if !metadata.is_file() {
            return Err(bad("NOT_FILE", "请选择普通文件"));
        }
        if metadata.len() > self.config.max_file_bytes {
            return Err(RelayError::new(413, "FILE_TOO_LARGE", "文件超过单文件上限"));
        }
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| bad("INVALID_FILENAME", "文件名不能转换为 Unicode"))?
            .to_owned();
        let sha256 = hash_file(path, &mut progress)?;
        let mime = mime_from_name(&name);
        let mut upload = self.create_upload(
            OWNER_ID,
            NewUpload {
                name,
                size: metadata.len(),
                mime,
                sha256,
                project: project.into(),
                build_id: None,
                request_id: request_id.into(),
            },
        )?;
        if upload.status == "complete" {
            return self.complete_upload(OWNER_ID, &upload.id);
        }
        if upload.status != "uploading" {
            return Err(RelayError::new(
                409,
                "UPLOAD_NOT_ACTIVE",
                "此导入任务已取消或失败，请创建新的任务",
            ));
        }
        (|| -> Result<Item> {
            let mut file = File::open(path)?;
            file.seek(SeekFrom::Start(upload.offset))?;
            let mut buffer = vec![0u8; self.config.chunk_size as usize];
            progress(upload.offset, upload.size);
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                upload =
                    self.append_upload(OWNER_ID, &upload.id, upload.offset, &buffer[..read])?;
                progress(upload.offset, upload.size);
            }
            self.complete_upload(OWNER_ID, &upload.id)
        })()
    }

    fn recover(&self) -> Result<()> {
        let uploads = {
            let db = self.db()?;
            let mut s = db.prepare("SELECT body FROM uploads")?;
            let rows = s
                .query_map([], |r| r.get::<_, String>(0))?
                .map(|r| decode::<Upload>(&r?))
                .collect::<Result<Vec<_>>>()?;
            rows
        };
        for mut upload in uploads {
            let temp = self.temp_path(&upload.id);
            let final_path = self.final_path(&upload.id);
            if upload.status == "uploading" {
                if !temp.exists() && final_path.exists() && upload.offset == upload.size {
                    // A process can stop after rename but before SQLite publishes the item.
                    match self.finish_file(OWNER_ID, upload.clone()) {
                        Ok(_) => continue,
                        Err(e) if e.code == "HASH_MISMATCH" => continue,
                        Err(e) => return Err(e),
                    }
                }
                match fs::metadata(&temp) {
                    Ok(m) if m.is_file() && m.len() >= upload.offset => {
                        let file = OpenOptions::new().write(true).open(&temp)?;
                        file.set_len(upload.offset)?;
                        file.sync_all()?;
                    }
                    _ => {
                        upload.status = "failed".into();
                        save_upload(&*self.db()?, &upload)?;
                        remove_if_exists(&temp)?;
                    }
                }
            } else if upload.status == "failed" || upload.status == "cancelled" {
                remove_if_exists(&temp)?;
                remove_if_exists(&final_path)?;
            }
        }
        let db = self.db()?;
        db.execute("DELETE FROM sessions WHERE expires_at<?", [now_ms()])?;
        db.execute(
            "DELETE FROM pairs WHERE expires_at<?",
            [now_ms() - 24 * 60 * 60 * 1000],
        )?;
        Ok(())
    }
}

fn device_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        id: r.get(0)?,
        name: r.get(1)?,
        created_at: r.get(2)?,
        last_seen: r.get(3)?,
        revoked: r.get(4)?,
    })
}
fn device(db: &Connection, id: &str) -> Result<Device> {
    db.query_row(
        "SELECT id,name,created_at,last_seen,revoked FROM devices WHERE id=?",
        [id],
        device_row,
    )
    .optional()?
    .ok_or_else(|| missing("设备"))
}
fn new_id() -> String {
    Uuid::new_v4().to_string()
}
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn random_token() -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}
fn bad(code: &'static str, message: impl Into<String>) -> RelayError {
    RelayError::new(400, code, message)
}
fn missing(what: &str) -> RelayError {
    RelayError::new(404, "NOT_FOUND", format!("找不到{what}"))
}
fn decode<T: DeserializeOwned>(s: &str) -> Result<T> {
    Ok(serde_json::from_str(s)?)
}
fn fingerprint<T: serde::Serialize>(v: &T) -> Result<String> {
    Ok(digest(&serde_json::to_vec(v)?))
}
fn short_text(s: &str, max: usize, label: &str) -> Result<()> {
    if s.trim().is_empty() {
        return Err(bad("INVALID_TEXT", format!("{label}不能为空")));
    }
    short_text_allow_empty(s, max, label)
}
fn short_text_allow_empty(s: &str, max: usize, label: &str) -> Result<()> {
    if s.chars().count() > max || s.chars().any(char::is_control) {
        Err(bad("INVALID_TEXT", format!("{label}过长或包含控制字符")))
    } else {
        Ok(())
    }
}
fn check_text(s: &str) -> Result<()> {
    if s.trim().is_empty() || s.len() > MAX_TEXT || s.contains('\0') {
        Err(bad("INVALID_TEXT", "文字不能为空，且不能超过 100 KiB"))
    } else {
        Ok(())
    }
}
fn request_id(s: &str) -> Result<()> {
    if s.len() < 8
        || s.len() > 128
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Err(bad("INVALID_REQUEST_ID", "请求标识格式不正确"))
    } else {
        Ok(())
    }
}
fn private_file(path: &Path, create_new: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn private_directory(path: &Path) -> Result<()> {
    if path.exists() {
        if !path.is_dir() {
            return Err(bad("INVALID_DIRECTORY", "数据路径不是目录"));
        }
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}
fn load_master(path: &Path) -> Result<[u8; 32]> {
    if path.exists() {
        let b = fs::read(path)?;
        return b
            .try_into()
            .map_err(|_| bad("INVALID_SERVER_KEY", "服务器身份文件损坏，请恢复备份"));
    }
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    let mut f = private_file(path, true)?;
    f.write_all(&b)?;
    f.sync_all()?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(b)
}
fn empty_item(actor: &str, kind: &str, project: &str) -> Item {
    Item {
        id: new_id(),
        kind: kind.into(),
        device_id: actor.into(),
        created_at: now_ms(),
        project: project.into(),
        text: String::new(),
        name: String::new(),
        mime: String::new(),
        size: 0,
        sha256: String::new(),
        build_id: None,
        attachment_ids: vec![],
        note: String::new(),
        test_status: "unverified".into(),
        apk: None,
    }
}
fn get_item(db: &Connection, id: &str) -> Result<Item> {
    let s: Option<String> = db
        .query_row("SELECT body FROM items WHERE id=?", [id], |r| r.get(0))
        .optional()?;
    decode(&s.ok_or_else(|| missing("内容"))?)
}
fn insert_item(db: &Connection, item: &Item, request_id: &str, fingerprint: &str) -> Result<()> {
    db.execute("INSERT INTO items(id,device_id,kind,request_id,request_fingerprint,created_at,body) VALUES(?,?,?,?,?,?,?)",params![item.id,item.device_id,item.kind,request_id,fingerprint,item.created_at,serde_json::to_string(item)?])?;
    Ok(())
}
fn existing_item(
    db: &Connection,
    actor: &str,
    kind: &str,
    request: &str,
    fingerprint: &str,
) -> Result<Option<Item>> {
    let row:Option<(String,String)>=db.query_row("SELECT body,request_fingerprint FROM items WHERE device_id=? AND kind=? AND request_id=?",params![actor,kind,request],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    match row {
        None => Ok(None),
        Some((body, expected)) => {
            if expected != fingerprint {
                return Err(RelayError::new(
                    409,
                    "REQUEST_REUSED",
                    "请求标识已用于不同的内容",
                ));
            }
            Ok(Some(decode(&body)?))
        }
    }
}
fn item_page(db: &Connection, before: Option<&str>, limit: usize) -> Result<ItemPage> {
    let cursor = if let Some(s) = before {
        if s.len() > 180 {
            return Err(bad("INVALID_CURSOR", "分页位置不正确"));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| bad("INVALID_CURSOR", "分页位置不正确"))?;
        let decoded =
            String::from_utf8(bytes).map_err(|_| bad("INVALID_CURSOR", "分页位置不正确"))?;
        let (t, id) = decoded
            .split_once(':')
            .ok_or_else(|| bad("INVALID_CURSOR", "分页位置不正确"))?;
        let t = t
            .parse::<i64>()
            .map_err(|_| bad("INVALID_CURSOR", "分页位置不正确"))?;
        if Uuid::parse_str(id).is_err() {
            return Err(bad("INVALID_CURSOR", "分页位置不正确"));
        }
        (t, id.to_owned())
    } else {
        (i64::MAX, String::new())
    };
    let mut stmt=db.prepare("SELECT body FROM items WHERE created_at<? OR (created_at=? AND id<?) ORDER BY created_at DESC,id DESC LIMIT ?")?;
    let mut items = stmt
        .query_map(
            params![cursor.0, cursor.0, cursor.1, (limit + 1) as i64],
            |r| r.get::<_, String>(0),
        )?
        .map(|r| decode::<Item>(&r?))
        .collect::<Result<Vec<_>>>()?;
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = if has_more {
        items
            .last()
            .map(|i| URL_SAFE_NO_PAD.encode(format!("{}:{}", i.created_at, i.id)))
    } else {
        None
    };
    Ok(ItemPage { items, next_cursor })
}
fn storage_bytes(db: &Connection) -> Result<u64> {
    let bytes:i64=db.query_row("SELECT COALESCE(SUM(CAST(json_extract(body,'$.size') AS INTEGER)),0) FROM uploads WHERE json_extract(body,'$.status') IN ('uploading','complete')",[],|r|r.get(0))?;
    u64::try_from(bytes)
        .map_err(|_| RelayError::new(500, "INVALID_STORED_DATA", "存储统计数据不正确"))
}
fn markdown(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('`', "\\`")
        .replace('*', "\\*")
        .replace(['\n', '\r'], " ")
}
fn quoted(s: &str) -> String {
    s.lines().map(|l| format!("> {l}\n")).collect()
}
fn upload_for(db: &Connection, actor: &str, id: &str) -> Result<Upload> {
    let raw: Option<String> = db
        .query_row("SELECT body FROM uploads WHERE id=?", [id], |r| r.get(0))
        .optional()?;
    let upload: Upload = decode(&raw.ok_or_else(|| missing("传输任务"))?)?;
    if actor != OWNER_ID && upload.device_id != actor {
        return Err(RelayError::new(403, "FORBIDDEN", "此任务属于另一台设备"));
    }
    Ok(upload)
}
fn save_upload(db: &Connection, u: &Upload) -> Result<()> {
    db.execute(
        "UPDATE uploads SET body=? WHERE id=?",
        params![serde_json::to_string(u)?, u.id],
    )?;
    Ok(())
}
fn validate_upload(input: &NewUpload, config: &Config) -> Result<()> {
    request_id(&input.request_id)?;
    short_text_allow_empty(&input.project, 128, "项目名称")?;
    if input.name.is_empty()
        || input.name.len() > 255
        || input
            .name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
        || input.name == "."
        || input.name == ".."
    {
        return Err(bad("INVALID_FILENAME", "文件名不正确"));
    }
    if input.size > config.max_file_bytes {
        return Err(RelayError::new(413, "FILE_TOO_LARGE", "文件超过单文件上限"));
    }
    if input.sha256.len() != 64
        || !input
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(bad("INVALID_HASH", "需要完整文件的小写 SHA-256"));
    }
    if input.mime.len() > 160
        || input.mime.is_empty()
        || !input.mime.is_ascii()
        || input.mime.bytes().any(|b| b < 32 || b == 127)
    {
        return Err(bad("INVALID_MIME", "文件类型格式不正确"));
    }
    Ok(())
}
pub fn hash_file(path: &Path, mut progress: impl FnMut(u64, u64)) -> Result<String> {
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut offset = 0;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        offset += n as u64;
        progress(offset, total);
    }
    Ok(hex::encode(hasher.finalize()))
}
fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
fn mime_from_name(name: &str) -> String {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "apk" => "application/vnd.android.package-archive",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "txt" | "log" => "text/plain",
        "json" => "application/json",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
    .into()
}
