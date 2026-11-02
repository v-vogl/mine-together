@echo off
rem Play with friends over the internet: builds the client, starts the game server (serving the page too)
rem in a second window, and opens a Cloudflare quick tunnel. The tunnel prints a public link
rem (https://....trycloudflare.com) to send to friends. Close this window to take the game offline again.
rem Needs cloudflared: winget install --id Cloudflare.cloudflared

cd /d "%~dp0"

rem winget puts it on the PATH, but only for Command Prompts opened after the install.
set "CLOUDFLARED="
where cloudflared >nul 2>nul && set "CLOUDFLARED=cloudflared"
if not defined CLOUDFLARED if exist "%ProgramFiles(x86)%\cloudflared\cloudflared.exe" set "CLOUDFLARED=%ProgramFiles(x86)%\cloudflared\cloudflared.exe"
if not defined CLOUDFLARED if exist "%ProgramFiles%\cloudflared\cloudflared.exe" set "CLOUDFLARED=%ProgramFiles%\cloudflared\cloudflared.exe"
if not defined CLOUDFLARED (
  echo cloudflared was not found. Install it with:  winget install --id Cloudflare.cloudflared
  echo Then open a new Command Prompt and run share.cmd again.
  exit /b 1
)

echo Building the client...
pushd client
call npm run build
if errorlevel 1 (
  popd
  echo The client build failed.
  exit /b 1
)
popd

rem The server listens only on this PC (127.0.0.1); friends reach it through the tunnel alone.
start "Mine Together server" cmd /k "cd /d "%~dp0server" && set "HOST=127.0.0.1" && set "STATIC_DIR=%~dp0client\dist" && cargo run --release"

echo.
echo Starting the tunnel. Look for the line with  https://....trycloudflare.com  below and send that link.
echo Close this window (or press Ctrl+C) to stop sharing. The server keeps running in its own window.
echo.
"%CLOUDFLARED%" tunnel --url http://localhost:8080
