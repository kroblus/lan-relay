use std::fmt;

#[derive(Debug)]
pub struct RelayError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl RelayError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for RelayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for RelayError {}

impl From<std::io::Error> for RelayError {
    fn from(e: std::io::Error) -> Self {
        let (status, code, text) = match e.kind() {
            std::io::ErrorKind::NotFound => {
                (404, "FILE_MISSING", "文件不存在，请重新选择或检查存储目录")
            }
            std::io::ErrorKind::PermissionDenied => {
                (500, "STORAGE_PERMISSION", "没有读取或写入所需文件的权限")
            }
            std::io::ErrorKind::StorageFull => (507, "STORAGE_FULL", "磁盘空间不足"),
            _ => (500, "STORAGE_IO", "文件读写失败，已保留可恢复的传输状态"),
        };
        Self::new(status, code, text)
    }
}
impl From<rusqlite::Error> for RelayError {
    fn from(_: rusqlite::Error) -> Self {
        Self::new(500, "DATABASE_ERROR", "本地数据库操作失败")
    }
}
impl From<serde_json::Error> for RelayError {
    fn from(_: serde_json::Error) -> Self {
        Self::new(500, "INVALID_STORED_DATA", "本地记录格式不正确")
    }
}

pub type Result<T> = std::result::Result<T, RelayError>;
