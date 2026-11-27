<#
.SYNOPSIS
  Builds the SSD Sentinel Windows x64 portable ZIP (no installer, all data stays in .\data).

.DESCRIPTION
  Stages ssd-sentinel.exe, a portable.txt marker, smartmontools, licences and a short guide,
  then creates SSD-Sentinel_<version>_x64-portable.zip and verifies the archive contents.
  Works on Windows PowerShell 5.1 and PowerShell 7.
#>
param(
  [string]$Version = "",
  [string]$ExePath = "src-tauri\target\release\ssd-sentinel.exe",
  [string]$SmartmontoolsDir = "tools\smartmontools",
  [string]$OutDir = "release-assets"
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
  if (-not $Version) {
    $conf = Get-Content -Raw -Path "src-tauri\tauri.conf.json" | ConvertFrom-Json
    $Version = $conf.version
  }
  if (-not $Version) { throw "Could not determine the application version." }

  function Require-File([string]$path, [string]$what, [switch]$NonEmpty) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing $what at '$path'." }
    if ($NonEmpty -and ((Get-Item -LiteralPath $path).Length -le 0)) { throw "$what at '$path' is empty." }
  }

  $smartctl = Join-Path $SmartmontoolsDir "bin\smartctl.exe"
  $copying = Join-Path $SmartmontoolsDir "doc\COPYING.txt"
  Require-File $ExePath "ssd-sentinel.exe (build it with: npm run tauri -- build --no-bundle)" -NonEmpty
  Require-File $smartctl "smartctl.exe" -NonEmpty
  Require-File $copying "smartmontools COPYING.txt"
  Require-File "LICENSE" "LICENSE"
  Require-File "THIRD_PARTY.md" "THIRD_PARTY.md"

  $stage = Join-Path ([System.IO.Path]::GetTempPath()) ("ssd-sentinel-portable-" + [guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Force -Path $stage | Out-Null
  try {
    Copy-Item -LiteralPath $ExePath -Destination (Join-Path $stage "ssd-sentinel.exe")
    Copy-Item -LiteralPath "LICENSE" -Destination (Join-Path $stage "LICENSE")
    Copy-Item -LiteralPath "THIRD_PARTY.md" -Destination (Join-Path $stage "THIRD_PARTY.md")
    New-Item -ItemType Directory -Force -Path (Join-Path $stage "smartmontools\bin"), (Join-Path $stage "smartmontools\doc") | Out-Null
    Copy-Item -LiteralPath $smartctl -Destination (Join-Path $stage "smartmontools\bin\smartctl.exe")
    $drivedb = Join-Path $SmartmontoolsDir "bin\drivedb.h"
    if (Test-Path -LiteralPath $drivedb) { Copy-Item -LiteralPath $drivedb -Destination (Join-Path $stage "smartmontools\bin\drivedb.h") }
    Copy-Item -LiteralPath $copying -Destination (Join-Path $stage "smartmontools\doc\COPYING.txt")
    $sums = Join-Path $SmartmontoolsDir "doc\checksums64.txt"
    if (Test-Path -LiteralPath $sums) { Copy-Item -LiteralPath $sums -Destination (Join-Path $stage "smartmontools\doc\checksums64.txt") }

    $utf8 = New-Object System.Text.UTF8Encoding($false)
    $marker = "SSD Sentinel portable mode marker.`r`nKeep this file next to ssd-sentinel.exe. While it exists, all data is stored in the .\data folder.`r`nBu dosya ssd-sentinel.exe ile ayni klasorde kaldigi surece tum veriler .\data klasorunde tutulur.`r`n"
    [System.IO.File]::WriteAllText((Join-Path $stage "portable.txt"), $marker, $utf8)

    $guide = @"
SSD Sentinel $Version - Portable (x64)
=======================================

ENGLISH
  1. Extract this ZIP anywhere (USB stick, Desktop, D:\Tools ...).
  2. Run ssd-sentinel.exe. For SMART access and maintenance use "Run as Admin" inside the app.
  3. Everything stays inside this folder, nothing is written to %LOCALAPPDATA%:
       data\history.sqlite3     measurement history and rollback records
       data\preferences.json    language, theme, Read-Only Guard, update settings
       data\webview2\           WebView2 browser profile
       data\updates\            verified update packages (SHA-256) when you update from inside the app
  4. To move the app, close it and move the whole folder. To uninstall, delete the folder.
  5. To update: use "Check for updates" in Tools & Settings. The verified portable ZIP is downloaded to
     data\updates and shown in Explorer; extract it over this folder and keep your data folder.
  Notes: Microsoft Edge WebView2 Runtime is required (present on Windows 11 and most Windows 10).
         The optional "launch on Windows startup" setting writes a Run entry in HKCU for this exe path.
         Verify downloads against SHA256SUMS.txt from the GitHub release page.

TURKCE
  1. ZIP dosyasini istediginiz yere (USB bellek, Masaustu, D:\Araclar ...) cikarin.
  2. ssd-sentinel.exe dosyasini calistirin. SMART erisimi ve bakim icin uygulama icinden "Yonetici olarak ac" kullanin.
  3. Her sey bu klasorde kalir, %LOCALAPPDATA% icine hicbir sey yazilmaz:
       data\history.sqlite3     olcum gecmisi ve geri alma kayitlari
       data\preferences.json    dil, tema, Salt-Okunur Koruma, guncelleme ayarlari
       data\webview2\           WebView2 tarayici profili
       data\updates\            uygulama icinden guncellerken indirilen dogrulanmis paketler (SHA-256)
  4. Tasimak icin uygulamayi kapatip klasorun tamamini tasiyin. Kaldirmak icin klasoru silin.
  5. Guncellemek icin Bakim ve Ayarlar > "Guncellemeleri kontrol et" kullanin. Dogrulanmis tasinabilir ZIP
     data\updates icine indirilir ve Gezgin'de gosterilir; bu klasorun uzerine cikarin, data klasorunu koruyun.

Project / Proje: https://github.com/yuefdev/ssd-sentinel
Licence: MIT. smartmontools (smartctl.exe) is GPL-2.0-or-later, see THIRD_PARTY.md and smartmontools\doc\COPYING.txt.
"@
    [System.IO.File]::WriteAllText((Join-Path $stage "README-PORTABLE.txt"), ($guide -replace "`r?`n", "`r`n"), $utf8)

    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $zipPath = Join-Path (Resolve-Path $OutDir).ProviderPath ("SSD-Sentinel_{0}_x64-portable.zip" -f $Version)
    if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
    [System.IO.Compression.ZipFile]::CreateFromDirectory($stage, $zipPath, [System.IO.Compression.CompressionLevel]::Optimal, $false)

    $required = @(
      "ssd-sentinel.exe", "portable.txt", "README-PORTABLE.txt", "LICENSE", "THIRD_PARTY.md",
      "smartmontools/bin/smartctl.exe", "smartmontools/doc/COPYING.txt"
    )
    $zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
    try {
      $names = @($zip.Entries | ForEach-Object { $_.FullName.Replace("\", "/") })
      foreach ($r in $required) {
        if ($names -notcontains $r) { throw "Portable ZIP is missing required entry '$r'." }
      }
    } finally { $zip.Dispose() }

    $item = Get-Item -LiteralPath $zipPath
    $hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "Portable ZIP : $($item.FullName)"
    Write-Host "Size         : $($item.Length) bytes"
    Write-Host "SHA-256      : $hash"
  } finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
  }
} finally {
  Pop-Location
}
