[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$SkipInstaller
)

$ErrorActionPreference = 'Stop'
$ProjectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$Version = '0.5.0'
$OutputRoot = Join-Path $ProjectRoot "output/dist/v$Version/windows"
$PortableName = "LanRelay-$Version-windows-x64"
$OutputParent = Split-Path -Parent $OutputRoot
$StagingRoot = Join-Path $OutputParent ('.windows-staging-' + [guid]::NewGuid().ToString('N'))
$PortableDirectory = Join-Path $StagingRoot $PortableName
$ProducedFiles = [System.Collections.Generic.List[string]]::new()

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
    if (Test-Path -LiteralPath $OutputRoot) {
        $Existing = Get-Item -LiteralPath $OutputRoot
        if (-not $Existing.PSIsContainer -or ($Existing.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'Refusing to replace a non-directory or linked artifact destination.'
        }
    }
    # Every run packages into a new directory. SkipInstaller must not pick up a
    # setup executable left by an earlier build of the same product version.
    New-Item -ItemType Directory -Path $PortableDirectory -Force | Out-Null
    Copy-Item -LiteralPath $Executable -Destination (Join-Path $PortableDirectory 'lan-relay.exe') -Force
    $ProducedFiles.Add("$PortableName/lan-relay.exe")
    foreach ($Tool in @('relay-server.exe', 'relay-migrate.exe')) {
        $ToolPath = Join-Path $ProjectRoot "target/release/$Tool"
        if (-not (Test-Path -LiteralPath $ToolPath -PathType Leaf)) { throw "Missing Rust tool: $ToolPath" }
        Copy-Item -LiteralPath $ToolPath -Destination $PortableDirectory -Force
        $ProducedFiles.Add("$PortableName/$Tool")
    }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'QUICK_START.txt') -Destination $PortableDirectory -Force
    $ProducedFiles.Add("$PortableName/QUICK_START.txt")
    $Archive = Join-Path $StagingRoot "$PortableName.zip"
    Compress-Archive -Path $PortableDirectory -DestinationPath $Archive -Force
    $ProducedFiles.Add("$PortableName.zip")

    if (-not $SkipInstaller) {
        $CompilerCommand = Get-Command ISCC.exe -ErrorAction SilentlyContinue
        $Compiler = if ($CompilerCommand) { $CompilerCommand.Source } else { "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
        if (Test-Path -LiteralPath $Compiler) {
            & $Compiler "/DAppVersion=$Version" "/DSourceDirectory=$PortableDirectory" "/O$StagingRoot" (Join-Path $PSScriptRoot 'installer.iss')
            if ($LASTEXITCODE -ne 0) { throw "Installer build failed with exit code $LASTEXITCODE" }
            $InstallerName = "LanRelay-$Version-windows-setup.exe"
            if (-not (Test-Path -LiteralPath (Join-Path $StagingRoot $InstallerName) -PathType Leaf)) {
                throw "Installer compiler did not produce $InstallerName"
            }
            $ProducedFiles.Add($InstallerName)
        } else {
            Write-Warning 'Inno Setup was not found. Portable ZIP is complete; installer was not built.'
        }
    }
    @{ schema_version = 1; version = $Version; files = @($ProducedFiles.ToArray()) } |
        ConvertTo-Json -Depth 4 |
        Set-Content -LiteralPath (Join-Path $StagingRoot 'ARTIFACT_INPUTS.json') -Encoding utf8
    $Hashes = Get-ChildItem -LiteralPath $StagingRoot -File | Where-Object { $_.Extension -in '.zip', '.exe' } | ForEach-Object {
        $Hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        "$Hash  $($_.Name)"
    }
    $Hashes | Set-Content -LiteralPath (Join-Path $StagingRoot 'SHA256SUMS.txt') -Encoding ascii
    # Preserve prior delivery files only after the complete new package exists.
    # Directory.Move has exact destination semantics (never nests staging into
    # an existing destination); a failed publication attempts to restore it.
    $PreviousOutput = $null
    if (Test-Path -LiteralPath $OutputRoot) {
        $Stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
        $PreviousOutput = "$OutputRoot-previous-$Stamp-$([guid]::NewGuid().ToString('N'))"
        [IO.Directory]::Move($OutputRoot, $PreviousOutput)
    }
    try {
        [IO.Directory]::Move($StagingRoot, $OutputRoot)
    } catch {
        if ($PreviousOutput -and -not (Test-Path -LiteralPath $OutputRoot)) {
            [IO.Directory]::Move($PreviousOutput, $OutputRoot)
        }
        throw
    }
    if ($PreviousOutput) { Write-Output "Previous artifacts retained: $PreviousOutput" }
    Write-Output "Windows artifacts: $OutputRoot"
} finally {
    Pop-Location
}
