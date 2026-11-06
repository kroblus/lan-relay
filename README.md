# 随手传 · LAN Relay

**V0.5.0：统一文件与文字发送、局域网自动发现、长下载恢复、文件直接打开与轻量动效。局域网协议仍为 2。**

把 APK、原图、录屏、普通文件和中文多行文字在自己的电脑与手机之间传递，并把构建、截图和测试反馈关联起来。电脑 UI、业务、网络和持久化全部使用 Rust；手机采用 Kotlin/Compose。无需 Node.js、浏览器窗口或 C# 启动器。

这是等待 Windows / Android 真机验收的试用版本。自动化结果与未测项目应在本地单独记录。编译成功不等于真机验收通过。

## 日常使用

1. 启动 Windows 包中的 `lan-relay.exe`。再次启动会唤起已有窗口。
2. 两端打开应用，在「设备」查看附近设备。电脑可以向发现的手机发起连接，手机也能选择发现的电脑。无需输入配对码。
3. 首次核对两端自动显示的验证数字，在电脑明确批准；已信任设备复用原连接。发现不可用时展开手动地址/扫码入口。
4. 在同一个发送区添加文件、按需写文字，再点击一次发送。拖放和系统分享进入待发送区，可以移除附件。普通传输无需填写开发验收记录。
5. 手机保存后直接预览图片，普通文件交给系统默认应用；APK 通过系统安装入口，首次按提示允许此来源安装。开发者安装/测试记录放在详情中，按需填写。

Windows 关闭窗口后保留托盘与服务；托盘“退出”停止本程序。电脑必须保持运行。端口占用时会报告错误，不会结束占用端口的其他程序。

## 已实现的设计

- 暖白、鼠尾草绿的统一界面：桌面宽屏侧栏/窄屏顶部导航，Android 底部导航；文件和文字共用编辑区，开发者详情按需展开。
- Rust Iced 独立窗口：文件、文字、构建、反馈、搜索、分页、项目整理和设备批准/撤销；首次尺寸按工作区与 DPI 限制。
- 本机文件直接进入 Rust 核心，有界缓冲、增量 SHA-256；本机导入队列保存请求身份，供恢复时核对原文件。
- 原生 Android：扫码、系统分享、SAF 文件选择、持久队列、用户启动的后台传输服务、通知暂停、下载校验后保存。
- 默认 8 MiB 分片，按服务端能力协商、兼容旧 2 MiB 请求；默认单文件 2 GiB / 总存储 20 GiB。已确认 offset 在写盘同步后提交，重启恢复不将未提交尾部当成功。
- 减少重复 HTTP 鉴权、无关进度引发的下载数据库访问、Android 进度日志与通知开销；下载校验合并到一次保存读取，校验成功前不发布媒体文件。阶段与速度分开显示，仍完整核对 SHA-256。
- 原始文件下载、HEAD、Range / If-Range；SHA-256 完整性校验。
- IPv4 局域网设备发现（UDP 4319）、首次两端核对并批准、原生 Bearer 会话与设备撤销；六位码/高熵二维码作为兼容入口。发现信息本身不授予访问权。
- 长文件下载使用连接/空闲超时，不因总耗时超过两分钟而中断；临时网络故障有界自动续传，暂停、取消与完整性检查继续生效。
- 桌面优先 GPU 渲染并保留软件回退；限制单页卡片数量、缓存卡片、将设置持久化移出 UI 线程。两端只在交互变化时执行短过渡，支持减少动效。
- 有边界的 APK 清单解析。无法解析的字段保持未知；元数据不证明签名或可安装性。
- 可重复请求的文字、文件完成与反馈；反馈引用已完成文件，安装/测试状态由用户明确确认。
- 独立 Rust 旧数据导入工具，先校验旧文件再写入新目录，保留源目录。

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

产物在 `output/dist/v0.5.0/windows/`。图标、托盘、单实例和双击均由 Rust 桌面应用负责。工具链由 `rust-toolchain.toml` 固定，依赖由 `Cargo.lock` 锁定。

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

最后一项单独验证 256 MiB 文件中断重启续传，不作为 Wi-Fi 吞吐基准。桌面另有隔离数据目录的 `--smoke-test`。GitHub Actions 对 Linux/Windows Rust 和 Android 单测、Lint、构建分别记录结果，并产出带源码提交与 SHA-256 清单的应用包。全部平台门禁通过后，`Verified delivery bundle` 作业验证两端原始清单，生成第四个 `lan-relay-delivery-<commit>` artifact；其根目录及完整交付 ZIP 根目录均包含 `DELIVERY_MANIFEST.json`，不再依赖单独提供的聚合文件。

## 使用边界

- 默认 HTTP 局域网模式没有传输加密，仅用于你信任的局域网；没有公网中转、自动端口映射或云端上传。
- 已批准设备共享一个工作区；项目标签不提供设备间内容隔离。发现到其它电脑不代表当前版本已支持完整的电脑对电脑收发。
- 自动发现要求可互访的 IPv4 局域网、手机应用在前台；AP 隔离、多网卡/VPN、防火墙可能阻止广播。IPv6 和受限网络保留手动地址入口。Windows 防火墙仅允许你选择的可信私有网络，不需关闭防火墙。
- 手机后台任务可能被系统或用户停止，恢复依赖持久队列、服务端断点和仍有效的原文件授权；不承诺永不停止。
- Android 17 的局域网权限、相机扫码权限与系统文件授权按实际用途请求。TLS 错误不会被绕过。
- 下载、安装和测试通过是独立状态，不会自动安装 APK、执行传来的代码或监控后台剪贴板。
- 源码重写不意味着旧会话/未完成上传自动迁移。旧源码保存在 Git 历史，活动源码树已经替换。

## 项目结构

| 目录 | 内容 |
|---|---|
| `crates/relay-core` | 共享业务、SQLite、文件完整性、APK 元数据 |
| `crates/relay-server` | 原生客户端协议、认证、分片与下载 |
| `crates/relay-desktop` | Iced UI、托盘、单实例、桌面交互 |
| `crates/relay-migrate` | 旧版已完成内容的离线导入 |
| `apps/android` | Kotlin/Compose 原生客户端与系统集成 |
| `packaging/windows` | Windows 便携包和安装器脚本 |
| `output` | 忽略的构建产物、测试证据与临时工具 |

此前用户报告的本机 53 项测试版本未被本次云端读取，不能宣称已导入这些不可见修改。本机 Codex 应先保护原工作区再进行版本对照。
