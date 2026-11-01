# 随手传 Windows 应用

这是一款轻量托盘应用。安装后双击桌面上的“随手传”，应用启动随包的本地服务，并在系统默认浏览器中打开管理网页。再次双击会通知已运行的实例重新打开网页，不会再启动第二个服务。

## 系统与运行方式

- Windows 10 1903 或更新版本、Windows 11；本次提供 x64 包。
- .NET Framework 4.8：Windows 10 May 2019 Update 起已随系统提供。旧版 Windows 不在本次最低支持范围。
- 包内包含 Node.js 24 运行时，使用者无需另外安装 Node、npm、.NET SDK 或浏览器内核。
- 关闭网页不会停止服务；在右下角托盘选择“退出并停止服务”才会结束进程。收件内容和已确认上传断点会保留。
- 第一次启动如果 Windows 询问网络权限，请仅允许自己信任的专用网络。应用不会自动修改防火墙。

## 安装与便携包

`LanRelay-<版本>-windows-x64-setup.exe` 为当前用户安装到 `%LOCALAPPDATA%\Programs\LanRelay`，并提供桌面与开始菜单快捷方式，不请求管理员权限。更新前先从托盘退出旧版本。卸载通过 Windows“已安装的应用”完成，卸载只清理程序，不删除收件箱。

`LanRelay-<版本>-windows-x64-portable.zip` 可解压到自己选择的位置，双击其中的 `LanRelay.exe`。请保留整个目录，不能只把单个 EXE 拷走；可以为它手动创建桌面快捷方式。

这两种包的数据目录均为 `%LOCALAPPDATA%\LanRelay\data`，更换程序位置不会改变数据目录。托盘菜单中的“打开数据目录”可定位它。环境变量 `LAN_RELAY_DATA_DIR` 可以显式覆盖此位置，`LAN_RELAY_PORT` 可以覆盖默认端口 `4318`。

本版 Windows 应用使用 HTTP 局域网模式，适用于自己的可信网络。它通过私有进程通道接收管理入口，不把 owner 密钥输出到控制台或日志。首次启动遇到旧命令行服务占用端口时会给出明确提示，不会接管未知服务或自动删除锁文件。

## 与之前源码版的数据衔接

应用版首次启动使用独立数据目录，**不会声称已自动迁移原项目里的 `.lan-relay-data`**。如果旧版本有需要保留的内容：

1. 先在旧命令窗口按 Ctrl+C，再从托盘退出应用，确认两个服务都已停止。
2. 找到旧版本真实数据目录。它可能由旧版本的 `LAN_RELAY_DATA_DIR` 配置改变，不要仅凭项目文件夹猜测。
3. 备份旧目录和应用版的新目录。将应用版原 `data` 目录改名留存，再把旧数据目录完整复制成 `%LOCALAPPDATA%\LanRelay\data`。
4. 重新打开应用。不要合并两个 SQLite 数据库，也不要只拷贝 `files` 子目录。

手机 App 与手机浏览器的 Cookie 分开保存，因此手机 App 需要单独配对一次。

## 构建

在 Windows PowerShell 7 或 Windows PowerShell 5.1 中运行。构建器使用 Windows 的 .NET Framework `csc.exe`，保持 C# 5 语法；安装器使用 [Inno Setup 6.3 或更新版本](https://jrsoftware.org/isinfo.php)。GitHub 的 Windows Server 2025 runner 镜像已提供 Inno Setup，CI 无需临时下载第三方安装脚本。

先从 [Node.js 官方 v24.22.2 分发目录](https://nodejs.org/dist/v24.22.2/) 下载 `node-v24.22.2-win-x64.zip` 和 `SHASUMS256.txt`，从清单读取该 ZIP 对应的 SHA-256：

```powershell
./apps/windows/Build.ps1 -NodeZip <官方ZIP路径> -NodeZipSha256 <官方清单中的64位SHA256>
./apps/windows/Test.ps1 -AppDirectory ./output/windows/payload
```

也可传入已经校验的完整官方解压目录：

```powershell
./apps/windows/Build.ps1 -NodeDirectory <包含node.exe和LICENSE的目录>
```

默认产物位于 `output/windows`：安装 EXE、便携 ZIP 和 `SHA256SUMS.txt`。`payload/build-info.json` 记录版本、源码提交、工作区是否有修改、运行时版本与 SHA-256，不包含用户配置或凭据。默认构建同时生成两种包；仅在明确不需要安装器时使用 `-SkipInstaller`。

不把已经安装的应用目录作为构建输出。构建器仅覆盖带有自己生成标记的 `payload` 文件夹，不复制收件箱、`.env`、Cookie 或 owner 密钥。

## 验证范围

`Test.ps1` 使用独立临时数据目录，以打包后的 EXE 和随包 Node 执行真实启动、HTTP 健康检查、未配对拒绝、两次重开复用、优雅退出、子进程与锁清理，并生成不含密钥的 JSON 结果。它不会打开浏览器，也不冒充物理电脑的桌面交互验收。

私有 host 另有真实 HTTP 测试：

```sh
node --test apps/windows/tests/host.test.mjs
```

自动测试之后仍应在使用者电脑验证：安装/卸载、桌面图标、默认浏览器实际开页、托盘、Windows 专用网络提示，以及手机连接。这些项目按实际证据记录，不能由构建成功推断通过。

当前产物未做商业代码签名，属于试用构建。如果组织策略阻止运行，应按组织的正常软件审批流程处理，不关闭系统保护来绕过。

## 结构

- `Launcher.cs`：托盘、单实例、本机用户限定的命名管道、默认浏览器打开与子进程生命周期。
- `host.mjs`：直接复用现有服务端，使用私有 stdin/stdout 交接。收到关闭指令或父进程管道关闭后停止服务。
- `Build.ps1` / `installer.iss`：校验并打包官方 Node、编译原生启动器、生成当前用户安装器。
- `Test.ps1`：打包后的应用自测；不采集用户文件。

参考：[Microsoft .NET Framework 安装说明](https://learn.microsoft.com/en-us/dotnet/framework/install/on-windows-10)、[命令行 C# 编译器](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/compiler-options/)、[GitHub Windows runner 软件清单](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-Readme.md)、[Inno Setup 当前用户安装](https://jrsoftware.org/ishelp/topic_setup_privilegesrequired.htm)。
