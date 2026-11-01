# 随手传 Android App（0.2 试用）

原生 Android 应用使用系统 WebView 显示电脑端的同一套随手传界面。电脑仍负责保存文件、配对与反馈记录；手机无需 Node.js。最低 Android 10，目标 Android 17（API 37）。

## 已实现

- App 内置离线二维码扫描，识别电脑网页现有的 HTTP 配对二维码。无需 Google Play 服务、相机模型下载或第三方扫码 App。
- 可粘贴电脑链接或手动输入地址；记住已经确认的电脑 origin，后续直接打开。
- 相机只在主动扫码时请求；Android 17 的局域网权限在开始连接前请求，拒绝后可继续停留在连接页。
- 电脑批准后，网页文件、文字、构建和反馈功能使用 App 自己的 WebView 会话。Chrome 与 App 的 Cookie 不共享，必须分别配对。
- 系统文件选择器支持原图、APK、录屏、普通文件及多选。通过 content:// 临时授权读取，不申请全盘、照片库或联系人权限；原始字节不转码。
- 系统分享的图片/文件可进入随手传。连接电脑后，点网页的“文件”，再确认使用这些分享文件。分享文字通过可见的“复制文字”按钮交接到网页输入框，不会自动发送。
- 下载使用当前 origin 的 Cookie，先做禁止重定向的 HEAD 检查；401/403、重定向、非附件响应均不会进入系统下载。有效文件交给系统 DownloadManager，保存于公共 Download/LANRelay 目录，并添加时间前缀避免覆盖。
- 支持 lanrelay://connect?url=<URL编码后的HTTP或HTTPS配对链接>；外部调用只能请求连接，仍须确认新电脑和完成原有配对。

## 使用顺序

1. 在电脑启动随手传，打开连接设备面板。
2. 安装试用 APK，打开“随手传·试用”。
3. 点击“扫一扫电脑上的配对码”，允许此次相机访问。Android 17 还会请求局域网访问。
4. 确认电脑地址，在网页中填写/选择设备名，提交后去电脑批准。
5. 直接使用内容流与文件按钮。其他 App 的“分享”菜单中也可选择随手传。
6. 点击文件的下载按钮后，用 App 右上角“下载”查看系统的实际结果。

没有相机或拒绝相机权限时，可粘贴电脑的连接链接；扫码不是唯一入口。不要使用电脑终端的管理者链接，手机使用网页内生成的配对链接。

## 边界与错误处理

- HTTP 传输没有加密，只用于自己的可信网络。清单允许动态 LAN IP 的 HTTP，而连接策略和 WebView 拦截器限制到用户确认的一个 origin；没有通用浏览器、JavaScript 原生桥或本机命令接口。
- 仅接受私有 IPv4、IPv6 ULA/link-local、100.64/10 地址以及单段/.local/.lan/.home.arpa 本地域名。这是地址策略，不证明网络真实路由；DNS、VPN、防火墙仍可能导致不可达。推荐电脑显示的 IPv4 地址。
- HTTPS 使用系统信任的证书；不会忽略 SSL 错误，不在 App 内导入证书。当前连接入口不接受普通公网域名。
- Web 页面不能直接获得原生相机、麦克风、位置或文件目录权限。扫码由独立原生扫码界面完成。
- “已交给系统下载”仅表示请求入队。HEAD 成功后 Cookie 仍可能被撤销、电脑可能离线；系统下载管理器的最终结果才是依据。
- APK 不会自动安装；应用不申请安装其他应用的权限。需要安装时由用户在系统文件管理器中自行操作。
- 上传继续沿用网页分片与完整 SHA 校验；没有原生后台上传服务。锁屏、切后台、系统回收进程会影响上传，返回后可重新选择原文件续传。
- 收到的分享内容只放在当前进程内存中，不自动持久化。关闭 App 或系统回收后，可能需要重新分享。
- 浏览器模拟、JVM 测试、APK 编译成功均不等于 Android 真机验收。

## 构建

已按官方稳定工具链固定：

| 工具 | 版本 |
| --- | --- |
| JDK | 17 |
| Gradle | 9.6.0 |
| Android Gradle Plugin | 9.5.0 |
| compileSdk / targetSdk | 37 |
| Build Tools | 36.0.0 |
| minSdk | 29 |

在已安装上述 JDK、Gradle 和合法配置的 Android SDK 的环境中：

~~~sh
gradle -p apps/android --no-daemon testDebugUnitTest lintDebug assembleDebug assembleRelease
~~~

- 可安装试用包：apps/android/app/build/outputs/apk/debug/app-debug.apk。
- 未签名正式构建：apps/android/app/build/outputs/apk/release/app-release-unsigned.apk，不能直接安装。
- JVM 报告：apps/android/app/build/reports/tests/testDebugUnitTest/。
- Lint 报告：apps/android/app/build/reports/lint-results-debug.html。

本工程不提交 Gradle wrapper JAR。CI 使用固定版本的 gradle/actions/setup-gradle 安装 Gradle；Android Studio/本机也可以先配置相同 Gradle 版本，或使用官方 gradle wrapper --gradle-version 9.6.0 生成 wrapper。不要为了解决下载问题而使用来源不明的 JAR。

试用包 applicationId 是 com.kroblus.lanrelay.preview，与未来正式包 com.kroblus.lanrelay 分开。CI 的 debug 签名是临时试用签名：不同干净构建可能使用不同密钥，不能保证直接覆盖安装。不要清除正在使用的 App 数据来掩盖签名冲突。正式分发前应由 Owner 在本机生成并保管长期签名密钥，用同一密钥签署后续版本；私钥和密码不得进入 Git。

## 必须补的真机验收

- Android 16 与实际 Android 17 设备的启动；Android 17 允许、拒绝、撤销局域网权限后的行为。
- 相机允许/拒绝、扫码取消、二维码过期、App 已打开时再次扫码/分享连接链接。
- App 与浏览器分别配对；电脑撤销 App 后，内容读取和下载均应失败。
- SAF 原图、中文文件名、大文件和多选；系统分享一张/多张图、APK、录屏、文本。
- 原图下载后回传，比较电脑上的 SHA-256；真实 APK 下载只证明传输，安装状态由用户确认。
- 下载预检后撤销设备或断开电脑，确认系统显示失败而非 App 宣称成功。
- 320/375/430 dp 宽度、大字体、软键盘、状态栏及导航栏不遮挡主要按钮。
- 上传途中切后台、锁屏、进程回收、刷新后重新选择同一文件续传。
- 设备无 Google Play 服务时的离线扫码。
- 安装包升级与正式签名流程；当前自动化不能替代这一项。

## 官方依据与第三方组件

- [Android 17 局域网权限](https://developer.android.com/privacy-and-security/local-network-permission)：target 37 必须动态请求；WebView 继承宿主权限。target 36 及以下不能照搬这个权限请求。
- [AGP 9.5.0 兼容表](https://developer.android.com/build/releases/agp-9-5-0-release-notes)：API 37、Gradle 9.6.0、JDK 17、Build Tools 36.0.0。
- [WebView 文件选择回调](https://developer.android.com/reference/android/webkit/WebChromeClient)：返回的 URI 必须由应用校验。
- [DownloadManager.Request](https://developer.android.com/reference/android/app/DownloadManager.Request)：请求头和下载目标接口。
- [ZXing Android Embedded](https://github.com/journeyapps/zxing-android-embedded)：4.3.0，Apache-2.0。
- [ZXing Core](https://github.com/zxing/zxing/releases)：3.5.4，Apache-2.0。
- AndroidX Activity 1.10.1，Apache-2.0。APK 包含第三方说明和 Apache 2.0 许可文本。
