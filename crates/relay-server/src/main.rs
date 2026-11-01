use relay_core::{Config, Relay};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("随手传启动失败：{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut data_dir = PathBuf::from("output/runtime-v2");
    let mut host = "0.0.0.0".to_owned();
    let mut port = 4318u16;
    let mut name = "我的电脑".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = args.next().ok_or("--data-dir 缺少路径")?.into(),
            "--host" => host = args.next().ok_or("--host 缺少地址")?,
            "--port" => port = args.next().ok_or("--port 缺少端口")?.parse()?,
            "--name" => name = args.next().ok_or("--name 缺少名称")?,
            "--help" | "-h" => {
                println!("relay-server [--data-dir PATH] [--host 0.0.0.0] [--port 4318] [--name 我的电脑]\n原生桌面应用负责批准配对；此命令供协议测试与无界面集成使用。HTTP 局域网模式不加密。");
                return Ok(());
            }
            _ => return Err(format!("未知参数：{arg}").into()),
        }
    }
    let mut config = Config::new(data_dir);
    config.name = name;
    let relay = Arc::new(Relay::open(config)?);
    let server =
        relay_server::start(relay, format!("{host}:{port}").parse::<SocketAddr>()?).await?;
    println!(
        "随手传协议 2 已监听 {}；HTTP 局域网模式不加密。按 Ctrl+C 停止。",
        server.addr
    );
    tokio::signal::ctrl_c().await?;
    server.shutdown().await;
    Ok(())
}
