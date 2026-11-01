//! Explicit, local-only UX verification fixture. Never a production launcher.
//! Example: cargo run -p relay-server --example ux_fixture -- --isolated-test
//!   --data-dir output/verification/unique-empty-fixture --file /path/to/test.bin
//! A test operator may approve one pending ID by writing it to approve-pair.txt
//! inside that isolated directory. No owner HTTP route or automatic trust exists.
use relay_core::{Config, NewText, Relay, OWNER_ID};
use serde_json::json;
use std::{error::Error, path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut explicit = false;
    let mut data_dir: Option<PathBuf> = None;
    let mut files = Vec::new();
    let mut port = 14352u16;
    let mut lifetime = 1200u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--isolated-test" => explicit = true,
            "--data-dir" => data_dir = Some(args.next().ok_or("missing --data-dir")?.into()),
            "--file" => files.push(PathBuf::from(args.next().ok_or("missing --file")?)),
            "--port" => port = args.next().ok_or("missing --port")?.parse()?,
            "--timeout-seconds" => {
                lifetime = args.next().ok_or("missing --timeout-seconds")?.parse()?
            }
            "--help" => {
                println!("ux_fixture --isolated-test --data-dir NEW_EMPTY_DIR [--file PATH ...] [--port 14352] [--timeout-seconds 1200]\nLocal verification fixture only; approve one exact pending ID via approve-pair.txt. Does not publish an owner HTTP endpoint.");
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if !explicit || !(1..=3600).contains(&lifetime) || port == 0 {
        return Err("requires explicit --isolated-test, a nonzero port and timeout 1..3600".into());
    }
    let root = data_dir.ok_or("requires --data-dir NEW_EMPTY_DIR")?;
    if root.exists() && (!root.is_dir() || std::fs::read_dir(&root)?.next().is_some()) {
        return Err(
            "fixture refuses to open a nonempty directory; use a new isolated test directory"
                .into(),
        );
    }
    for file in &files {
        if !file.is_file() {
            return Err("one of the explicitly selected fixture files does not exist".into());
        }
    }
    let mut config = Config::new(&root);
    config.name = "隔离 UX 验证电脑".into();
    let relay = Arc::new(Relay::open(config)?);
    for file in &files {
        relay.import_file(file, "隔离验证")?;
    }
    relay.create_text(
        OWNER_ID,
        NewText {
            text: "隔离验证\n中文多行文字\n文件与文字统一发送".into(),
            project: "隔离验证".into(),
            request_id: uuid::Uuid::new_v4().to_string(),
        },
    )?;
    let server = relay_server::start(relay.clone(), ([127, 0, 0, 1], port).into()).await?;
    println!(
        "Isolated UX fixture listening on {}; waiting for explicit local approval.",
        server.addr
    );
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::sleep(Duration::from_secs(lifetime));
    tokio::pin!(deadline);
    let mut last_state = Vec::new();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = &mut deadline => break,
            _ = tick.tick() => {
                let approve_path = root.join("approve-pair.txt");
                if let Ok(input) = std::fs::read_to_string(&approve_path) {
                    std::fs::remove_file(&approve_path)?;
                    let id = input.trim();
                    let result = if uuid::Uuid::parse_str(id).is_ok() {
                        relay.decide_pair(id, true).map(|_| "approved".to_owned()).map_err(|e| e.code.to_owned())
                    } else { Err("INVALID_PAIR_ID".to_owned()) };
                    std::fs::write(root.join("approval-result.json"), serde_json::to_vec_pretty(&json!({"result":result}))?)?;
                }
                let state = relay.snapshot(OWNER_ID)?;
                let data = serde_json::to_vec_pretty(&json!({"isolated_test":true,"http_port":port,"state":state}))?;
                if data != last_state {
                    let temporary = root.join("fixture-state.json.tmp");
                    std::fs::write(&temporary, &data)?;
                    std::fs::rename(temporary, root.join("fixture-state.json"))?;
                    last_state = data;
                }
            }
        }
    }
    server.shutdown().await;
    println!("Isolated fixture stopped.");
    Ok(())
}
