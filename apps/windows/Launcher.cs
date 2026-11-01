// Compiled with the .NET Framework compiler shipped by Windows; C# 5 syntax.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.IO.Pipes;
using System.Net;
using System.Security.AccessControl;
using System.Security.Cryptography;
using System.Security.Principal;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using System.Web.Script.Serialization;
using System.Windows.Forms;

namespace LanRelayDesktop
{
    internal static class Program
    {
        internal const string MutexName = "Local\\LanRelay.Desktop";
        internal static readonly string PipeName = MakePipeName();
        internal static int ExitStatus;
        internal static string SelfTestReport;

        private static string MakePipeName()
        {
            using (SHA256 hash = SHA256.Create())
            {
                string sid = WindowsIdentity.GetCurrent().User.Value;
                return "LanRelay.Desktop." + BitConverter.ToString(hash.ComputeHash(Encoding.UTF8.GetBytes(sid))).Replace("-", "").Substring(0, 24);
            }
        }

        [STAThread]
        private static int Main(string[] args)
        {
            bool noBrowser = false;
            bool selfTest = false;
            if (args.Length == 1 && args[0] == "--probe-existing") return SendCommand('P', 5000) ? 0 : 1;
            if (args.Length == 1 && args[0] == "--shutdown-existing") return SendCommand('Q', 5000) ? 0 : 1;
            if (args.Length == 1 && args[0] == "--no-browser") noBrowser = true;
            else if (args.Length == 2 && args[0] == "--self-test")
            {
                selfTest = true;
                noBrowser = true;
                SelfTestReport = Path.GetFullPath(args[1]);
                if (String.IsNullOrWhiteSpace(Environment.GetEnvironmentVariable("LAN_RELAY_DATA_DIR")) || Environment.GetEnvironmentVariable("LAN_RELAY_PORT") != "0") return 64;
            }
            else if (args.Length != 0) return 64;

            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            using (Mutex mutex = new Mutex(false, MutexName))
            {
                bool owned;
                try { owned = mutex.WaitOne(0, false); }
                catch (AbandonedMutexException) { owned = true; }
                if (!owned)
                {
                    if (selfTest) return 73;
                    if (SendCommand('O', 15000)) return 0;
                    MessageBox.Show("随手传已经运行，但暂时没有响应。请通过右下角托盘退出后重新打开。", "随手传", MessageBoxButtons.OK, MessageBoxIcon.Information);
                    return 1;
                }
                try
                {
                    Application.Run(new RelayContext(noBrowser, selfTest));
                    return ExitStatus;
                }
                catch
                {
                    if (!selfTest) MessageBox.Show("桌面应用未能启动。请确认安装文件完整，且系统已安装 .NET Framework 4.8。", "随手传", MessageBoxButtons.OK, MessageBoxIcon.Error);
                    return 1;
                }
                finally { mutex.ReleaseMutex(); }
            }
        }

        internal static bool SendCommand(char command, int timeout)
        {
            Stopwatch timer = Stopwatch.StartNew();
            do
            {
                try
                {
                    using (NamedPipeClientStream pipe = new NamedPipeClientStream(".", PipeName, PipeDirection.InOut, PipeOptions.Asynchronous))
                    {
                        pipe.Connect(Math.Min(400, Math.Max(1, timeout - (int)timer.ElapsedMilliseconds)));
                        pipe.WriteByte((byte)command);
                        pipe.Flush();
                        byte[] reply = new byte[1];
                        Task<int> read = pipe.ReadAsync(reply, 0, 1);
                        if (!read.Wait(Math.Min(1500, Math.Max(1, timeout - (int)timer.ElapsedMilliseconds)))) return false;
                        return read.Result == 1 && reply[0] == (byte)'K';
                    }
                }
                catch (TimeoutException) { }
                catch (IOException) { }
                catch (UnauthorizedAccessException) { return false; }
                catch (AggregateException) { return false; }
                if (timer.ElapsedMilliseconds < timeout) Thread.Sleep(80);
            } while (timer.ElapsedMilliseconds < timeout);
            return false;
        }
    }

    internal sealed class RelayContext : ApplicationContext
    {
        private readonly Control dispatcher = new Control();
        private readonly bool noBrowser;
        private readonly bool selfTest;
        private readonly string dataDir;
        private readonly string executable;
        private readonly JavaScriptSerializer json = new JavaScriptSerializer();
        private Process server;
        private NamedPipeServerStream activePipe;
        private readonly object pipeGate = new object();
        private NotifyIcon tray;
        private ToolStripMenuItem statusItem;
        private System.Windows.Forms.Timer startupTimer;
        private string ownerUrl;
        private string safeError;
        private bool ready;
        private bool stopping;
        private bool openRequested = true;
        private bool smokePassed;
        private int serverPid;
        private int serverPort;

        internal RelayContext(bool noBrowser, bool selfTest)
        {
            this.noBrowser = noBrowser;
            this.selfTest = selfTest;
            executable = Application.ExecutablePath;
            string configured = Environment.GetEnvironmentVariable("LAN_RELAY_DATA_DIR");
            dataDir = Path.GetFullPath(String.IsNullOrWhiteSpace(configured) ? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "LanRelay", "data") : configured);
            // A hidden control supplies one UI dispatcher without a persistent browser engine.
            IntPtr handle = dispatcher.Handle;
            if (!selfTest) CreateTray();
            Task.Run((Func<Task>)ListenForReopen);
            dispatcher.BeginInvoke(new Action(StartService));
        }

        private void CreateTray()
        {
            ContextMenuStrip menu = new ContextMenuStrip();
            statusItem = new ToolStripMenuItem("正在启动本地服务…");
            statusItem.Enabled = false;
            menu.Items.Add(statusItem);
            menu.Items.Add(new ToolStripSeparator());
            menu.Items.Add("打开随手传", null, delegate { OpenBrowser(); });
            menu.Items.Add("打开数据目录", null, delegate
            {
                if (Directory.Exists(dataDir)) Process.Start(new ProcessStartInfo(dataDir) { UseShellExecute = true });
            });
            menu.Items.Add(new ToolStripSeparator());
            menu.Items.Add("退出并停止服务", null, async delegate { await StopService(); });
            tray = new NotifyIcon();
            tray.Icon = Icon.ExtractAssociatedIcon(executable) ?? SystemIcons.Application;
            tray.Text = "随手传 · 正在启动";
            tray.ContextMenuStrip = menu;
            tray.Visible = true;
            tray.DoubleClick += delegate { OpenBrowser(); };
        }

        private void OnUi(Action callback)
        {
            if (stopping || dispatcher.IsDisposed) return;
            try { dispatcher.BeginInvoke(callback); }
            catch (InvalidOperationException) { }
        }

        private async Task ListenForReopen()
        {
            while (!stopping)
            {
                try
                {
                    PipeSecurity security = new PipeSecurity();
                    security.SetAccessRuleProtection(true, false);
                    security.AddAccessRule(new PipeAccessRule(new SecurityIdentifier(WellKnownSidType.NetworkSid, null), PipeAccessRights.FullControl, AccessControlType.Deny));
                    security.AddAccessRule(new PipeAccessRule(WindowsIdentity.GetCurrent().User, PipeAccessRights.FullControl, AccessControlType.Allow));
                    using (NamedPipeServerStream pipe = new NamedPipeServerStream(Program.PipeName, PipeDirection.InOut, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous, 1024, 1024, security))
                    {
                        lock (pipeGate) { activePipe = pipe; }
                        await pipe.WaitForConnectionAsync().ConfigureAwait(false);
                        byte[] command = new byte[1];
                        Task<int> read = pipe.ReadAsync(command, 0, 1);
                        if (await Task.WhenAny(read, Task.Delay(5000)).ConfigureAwait(false) != read || await read.ConfigureAwait(false) != 1) continue;
                        if (command[0] != (byte)'O' && command[0] != (byte)'P' && command[0] != (byte)'Q') continue;
                        await pipe.WriteAsync(new byte[] { (byte)'K' }, 0, 1).ConfigureAwait(false);
                        await pipe.FlushAsync().ConfigureAwait(false);
                        if (command[0] == (byte)'O') OnUi(OpenBrowser);
                        else if (command[0] == (byte)'Q') OnUi(async delegate { await StopService(); });
                    }
                }
                catch (ObjectDisposedException) { if (stopping) return; }
                catch (IOException) { if (stopping) return; }
                catch (UnauthorizedAccessException) { OnUi(delegate { Fail("本机启动器通信权限不足，请退出后重新打开。"); }); return; }
                finally { lock (pipeGate) { activePipe = null; } }
            }
        }

        private void StartService()
        {
            string root = Path.GetDirectoryName(executable);
            string node = Path.Combine(root, "runtime", "node.exe");
            string host = Path.Combine(root, "app", "apps", "windows", "host.mjs");
            if (!File.Exists(node) || !File.Exists(host)) { Fail("安装文件不完整。请重新解压完整便携包，或重新运行安装程序。"); return; }
            Directory.CreateDirectory(dataDir);
            ProcessStartInfo start = new ProcessStartInfo(node, "\"" + host + "\"");
            start.WorkingDirectory = root;
            start.UseShellExecute = false;
            start.CreateNoWindow = true;
            start.RedirectStandardInput = true;
            start.RedirectStandardOutput = true;
            start.RedirectStandardError = true;
            start.StandardOutputEncoding = Encoding.UTF8;
            start.StandardErrorEncoding = Encoding.UTF8;
            start.EnvironmentVariables["LAN_RELAY_DATA_DIR"] = dataDir;
            start.EnvironmentVariables["NODE_NO_WARNINGS"] = "1";
            // Node's diagnostic channel is consumed privately; never print owner links.
            server = new Process { StartInfo = start, EnableRaisingEvents = true };
            server.OutputDataReceived += delegate(object sender, DataReceivedEventArgs e)
            {
                if (String.IsNullOrEmpty(e.Data)) return;
                string line = e.Data;
                OnUi(delegate { ReadHostMessage(line); });
            };
            server.ErrorDataReceived += delegate { /* Diagnostics may contain local paths; no automatic logging. */ };
            server.Exited += delegate { OnUi(delegate { if (!stopping) Fail(safeError ?? "本地服务已停止。请退出随手传，再双击桌面图标重新启动。"); }); };
            try
            {
                server.Start();
                serverPid = server.Id;
                server.BeginOutputReadLine();
                server.BeginErrorReadLine();
                startupTimer = new System.Windows.Forms.Timer();
                startupTimer.Interval = 30000;
                startupTimer.Tick += delegate { Fail("本地服务启动超时。请检查端口是否被占用，以及数据目录是否可访问。"); };
                startupTimer.Start();
            }
            catch { Fail("无法启动随包运行时。请确认安装文件完整，并允许此应用在自己的专用网络中使用。"); }
        }

        private void ReadHostMessage(string line)
        {
            try
            {
                Dictionary<string, object> message = json.Deserialize<Dictionary<string, object>>(line);
                object eventValue;
                if (!message.TryGetValue("event", out eventValue)) return;
                string eventName = eventValue as string;
                if (eventName == "error")
                {
                    object text;
                    safeError = message.TryGetValue("message", out text) ? text as string : null;
                    Fail(safeError ?? "本地服务未能启动。");
                }
                else if (eventName == "ready")
                {
                    Uri parsed;
                    string url = message["url"] as string;
                    if (!Uri.TryCreate(url, UriKind.Absolute, out parsed) || parsed.Scheme != "http" || parsed.Host != "127.0.0.1" || parsed.UserInfo.Length != 0 || !parsed.Fragment.StartsWith("#owner=", StringComparison.Ordinal)) { Fail("本地服务返回了无效的管理入口。"); return; }
                    ownerUrl = url;
                    serverPort = parsed.Port;
                    ready = true;
                    startupTimer.Stop();
                    if (tray != null)
                    {
                        tray.Text = "随手传 · 服务运行中";
                        statusItem.Text = "本地服务运行中 · 端口 " + serverPort;
                    }
                    if (selfTest) { Task.Run((Func<Task>)RunSelfTest); }
                    else if (openRequested) OpenBrowser();
                }
            }
            catch { Fail("无法读取本地服务的启动状态。请重新安装完整应用包。"); }
        }

        private void OpenBrowser()
        {
            openRequested = true;
            if (!ready || noBrowser || stopping) return;
            try { Process.Start(new ProcessStartInfo(ownerUrl) { UseShellExecute = true }); }
            catch { MessageBox.Show("无法打开默认浏览器。请先在 Windows 中设置默认浏览器，再从托盘选择“打开随手传”。", "随手传", MessageBoxButtons.OK, MessageBoxIcon.Information); }
        }

        private void Fail(string message)
        {
            if (stopping) return;
            Program.ExitStatus = 1;
            if (startupTimer != null) startupTimer.Stop();
            if (!selfTest) MessageBox.Show(message, "随手传", MessageBoxButtons.OK, MessageBoxIcon.Error);
            StopService();
        }

        private async Task RunSelfTest()
        {
            try
            {
                HttpWebRequest health = (HttpWebRequest)WebRequest.Create("http://127.0.0.1:" + serverPort + "/api/health");
                health.Proxy = null;
                health.Timeout = 5000;
                using (WebResponse response = health.GetResponse())
                using (StreamReader reader = new StreamReader(response.GetResponseStream()))
                {
                    Dictionary<string, object> body = new JavaScriptSerializer().Deserialize<Dictionary<string, object>>(reader.ReadToEnd());
                    if (!body.ContainsKey("ok") || !Object.Equals(body["ok"], true)) throw new InvalidOperationException();
                }
                HttpWebRequest protectedRequest = (HttpWebRequest)WebRequest.Create("http://127.0.0.1:" + serverPort + "/api/items");
                protectedRequest.Proxy = null;
                protectedRequest.Timeout = 5000;
                bool denied = false;
                try { using (WebResponse response = protectedRequest.GetResponse()) { } }
                catch (WebException error) { using (HttpWebResponse response = error.Response as HttpWebResponse) { denied = response != null && response.StatusCode == HttpStatusCode.Unauthorized; } }
                if (!denied) throw new InvalidOperationException();
                foreach (string argument in new string[] { "--probe-existing", "--no-browser" })
                {
                    using (Process second = Process.Start(new ProcessStartInfo(executable, argument) { UseShellExecute = false, CreateNoWindow = true }))
                    {
                        if (!second.WaitForExit(10000)) { second.Kill(); throw new InvalidOperationException(); }
                        if (second.ExitCode != 0 || server.HasExited || server.Id != serverPid) throw new InvalidOperationException();
                    }
                }
                smokePassed = true;
            }
            catch { Program.ExitStatus = 1; }
            finally { OnUi(async delegate { await StopService(); }); }
            await Task.FromResult(0);
        }

        private async Task StopService()
        {
            if (stopping) return;
            stopping = true;
            if (startupTimer != null) { startupTimer.Stop(); startupTimer.Dispose(); }
            lock (pipeGate) { if (activePipe != null) activePipe.Dispose(); }
            bool graceful = true;
            Process child = server;
            if (child != null)
            {
                try
                {
                    if (!child.HasExited)
                    {
                        child.StandardInput.WriteLine("shutdown");
                        child.StandardInput.Flush();
                        child.StandardInput.Close();
                        if (!await Task.Run(delegate { return child.WaitForExit(12000); }))
                        {
                            graceful = false;
                            child.Kill();
                            await Task.Run(delegate { child.WaitForExit(5000); });
                        }
                    }
                    if (child.ExitCode != 0) graceful = false;
                }
                catch { graceful = false; try { if (!child.HasExited) child.Kill(); } catch { } }
            }
            if (!graceful) Program.ExitStatus = 1;
            if (selfTest)
            {
                try
                {
                    bool childExited = child != null && child.HasExited;
                    bool passed = smokePassed && graceful && childExited && Program.ExitStatus == 0;
                    if (!passed) Program.ExitStatus = 1;
                    Directory.CreateDirectory(Path.GetDirectoryName(Program.SelfTestReport));
                    File.WriteAllText(Program.SelfTestReport, new JavaScriptSerializer().Serialize(new { passed = passed, health = smokePassed, unauthorizedDenied = smokePassed, reopenReusedService = smokePassed, gracefulShutdown = graceful, childExited = childExited, port = serverPort }), new UTF8Encoding(false));
                }
                catch { Program.ExitStatus = 1; }
            }
            if (tray != null) { tray.Visible = false; tray.Icon.Dispose(); tray.Dispose(); }
            if (child != null) child.Dispose();
            dispatcher.Dispose();
            ExitThread();
        }
    }
}
