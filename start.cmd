@echo off
chcp 65001 >nul
cd /d "%~dp0"
where node >nul 2>nul
if errorlevel 1 (
  echo Please install Node.js 24 or later from https://nodejs.org/ first.
  pause
  exit /b 1
)
node src\cli.mjs
set "LAN_RELAY_EXIT=%errorlevel%"
if not "%LAN_RELAY_EXIT%"=="0" pause
exit /b %LAN_RELAY_EXIT%
