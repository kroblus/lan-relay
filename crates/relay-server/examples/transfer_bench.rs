//! Opt-in, isolated loopback benchmark using real HTTP, durable acknowledgements,
//! original file bytes and complete SHA-256 checks. Never a Wi-Fi/FEEM benchmark.
//!
//! cargo run --release -p relay-server --example transfer_bench -- \
//!   --mib 256 --chunk-mib 2 --label baseline --work-dir output/bench
use relay_core::{
    ClaimRequest, Config, Item, NewPairRequest, NewUpload, PairClaim, PairTicket, Relay, Upload,
};
use reqwest::Client;
use serde_json::json;
use std::{
    error::Error,
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()?
        .block_on(run())
}

fn rss_bytes() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|s| s.starts_with("VmRSS:"))?;
    line.split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()
        .map(|n| n * 1024)
}

async fn run() -> Result<(), Box<dyn Error>> {
    let mut mib = 256_u64;
    let mut chunk_mib = 2_u64;
    let mut label = "unlabelled".to_string();
    let mut directory = PathBuf::from("output/bench");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mib" => mib = args.next().ok_or("--mib needs a value")?.parse()?,
            "--chunk-mib" => chunk_mib = args.next().ok_or("--chunk-mib needs a value")?.parse()?,
            "--label" => label = args.next().ok_or("--label needs a value")?,
            "--work-dir" => directory = args.next().ok_or("--work-dir needs a value")?.into(),
            _ => return Err(format!("Unknown argument {arg}").into()),
        }
    }
    if !(1..=2048).contains(&mib) || !(1..=16).contains(&chunk_mib) {
        return Err("Use 1..=2048 MiB files and 1..=16 MiB chunks".into());
    }
    fs::create_dir_all(&directory)?;
    let data = tempfile::Builder::new()
        .prefix("transfer-benchmark-")
        .tempdir_in(directory)?;
    let source = data.path().join("source.bin");
    let downloaded = data.path().join("downloaded.bin");
    let size = mib * 1024 * 1024;
    let mut pattern = vec![0_u8; 1024 * 1024];
    let mut random = 0x6d32_a149_u32;
    for byte in &mut pattern {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        *byte = random as u8;
    }
    let prepare = Instant::now();
    let mut file = File::create(&source)?;
    for _ in 0..mib {
        file.write_all(&pattern)?;
    }
    file.sync_all()?;
    drop(file);
    drop(pattern);
    let source_generation_seconds = prepare.elapsed().as_secs_f64();

    let rss_start = rss_bytes();
    let peak = Arc::new(AtomicU64::new(rss_start.unwrap_or(0)));
    let running = Arc::new(AtomicBool::new(true));
    let sample_peak = peak.clone();
    let sample_running = running.clone();
    let sampler = thread::spawn(move || {
        while sample_running.load(Ordering::Relaxed) {
            if let Some(rss) = rss_bytes() {
                sample_peak.fetch_max(rss, Ordering::Relaxed);
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    let all = Instant::now();
    let hash_start = Instant::now();
    let expected = relay_core::hash_file(&source, |_, _| {})?;
    let source_hash_seconds = hash_start.elapsed().as_secs_f64();
    let mut config = Config::new(data.path().join("receiver"));
    config.chunk_size = chunk_mib * 1024 * 1024;
    let relay = Arc::new(Relay::open(config)?);
    let server = relay_server::start(relay.clone(), "127.0.0.1:0".parse()?).await?;
    let base = format!("http://{}", server.addr);
    // Isolated loopback must not be sent through an external HTTP proxy.
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let invite = relay.create_invite()?;
    let ticket: PairTicket = client
        .post(format!("{base}/api/v2/pair/requests"))
        .json(&NewPairRequest {
            name: "隔离性能基准".into(),
            code: None,
            token: Some(invite.token),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    relay.decide_pair(&ticket.id, true)?;
    let claim: PairClaim = client
        .post(format!("{base}/api/v2/pair/claim"))
        .json(&ClaimRequest {
            id: ticket.id,
            claim_token: ticket.claim_token,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let token = claim.token.ok_or("Pairing was not approved")?;
    let mut upload: Upload = client
        .post(format!("{base}/api/v2/uploads"))
        .bearer_auth(&token)
        .json(&NewUpload {
            name: "benchmark.bin".into(),
            size,
            mime: "application/octet-stream".into(),
            sha256: expected.clone(),
            project: "isolated-benchmark".into(),
            build_id: None,
            request_id: uuid::Uuid::new_v4().to_string(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let upload_start = Instant::now();
    let mut input = File::open(&source)?;
    let mut buffer = vec![0_u8; upload.chunk_size as usize];
    let mut requests = 0_u64;
    while upload.offset < size {
        let count = (size - upload.offset).min(buffer.len() as u64) as usize;
        input.read_exact(&mut buffer[..count])?;
        let previous = upload.offset;
        upload = client
            .patch(format!("{base}/api/v2/uploads/{}", upload.id))
            .bearer_auth(&token)
            .header("Content-Type", "application/octet-stream")
            .header("Upload-Offset", previous)
            .body(buffer[..count].to_vec())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if upload.offset != previous + count as u64 {
            return Err("Invalid durable upload offset".into());
        }
        requests += 1;
    }
    drop(input);
    drop(buffer);
    let upload_seconds = upload_start.elapsed().as_secs_f64();
    let complete_start = Instant::now();
    let item: Item = client
        .post(format!("{base}/api/v2/uploads/{}/complete", upload.id))
        .bearer_auth(&token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let server_verify_seconds = complete_start.elapsed().as_secs_f64();
    if item.sha256 != expected || item.size != size {
        return Err("Server completion identity mismatch".into());
    }
    let download_start = Instant::now();
    let mut response = client
        .get(format!("{base}/api/v2/files/{}", item.id))
        .bearer_auth(&token)
        .send()
        .await?
        .error_for_status()?;
    let mut output = File::create(&downloaded)?;
    let mut received = 0_u64;
    while let Some(chunk) = response.chunk().await? {
        output.write_all(&chunk)?;
        received += chunk.len() as u64;
    }
    output.sync_all()?;
    drop(output);
    drop(response);
    let download_seconds = download_start.elapsed().as_secs_f64();
    let verify_start = Instant::now();
    let actual = relay_core::hash_file(&downloaded, |_, _| {})?;
    let download_verify_seconds = verify_start.elapsed().as_secs_f64();
    if actual != expected || received != size {
        return Err("Downloaded bytes failed full SHA-256/size verification".into());
    }
    let end_to_end_seconds = all.elapsed().as_secs_f64();
    running.store(false, Ordering::Relaxed);
    let _ = sampler.join();
    drop(client);
    server.shutdown().await;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "label": label, "protocol": 2, "server_version": relay.hello().version,
            "transport": "isolated-loopback-http", "size_bytes": size,
            "chunk_bytes": chunk_mib * 1024 * 1024, "acknowledged_requests": requests,
            "source_generation_seconds": source_generation_seconds,
            "source_hash_seconds": source_hash_seconds, "upload_seconds": upload_seconds,
            "server_verify_seconds": server_verify_seconds, "download_to_disk_seconds": download_seconds,
            "download_verify_seconds": download_verify_seconds, "end_to_end_seconds": end_to_end_seconds,
            "upload_mib_per_second": mib as f64 / upload_seconds,
            "download_to_disk_mib_per_second": mib as f64 / download_seconds,
            "process_rss_before_bytes_linux": rss_start,
            "process_peak_rss_bytes_linux": if rss_start.is_some() {Some(peak.load(Ordering::Relaxed))} else {None},
            "rss_scope": "client and server in the same process; sampled every 20 ms, not a device or server-only memory bound",
            "sha256": actual, "integrity": "PASS", "architecture": std::env::consts::ARCH,
            "runtime_workers": 2, "runtime_blocking_limit": 4,
            "measurement_scope": "end_to_end includes source hashing, server/pairing setup, upload acknowledgements, server verification, download to synced disk and independent downloaded-file hashing; excludes fixture generation and shutdown",
            "network_speed_claim": "None. This is a loopback/storage/cache benchmark, not phone Wi-Fi or FEEM."
        }))?
    );
    Ok(())
}
