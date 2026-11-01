use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Preferences {
    pub project: String,
    pub address: String,
    pub pending_files: Vec<QueuedFile>,
    pub draft_files: Vec<PathBuf>,
    pub reduce_motion: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueuedFile {
    pub path: PathBuf,
    pub project: String,
    #[serde(default = "new_request_id")]
    pub request_id: String,
}

fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Preferences {
    pub fn load(directory: &Path) -> Self {
        let path = directory.join("desktop-preferences.json");
        let value = fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= 256 * 1024)
            .and_then(|_| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok());
        let mut value = value.unwrap_or_default();
        value.pending_files.truncate(64);
        value.draft_files.truncate(64);
        value
    }

    pub fn save(&self, directory: &Path) -> io::Result<()> {
        let temporary = directory.join("desktop-preferences.tmp");
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.sync_all()?;
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
        }
        let path = directory.join("desktop-preferences.json");
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
            };
            let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
            let destination: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe {
                MoveFileExW(
                    source.as_ptr(),
                    destination.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            fs::rename(temporary, path)?;
            fs::File::open(directory)?.sync_all()
        }
    }
}

pub fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("LanRelay").join("v2");
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("lan-relay").join("v2");
    }
    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path).join(".local/share/lan-relay/v2");
    }
    std::env::temp_dir().join("lan-relay-v2")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_preserves_each_queued_files_identity_and_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut preferences = Preferences {
            project: "项目甲".into(),
            pending_files: vec![QueuedFile {
                path: dir.path().join("原图.png"),
                project: "项目甲".into(),
                request_id: new_request_id(),
            }],
            ..Preferences::default()
        };
        preferences.save(dir.path()).unwrap();
        let identity = preferences.pending_files[0].request_id.clone();
        preferences.project = "项目乙".into();
        preferences.draft_files = vec![dir.path().join("尚未发送.apk")];
        preferences.reduce_motion = true;
        preferences.save(dir.path()).unwrap();
        let restored = Preferences::load(dir.path());
        assert_eq!(restored.project, "项目乙");
        assert_eq!(restored.pending_files[0].project, "项目甲");
        assert_eq!(restored.pending_files[0].request_id, identity);
        assert_eq!(restored.draft_files, preferences.draft_files);
        assert!(restored.reduce_motion);
        assert!(!dir.path().join("desktop-preferences.tmp").exists());
    }
}
