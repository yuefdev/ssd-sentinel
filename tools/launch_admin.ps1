$ErrorActionPreference = 'Stop'
$entry = Join-Path (Split-Path -Parent $PSScriptRoot) 'src-tauri\target\release\ssd-sentinel.exe'
if (-not (Test-Path -LiteralPath $entry -PathType Leaf)) { throw 'Rust uygulamasi bulunamadi. npm run desktop:build calistirin.' }
Start-Process -FilePath $entry -WorkingDirectory (Split-Path -Parent $entry) -Verb RunAs
