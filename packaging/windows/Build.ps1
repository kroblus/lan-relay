[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$SkipInstaller
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$Version = '0.4.0'
$OutputRoot = Join-Path $ProjectRoot 'output/dist/windows'
$PortableName = "LanRelay-$Version-windows-x64"
$PortableDirectory = Join-Path $OutputRoot $PortableName

Push-Location $ProjectRoot
try {
    if (-not $SkipBuild) {
        & cargo build --locked --release -p relay-desktop -p relay-server -p relay-migrate
        if ($LASTEXITCODE -ne 0) { throw "Rust build failed with exit code $LASTEXITCODE" }
    }
    $Executable = Join-Path $ProjectRoot 'target/release/lan-relay.exe'
    if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
        throw "Missing compiled desktop app: $Executable"
    }
    New-Item -ItemType Directory -Path $PortableDirectory -Force | Out-Null
    Copy-Item -LiteralPath $Executable -Destination (Join-Path $PortableDirectory 'lan-relay.exe') -Force
    foreach ($Tool in @('relay-server.exe', 'relay-migrate.exe')) {
        $ToolPath = Join-Path $ProjectRoot "target/release/$Tool"
        if (-not (Test-Path -LiteralPath $ToolPath -PathType Leaf)) { throw "Missing Rust tool: $ToolPath" }
        Copy-Item -LiteralPath $ToolPath -Destination $PortableDirectory -Force
    }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'QUICK_START.txt') -Destination $PortableDirectory -Force
    $Archive = Join-Path $OutputRoot "$PortableName.zip"
    Compress-Archive -Path $PortableDirectory -DestinationPath $Archive -Force

    if (-not $SkipInstaller) {
        $CompilerCommand = Get-Command ISCC.exe -ErrorAction SilentlyContinue
        $Compiler = if ($CompilerCommand) { $CompilerCommand.Source } else { "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
        if (Test-Path -LiteralPath $Compiler) {
            & $Compiler "/DAppVersion=$Version" "/DSourceDirectory=$PortableDirectory" "/O$OutputRoot" (Join-Path $PSScriptRoot 'installer.iss')
            if ($LASTEXITCODE -ne 0) { throw "Installer build failed with exit code $LASTEXITCODE" }
        } else {
            Write-Warning 'Inno Setup was not found. Portable ZIP is complete; installer was not built.'
        }
    }
    $Hashes = Get-ChildItem -LiteralPath $OutputRoot -File | Where-Object { $_.Extension -in '.zip', '.exe' } | ForEach-Object {
        $Hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$Hash  $($_.Name)"
    }
    $Hashes | Set-Content -LiteralPath (Join-Path $OutputRoot 'SHA256SUMS.txt') -Encoding ascii
    Write-Output "Windows artifacts: $OutputRoot"
} finally {
    Pop-Location
}
