$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
  throw 'Install Node.js 24 or later from https://nodejs.org/ first.'
}
& node 'src/cli.mjs'
exit $LASTEXITCODE
