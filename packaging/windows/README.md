# Windows Rust packaging

Run from PowerShell with Rust 1.99.1 and the Windows MSVC build tools:

```powershell
cargo test --locked --workspace --all-targets
cargo build --locked --release -p relay-desktop -p relay-server -p relay-migrate
./packaging/windows/Build.ps1 -SkipBuild
```

The script creates a portable ZIP under `output/dist/windows/`. When Inno Setup 6.3 或更新版本
is installed it also creates a per-user installer with a desktop shortcut. The
installer requires no administrator account. Installation does not change the
firewall or add automatic startup. Runtime data remains in
`%LOCALAPPDATA%\LanRelay\v2`; uninstalling leaves it intact.

`--data-dir PATH` and `--port PORT` select explicit alternatives. Two instances
using the same directory share a file lock; the second process sends only a
filesystem show request in that user's directory. It never receives a network
owner credential or exposes an anonymous control endpoint.

For an isolated command-line lifecycle check, pass an empty test directory:

```powershell
$Smoke = Start-Process -FilePath ./target/release/lan-relay.exe -ArgumentList '--smoke-test', '--data-dir', './output/verification/desktop-smoke-fresh' -Wait -PassThru
if ($Smoke.ExitCode -ne 0) { throw "Desktop smoke failed: $($Smoke.ExitCode)" }
Get-Content ./output/verification/desktop-smoke-fresh/smoke-report.json
```

This checks single-instance locking, real HTTP `hello`, and listening socket
shutdown. It does not validate tray UI, Chinese IME, multi-display DPI, system file
dialogs, Windows installation, or an Android physical device. Those checks remain
explicit manual acceptance items until evidence is recorded.
`Start-Process -Wait` is intentional because the release executable uses the
Windows GUI subsystem. The test writes the same JSON report inside its isolated
data directory; errors return a nonzero exit code without a blocking dialog.

The desktop uses Iced 0.14's native rendering with a software rendering backend.
Windows tray handles live on a dedicated Windows message-loop thread. When tray
creation fails the main window displays that condition and closing it exits the
server, preventing an inaccessible background process.

## Importing completed V1 content

The package also includes `relay-migrate.exe`. Stop the old service normally and
back up its data directory before importing. The destination must not exist and
its parent must exist. First check the source and its file hashes without writing
a new data directory:

```powershell
./relay-migrate.exe --from "D:\LanRelayOldData" --to "$env:LOCALAPPDATA\LanRelay\v2-imported" --dry-run
./relay-migrate.exe --from "D:\LanRelayOldData" --to "$env:LOCALAPPDATA\LanRelay\v2-imported"
./lan-relay.exe --data-dir "$env:LOCALAPPDATA\LanRelay\v2-imported"
```

Replace the example source with the old data directory, not the program folder.
The importer reads completed text/files/feedback and reports its results. It does
not migrate device credentials or unfinished uploads; pair phones again. Do not
point a running desktop at the import destination during migration. The old
directory remains unchanged, and importing never merges into an existing V2
directory. See the repository migration documentation for the full field mapping
and verification process.
