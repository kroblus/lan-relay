#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod instance;
mod preferences;
mod tray;
mod ui;

use relay_core::{Config, Relay};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

#[derive(Debug, Clone)]
pub enum Signal {
    Tray(tray::TrayAction),
    Refresh,
    ImportProgress { name: String, done: u64, total: u64 },
}

static SIGNALS: OnceLock<Mutex<Option<UnboundedReceiver<Signal>>>> = OnceLock::new();

pub struct Host {
    pub relay: Arc<Relay>,
    pub directory: PathBuf,
    pub port: u16,
    pub sender: UnboundedSender<Signal>,
    pub server: Mutex<Option<relay_server::ServerHandle>>,
}

#[derive(Debug)]
struct Options {
    data_dir: PathBuf,
    port: u16,
    smoke_test: bool,
}

fn options() -> Result<Option<Options>, String> {
    let mut options = Options {
        data_dir: preferences::default_data_dir(),
        port: 4318,
        smoke_test: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => options.data_dir = args.next().ok_or("--data-dir 需要目录")?.into(),
            "--port" => {
                options.port = args
                    .next()
                    .ok_or("--port 需要端口")?
                    .parse()
                    .map_err(|_| "端口必须为 1–65535")?;
                if options.port == 0 {
                    return Err("桌面端口必须为 1–65535".into());
                }
            }
            "--smoke-test" => options.smoke_test = true,
            "--help" | "-h" => {
                println!("随手传 {}\nlan-relay [--data-dir PATH] [--port 4318]\nlan-relay --smoke-test --data-dir EMPTY_TEST_DIRECTORY", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            _ => return Err(format!("未知参数：{arg}")),
        }
    }
    Ok(Some(options))
}

fn main() {
    #[cfg(windows)]
    let headless =
        std::env::args().any(|arg| matches!(arg.as_str(), "--smoke-test" | "--help" | "-h"));
    if let Err(error) = run() {
        eprintln!("随手传启动失败：{error}");
        #[cfg(windows)]
        if !headless {
            rfd::MessageDialog::new()
                .set_title("随手传无法启动")
                .set_description(format!(
                    "{error}\n\n请检查端口占用与数据目录。程序不会结束其他进程。"
                ))
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let Some(options) = options()? else {
        return Ok(());
    };
    if options.smoke_test {
        return smoke_test(&options);
    }
    let mut guard = match instance::acquire(&options.data_dir).map_err(|e| e.to_string())? {
        instance::Instance::Existing => return Ok(()),
        instance::Instance::Primary(guard) => guard,
    };
    let mut config = Config::new(&options.data_dir);
    config.name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "我的电脑".into());
    let relay = Arc::new(Relay::open(config).map_err(|e| e.to_string())?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let server = runtime
        .block_on(relay_server::start(
            relay.clone(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), options.port),
        ))
        .map_err(|e| format!("无法监听端口 {}：{e}", options.port))?;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    SIGNALS
        .set(Mutex::new(Some(receiver)))
        .map_err(|_| "内部事件通道已初始化")?;
    let wake_sender = sender.clone();
    guard
        .watch(move || {
            let _ = wake_sender.send(Signal::Tray(tray::TrayAction::Show));
        })
        .map_err(|e| format!("无法建立单实例唤起通道：{e}"))?;
    let tray = tray::start(sender.clone());
    let tray_enabled = tray.is_ok();
    let tray_notice = tray.as_ref().err().cloned();
    let mut changes = relay.subscribe();
    let refresh_sender = sender.clone();
    let monitor = runtime.spawn(async move {
        while let Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) =
            changes.recv().await
        {
            tokio::time::sleep(Duration::from_millis(250)).await;
            while changes.try_recv().is_ok() {}
            if refresh_sender.send(Signal::Refresh).is_err() {
                break;
            }
        }
    });
    let host = Arc::new(Host {
        relay,
        directory: options.data_dir,
        port: options.port,
        sender,
        server: Mutex::new(Some(server)),
    });
    let app_host = host.clone();
    let result = iced::daemon(
        move || ui::App::boot(app_host.clone(), tray_enabled, tray_notice.clone()),
        ui::App::update,
        ui::App::view,
    )
    .title(|_: &ui::App, _| "随手传 · 手机与电脑，近一点".to_string())
    .subscription(ui::App::subscription)
    .theme(|_: &ui::App, _| Some(ui::theme()))
    .default_font(ui::default_font())
    .antialiasing(true)
    .run()
    .map_err(|e| e.to_string());
    monitor.abort();
    let server = host.server.lock().map_err(|_| "服务退出锁失效")?.take();
    if let Some(server) = server {
        runtime.block_on(server.shutdown());
    }
    drop(tray);
    drop(guard);
    result
}

fn smoke_test(options: &Options) -> Result<(), String> {
    // Require an explicitly fresh directory: acceptance must never use a user's
    // real inbox or mutate their live session just to check startup.
    if options.data_dir.exists()
        && std::fs::read_dir(&options.data_dir)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err("smoke-test 需要空测试目录，不能使用真实收件箱".into());
    }
    let first = instance::acquire(&options.data_dir).map_err(|e| e.to_string())?;
    if !matches!(first, instance::Instance::Primary(_)) {
        return Err("测试目录已有实例".into());
    }
    if !matches!(
        instance::acquire(&options.data_dir).map_err(|e| e.to_string())?,
        instance::Instance::Existing
    ) {
        return Err("单实例锁验证失败".into());
    }
    let relay = Arc::new(Relay::open(Config::new(&options.data_dir)).map_err(|e| e.to_string())?);
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let server = runtime
        .block_on(relay_server::start(relay, "127.0.0.1:0".parse().unwrap()))
        .map_err(|e| e.to_string())?;
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect_timeout(&server.addr, Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(
            format!(
                "GET /api/v2/hello HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                server.addr
            )
            .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream
        .take(32 * 1024)
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;
    let address = server.addr;
    runtime.block_on(server.shutdown());
    if !response.starts_with("HTTP/1.1 200") || !response.contains("\"protocol_version\":2") {
        return Err("真实 HTTP hello 验证失败".into());
    }
    if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok() {
        return Err("服务退出后端口仍可连接".into());
    }
    let report = "{\"single_instance\":\"PASS\",\"http_hello\":\"PASS\",\"shutdown\":\"PASS\",\"native_gui\":\"NOT_RUN\"}";
    std::fs::write(options.data_dir.join("smoke-report.json"), report)
        .map_err(|e| format!("无法保存 smoke-test 报告：{e}"))?;
    println!("{report}");
    Ok(())
}
