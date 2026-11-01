//! Explicit large-file regression; creates a temporary 256 MiB receiver file.
//! Run with: cargo test -p relay-core --test large_transfer -- --ignored
use relay_core::*;
use sha2::{Digest, Sha256};
use std::{fs, io::Read};
use tempfile::TempDir;

#[test]
#[ignore = "explicit 256 MiB disk/CPU regression"]
fn restart_and_complete_256_mib_with_bounded_buffers() {
    let dir = TempDir::new().unwrap();
    let chunk = vec![0x5au8; 2 * 1024 * 1024];
    let mut expected = Sha256::new();
    for _ in 0..128 {
        expected.update(&chunk);
    }
    let sha256 = hex::encode(expected.finalize());
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    let input = NewUpload {
        name: "256MiB-resume.bin".into(),
        size: 256 * 1024 * 1024,
        mime: "application/octet-stream".into(),
        sha256: sha256.clone(),
        project: "大文件回归".into(),
        build_id: None,
        request_id: "large-regression-256mib".into(),
    };
    let mut task = relay.create_upload(OWNER_ID, input).unwrap();
    for _ in 0..63 {
        task = relay
            .append_upload(OWNER_ID, &task.id, task.offset, &chunk)
            .unwrap();
    }
    assert_eq!(task.offset, 126 * 1024 * 1024);
    let id = task.id.clone();
    drop(relay);
    let relay = Relay::open(Config::new(dir.path())).unwrap();
    let mut task = relay.get_upload(OWNER_ID, &id).unwrap();
    assert_eq!(task.offset, 126 * 1024 * 1024);
    while task.offset < task.size {
        task = relay
            .append_upload(OWNER_ID, &id, task.offset, &chunk)
            .unwrap();
    }
    let item = relay.complete_upload(OWNER_ID, &id).unwrap();
    assert_eq!(item.sha256, sha256);
    let info = relay.file_info(OWNER_ID, &item.id).unwrap();
    assert_eq!(fs::metadata(&info.path).unwrap().len(), 256 * 1024 * 1024);
    let mut file = fs::File::open(info.path).unwrap();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut actual = Sha256::new();
    loop {
        let n = file.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        actual.update(&buffer[..n]);
    }
    assert_eq!(hex::encode(actual.finalize()), sha256);
    assert_eq!(relay.complete_upload(OWNER_ID, &id).unwrap().id, item.id);
}
