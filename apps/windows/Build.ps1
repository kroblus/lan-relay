[CmdletBinding()]
param(
    [string]$NodeDirectory,
    [string]$NodeZip,
    [string]$NodeZipSha256,
    [string]$OutputDirectory,
    [switch]$SkipInstaller
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repo 'output/windows' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$null = New-Item -ItemType Directory -Force -Path $OutputDirectory
$package = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $repo 'package.json') | ConvertFrom-Json
$version = [string]$package.version
if ($version -notmatch '^\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?$') { throw 'Package version must be a safe SemVer.' }
if ($NodeZip -and $NodeDirectory) { throw 'Choose NodeDirectory or NodeZip, not both.' }
$nodeStage = $null
try {
    if ($NodeZip) {
        if ($NodeZipSha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'NodeZipSha256 must be the official SHA-256 of the exact Node ZIP.' }
        $NodeZip = (Resolve-Path -LiteralPath $NodeZip).Path
        if ((Get-FileHash -LiteralPath $NodeZip -Algorithm SHA256).Hash -ne $NodeZipSha256) { throw 'Node ZIP SHA-256 does not match; refusing to extract or execute it.' }
        $nodeStage = Join-Path $OutputDirectory ('node-stage-' + [Guid]::NewGuid().ToString('N'))
        Expand-Archive -LiteralPath $NodeZip -DestinationPath $nodeStage
        $nodes = @(Get-ChildItem -LiteralPath $nodeStage -Filter 'node.exe' -Recurse -File)
        if ($nodes.Count -ne 1) { throw 'Expected exactly one node.exe in the official Windows distribution.' }
        $NodeDirectory = $nodes[0].DirectoryName
    }
    if (-not $NodeDirectory) { throw 'Supply a verified NodeDirectory or NodeZip with its official NodeZipSha256.' }
    $NodeDirectory = (Resolve-Path -LiteralPath $NodeDirectory).Path
    $nodeExe = Join-Path $NodeDirectory 'node.exe'
    $nodeLicense = Join-Path $NodeDirectory 'LICENSE'
    if (-not (Test-Path -LiteralPath $nodeExe -PathType Leaf) -or -not (Test-Path -LiteralPath $nodeLicense -PathType Leaf)) { throw 'The Node distribution must contain node.exe and LICENSE.' }
    $nodeVersion = (& $nodeExe -p 'process.version').Trim()
    if ($LASTEXITCODE -ne 0 -or $nodeVersion -notmatch '^v24\.') { throw 'Use a verified Node.js 24 Windows x64 distribution.' }
    $nodeArch = (& $nodeExe -p 'process.arch').Trim()
    if ($LASTEXITCODE -ne 0 -or $nodeArch -ne 'x64') { throw 'This Windows package requires the x64 Node distribution.' }

    $payload = Join-Path $OutputDirectory 'payload'
    if (Test-Path -LiteralPath $payload) {
        if (-not (Test-Path -LiteralPath (Join-Path $payload '.lan-relay-build'))) { throw 'Existing payload directory has no build marker; refusing to replace it.' }
        Remove-Item -LiteralPath $payload -Recurse -Force
    }
    $null = New-Item -ItemType Directory -Force -Path $payload, (Join-Path $payload 'runtime'), (Join-Path $payload 'app/apps/windows')
    Set-Content -LiteralPath (Join-Path $payload '.lan-relay-build') -Value 'Generated LAN Relay package, not user data.' -Encoding utf8
    Copy-Item -LiteralPath $nodeExe -Destination (Join-Path $payload 'runtime/node.exe')
    Copy-Item -LiteralPath $nodeLicense -Destination (Join-Path $payload 'runtime/LICENSE.node.txt')
    foreach ($directory in @('src', 'public')) { Copy-Item -LiteralPath (Join-Path $repo $directory) -Destination (Join-Path $payload 'app') -Recurse }
    Copy-Item -LiteralPath (Join-Path $repo 'package.json') -Destination (Join-Path $payload 'app/package.json')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'host.mjs') -Destination (Join-Path $payload 'app/apps/windows/host.mjs')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'LanRelay.exe.config') -Destination $payload
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'README.md') -Destination (Join-Path $payload 'README-WINDOWS.txt')

    # The icon is an original code-drawn version of icon.svg; no bitmap dependency.
    Add-Type -AssemblyName System.Drawing
    $bitmap = New-Object System.Drawing.Bitmap 256, 256
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $shape = New-Object Drawing.Drawing2D.GraphicsPath
    $shape.AddArc(8, 8, 104, 104, 180, 90)
    $shape.AddArc(144, 8, 104, 104, 270, 90)
    $shape.AddArc(144, 144, 104, 104, 0, 90)
    $shape.AddArc(8, 144, 104, 104, 90, 90)
    $shape.CloseFigure()
    $brush = New-Object Drawing.SolidBrush ([Drawing.ColorTranslator]::FromHtml('#e87558'))
    $graphics.FillPath($brush, $shape)
    $pen = New-Object Drawing.Pen ([Drawing.ColorTranslator]::FromHtml('#fffefb')), 17
    $pen.StartCap = $pen.EndCap = [Drawing.Drawing2D.LineCap]::Round
    $pen.LineJoin = [Drawing.Drawing2D.LineJoin]::Round
    $graphics.DrawLine($pen, 73, 183, 182, 74)
    $graphics.DrawLines($pen, [Drawing.Point[]]@([Drawing.Point]::new(102,74), [Drawing.Point]::new(182,74), [Drawing.Point]::new(182,154)))
    $png = New-Object IO.MemoryStream
    $bitmap.Save($png, [Drawing.Imaging.ImageFormat]::Png)
    $bytes = $png.ToArray()
    $iconPath = Join-Path $payload 'LanRelay.ico'
    $iconFile = [IO.File]::Create($iconPath)
    $writer = New-Object IO.BinaryWriter $iconFile
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]1)
    $writer.Write([byte]0); $writer.Write([byte]0); $writer.Write([byte]0); $writer.Write([byte]0)
    $writer.Write([uint16]1); $writer.Write([uint16]32); $writer.Write([uint32]$bytes.Length); $writer.Write([uint32]22); $writer.Write($bytes)
    $writer.Dispose(); $png.Dispose(); $pen.Dispose(); $brush.Dispose(); $shape.Dispose(); $graphics.Dispose(); $bitmap.Dispose()

    $csc = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
    if (-not (Test-Path -LiteralPath $csc)) { throw '.NET Framework compiler not found; build on Windows with .NET Framework 4.8.' }
    $numericVersion = ([regex]::Match($version, '^\d+\.\d+\.\d+')).Value + '.0'
    $assemblyInfo = Join-Path $OutputDirectory 'AssemblyInfo.cs'
    [IO.File]::WriteAllText($assemblyInfo, "using System.Reflection;`n[assembly: AssemblyTitle(`"LAN Relay`") ]`n[assembly: AssemblyProduct(`"\u968f\u624b\u4f20`") ]`n[assembly: AssemblyVersion(`"$numericVersion`") ]`n[assembly: AssemblyFileVersion(`"$numericVersion`") ]`n", [Text.UTF8Encoding]::new($false))
    $compilerArgs = @('/nologo', '/target:winexe', '/platform:x64', '/optimize+', '/codepage:65001', ('/out:' + (Join-Path $payload 'LanRelay.exe')), ('/win32icon:' + $iconPath), ('/win32manifest:' + (Join-Path $PSScriptRoot 'app.manifest')), '/reference:System.dll', '/reference:System.Core.dll', '/reference:System.Drawing.dll', '/reference:System.Windows.Forms.dll', '/reference:System.Web.Extensions.dll', (Join-Path $PSScriptRoot 'Launcher.cs'), $assemblyInfo)
    & $csc @compilerArgs
    if ($LASTEXITCODE -ne 0) { throw 'Windows launcher compilation failed.' }

    $commit = 'unknown'
    $dirty = $true
    if (Get-Command git -ErrorAction SilentlyContinue) {
        $commit = (& git -C $repo rev-parse HEAD).Trim()
        $dirty = [bool](& git -C $repo status --porcelain --untracked-files=no)
    }
    [ordered]@{ version=$version; sourceCommit=$commit; sourceWorkingTreeDirty=$dirty; nodeVersion=$nodeVersion; nodeSha256=(Get-FileHash -LiteralPath $nodeExe -Algorithm SHA256).Hash.ToLowerInvariant(); architecture='x64'; signed=$false } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $payload 'build-info.json') -Encoding utf8
    $portable = Join-Path $OutputDirectory "LanRelay-$version-windows-x64-portable.zip"
    Compress-Archive -Path (Join-Path $payload '*') -DestinationPath $portable -Force
    $artifacts = @($portable)
    if (-not $SkipInstaller) {
        $iscc = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6/ISCC.exe'
        if (-not (Test-Path -LiteralPath $iscc)) {
            $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
            if ($command) { $iscc = $command.Source } else { throw 'Inno Setup 6 was not found. Install it from jrsoftware.org or use the configured Windows CI build.' }
        }
        & $iscc ('/DAppVersion=' + $version) ('/DPayloadDir=' + $payload) ('/DOutputDirectory=' + $OutputDirectory) (Join-Path $PSScriptRoot 'installer.iss')
        if ($LASTEXITCODE -ne 0) { throw 'Windows installer compilation failed.' }
        $artifacts += Join-Path $OutputDirectory "LanRelay-$version-windows-x64-setup.exe"
    }
    $checksums = foreach ($artifact in $artifacts) { (Get-FileHash -LiteralPath $artifact -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + [IO.Path]::GetFileName($artifact) }
    $checksums | Set-Content -LiteralPath (Join-Path $OutputDirectory 'SHA256SUMS.txt') -Encoding utf8
    Write-Host "Built LAN Relay $version for Windows x64. Run apps/windows/Test.ps1 against $payload before distributing."
} finally {
    if ($nodeStage -and (Test-Path -LiteralPath $nodeStage)) { Remove-Item -LiteralPath $nodeStage -Recurse -Force }
}
