@echo off
if not exist "%~dp0src-tauri\target\release\ssd-sentinel.exe" (
  echo Rust uygulamasi bulunamadi. Once npm install ve npm run desktop:build calistirin.
  pause
  exit /b 1
)
start "" "%~dp0src-tauri\target\release\ssd-sentinel.exe" %*
