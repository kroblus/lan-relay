[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$AppDirectory, [string]$EvidenceDirectory)
$ErrorActionPreference = 'Stop'
$AppDirectory = (Resolve-Path -LiteralPath $AppDirectory).Path
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not $EvidenceDirectory) { $EvidenceDirectory = Join-Path $repo ('output/verification/windows-' + [Guid]::NewGuid().ToString('N')) }
$EvidenceDirectory = [IO.Path]::GetFullPath($EvidenceDirectory)
$null = New-Item -ItemType Directory -Force -Path $EvidenceDirectory
$dataDir = Join-Path $EvidenceDirectory 'data'
$report = Join-Path $EvidenceDirectory 'launcher.json'
$oldData = $env:LAN_RELAY_DATA_DIR
$oldPort = $env:LAN_RELAY_PORT
$process = $null
try {
    $env:LAN_RELAY_DATA_DIR = $dataDir
    $env:LAN_RELAY_PORT = '0'
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = Join-Path $AppDirectory 'LanRelay.exe'
    $start.Arguments = '--self-test "' + $report + '"'
    $start.WorkingDirectory = $AppDirectory
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $process = [Diagnostics.Process]::Start($start)
    if (-not $process.WaitForExit(45000)) { $process.Kill(); throw 'Launcher self-test timed out.' }
    if ($process.ExitCode -ne 0) { throw ('Launcher self-test exited with code ' + $process.ExitCode) }
    if (-not (Test-Path -LiteralPath $report)) { throw 'Launcher did not produce its non-secret report.' }
    $result = Get-Content -Raw -LiteralPath $report | ConvertFrom-Json
    foreach ($name in @('passed','health','unauthorizedDenied','reopenReusedService','gracefulShutdown','childExited')) { if ($result.$name -ne $true) { throw "Launcher check failed: $name" } }
    if (Test-Path -LiteralPath (Join-Path $dataDir 'process-lock')) { throw 'Service lock remains after graceful shutdown.' }
    $client = New-Object Net.Sockets.TcpClient
    try {
        $connected = $false
        try { $task = $client.ConnectAsync('127.0.0.1', [int]$result.port); if ($task.Wait(1500)) { $connected = $client.Connected } } catch { }
        if ($connected) { throw 'Service port is still listening after shutdown.' }
    } finally { $client.Dispose() }
    Write-Host 'PASS: Windows launcher started the bundled runtime, served real HTTP, rejected an unpaired request, reused the existing instance twice, and shut down without leaving a server or lock.'
    Write-Host ('Evidence: ' + $report)
} finally {
    $env:LAN_RELAY_DATA_DIR = $oldData
    $env:LAN_RELAY_PORT = $oldPort
    if ($process) { $process.Dispose() }
}
