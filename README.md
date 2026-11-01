# 随手传 · LAN Relay

**V0.4.0：统一清新界面、传输开销优化与 V0.3 验收缺陷修复，局域网协议仍为 2。**

把 APK、原图、录屏、普通文件和中文多行文字在自己的电脑与手机之间传递，并把构建、截图和测试反馈关联起来。电脑 UI、业务、网络和持久化全部使用 Rust；手机采用 Kotlin/Compose。无需 Node.js、浏览器窗口或 C# 启动器。

这是等待 Windows / Android 真机验收的试用版本。自动化结果与未测项目见 [验证记录](docs/VERIFICATION.md)，给本机 Codex 的完整任务见 [验收提示词](docs/CODEX_ACCEPTANCE.md)。编译成功不等于真机验收通过。

## 日常使用

1. 启动 Windows 包中的 `lan-relay.exe`。再次启动会唤起已有窗口。
2. 电脑与手机处在可互访的局域网中。在电脑选择实际 LAN 地址，显示配对二维码或六位连接码。
3. 新版手机 App 扫码，或输入电脑地址和当前连接码。设备名称默认「我的手机」。电脑批准后开始收发。
4. 电脑拖入文件或选择文件；手机通过文件选择器或系统分享添加文件。文字支持中文、多行和复制。
5. 选择某个 APK 构建，将截图、录屏和问题文字关联为反馈；导出 Markdown 交给 Codex。

Windows 关闭窗口后保留托盘与服务；托盘“退出”停止本程序。电脑必须保持运行。端口占用时会报告错误，不会结束占用端口的其他程序。

## 已实现的设计

- 暖白、鼠尾草绿的统一界面：桌面宽屏侧栏/窄屏顶部导航，Android 底部导航；文件优先，文字与开发者详情按需展开。[视觉约定](docs/DESIGN_V04.md)
- Rust Iced 独立窗口：文件、文字、构建、反馈、搜索、分页、项目整理和设备批准/撤销；首次尺寸按工作区与 DPI 限制。
- 本机文件直接进入 Rust 核心，有界缓冲、增量 SHA-256；本机导入队列保存请求身份，供恢复时核对原文件。
- 原生 Android：扫码、系统分享、SAF 文件选择、持久队列、用户启动的后台传输服务、通知暂停、下载校验后保存。
- 默认 8 MiB 分片，按服务端能力协商、兼容旧 2 MiB 请求；默认单文件 2 GiB / 总存储 20 GiB。已确认 offset 在写盘同步后提交，重启恢复不将未提交尾部当成功。
- 减少重复 HTTP 鉴权、无关进度引发的下载数据库访问、Android 进度日志与通知开销；下载校验合并到一次保存读取，校验成功前不发布媒体文件。阶段与速度分开显示，仍完整核对 SHA-256。
- 原始文件下载、HEAD、Range / If-Range；SHA-256 完整性校验。
- 六位随机连接码/高熵二维码、电脑批准、原生 Bearer 会话和设备撤销。
- 有边界的 APK 清单解析。无法解析的字段保持未知；元数据不证明签名或可安装性。
- 可重复请求的文字、文件完成与反馈；反馈引用已完成文件，安装/测试状态由用户明确确认。
- 独立 Rust 旧数据导入工具，先校验旧文件再写入新目录，保留源目录。[迁移说明](docs/MIGRATION.md)

## 运行与构建

### Windows 桌面

安装 Rust 工具链和 Visual Studio C++ Build Tools 后，在仓库根目录执行：

```powershell
cargo run --release -p relay-desktop
```

构建便携包与可用时的 Inno Setup 安装器（安装器要求 Inno Setup 6.3 或更新版本）：

```powershell
./packaging/windows/Build.ps1
```

产物在 `output/dist/windows/`。图标、托盘、单实例和双击均由 Rust 桌面应用负责。工具链由 `rust-toolchain.toml` 固定，依赖由 `Cargo.lock` 锁定。

指定独立测试数据目录与端口：

```powershell
cargo run -p relay-desktop -- --data-dir output/verification/desktop-data --port 14318
```

桌面默认 Windows 数据目录：`%LOCALAPPDATA%\LanRelay\v2`。不会自动打开或覆盖旧版 `LanRelay\data`。仓库根目录只是源码，不是默认收件箱。

### Android

要求完整 JDK 17、Android SDK 平台 37.0、Build Tools 36.0.0。Gradle 9.4.1、AGP 9.2.1 和 Compose 编译插件固定在项目中。

```powershell
cd apps/android
./gradlew.bat --no-daemon testDebugUnitTest lintDebug assembleDebug assembleRelease
```

Linux/macOS 使用 `./gradlew`。Android Studio 也可打开 `apps/android/`。`local.properties` 和 SDK 不提交到 Git。

- `app-debug.apk` 是临时调试签名试用包；以 SHA-256 标识具体产物。
- `app-release-unsigned.apk` 需要使用长期保管的签名密钥签名后才能安装。
- 试用包 `com.kroblus.lanrelay.preview` 与正式包 `com.kroblus.lanrelay` 是不同应用。即使包名相同，签名不兼容也不能覆盖安装；不应清除旧数据来掩盖冲突。

### 无界面核心

```sh
cargo run -p relay-server -- --data-dir output/server-data --host 127.0.0.1 --port 14318
```

该入口用于协议集成，配对批准在原生桌面内进行。不存在远程匿名 owner 接口。一般使用者直接运行桌面应用。

## 验证

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p relay-core --test large_transfer -- --ignored
```

最后一项单独验证 256 MiB 文件中断重启续传，不作为 Wi-Fi 吞吐基准。桌面另有隔离数据目录的 `--smoke-test`。GitHub Actions 对 Linux/Windows Rust 和 Android 单测、Lint、构建分别记录结果，并产出带源码提交与 SHA-256 清单的应用包。

## 使用边界

- 默认 HTTP 局域网模式没有传输加密，仅用于你信任的局域网；没有公网中转、自动端口映射或云端上传。
- 已批准设备共享一个工作区；项目标签不提供设备间内容隔离。
- 手机后台任务可能被系统或用户停止，恢复依赖持久队列、服务端断点和仍有效的原文件授权；不承诺永不停止。
- Android 17 的局域网权限、相机扫码权限与系统文件授权按实际用途请求。TLS 错误不会被绕过。
- 下载、安装和测试通过是独立状态，不会自动安装 APK、执行传来的代码或监控后台剪贴板。
- 源码重写不意味着旧会话/未完成上传自动迁移。旧源码保存在 Git 历史，活动源码树已经替换。[迁移边界](docs/MIGRATION.md)

## 项目结构

| 目录 | 内容 |
|---|---|
| `crates/relay-core` | 共享业务、SQLite、文件完整性、APK 元数据 |
| `crates/relay-server` | 原生客户端协议、认证、分片与下载 |
| `crates/relay-desktop` | Iced UI、托盘、单实例、桌面交互 |
| `crates/relay-migrate` | 旧版已完成内容的离线导入 |
| `apps/android` | Kotlin/Compose 原生客户端与系统集成 |
| `packaging/windows` | Windows 便携包和安装器脚本 |
| `docs` | 协议、迁移、验证与 Codex 验收 |
| `output` | 忽略的构建产物、测试证据与临时工具 |

此前用户报告的本机 53 项测试版本未被本次云端读取，不能宣称已导入这些不可见修改。本机 Codex 应先保护原工作区再进行版本对照。
