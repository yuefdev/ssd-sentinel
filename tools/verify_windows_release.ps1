param(
  [Parameter(Mandatory)]
  [string]$AssetDirectory,
  [Parameter(Mandatory)]
  [ValidatePattern('^[0-9a-fA-F]{64}$')]
  [string]$SmartctlSha256,
  [string]$SevenZip = "7z"
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$config = Get-Content -LiteralPath (Join-Path $repo "src-tauri/tauri.conf.json") -Raw | ConvertFrom-Json
$assets = (Resolve-Path -LiteralPath $AssetDirectory).ProviderPath
$installer = @(Get-ChildItem -LiteralPath $assets -Filter "*-setup.exe" -File)
$portable = @(Get-ChildItem -LiteralPath $assets -Filter "*-portable.zip" -File)
if ($installer.Count -ne 1 -or $portable.Count -ne 1) {
  throw "Expected exactly one Windows installer and one portable ZIP in '$assets'."
}

function Assert-ProjectBinary([string]$Path, [string]$Label) {
  $version = (Get-Item -LiteralPath $Path).VersionInfo
  if ($version.ProductName -cne "SSD Sentinel" -or
      $version.ProductVersion -cne $config.version -or
      $version.FileVersion -cne $config.version) {
    throw "$Label has product '$($version.ProductName)', product version '$($version.ProductVersion)' and file version '$($version.FileVersion)'. Expected SSD Sentinel $($config.version)."
  }
  Write-Host ("{0}: SSD Sentinel {1}" -f $Label, $config.version)
}

$tempRoot = (Resolve-Path -LiteralPath ([IO.Path]::GetTempPath())).ProviderPath
$stage = Join-Path $tempRoot ("ssd-sentinel-metadata-" + [guid]::NewGuid().ToString("N"))
Assert-ProjectBinary $installer[0].FullName "NSIS installer"
New-Item -ItemType Directory -Path $stage | Out-Null
try {
  $nsisDir = Join-Path $stage "nsis"
  $portableDir = Join-Path $stage "portable"
  & $SevenZip x $installer[0].FullName "-o$nsisDir" -y "ssd-sentinel.exe" "uninstall.exe" "smartmontools\bin\smartctl.exe" | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "7-Zip could not extract the Windows installer (exit $LASTEXITCODE)."
  }
  Expand-Archive -LiteralPath $portable[0].FullName -DestinationPath $portableDir

  Assert-ProjectBinary (Join-Path $nsisDir "ssd-sentinel.exe") "Installed executable"
  Assert-ProjectBinary (Join-Path $nsisDir "uninstall.exe") "NSIS uninstaller"
  Assert-ProjectBinary (Join-Path $portableDir "ssd-sentinel.exe") "Portable executable"

  foreach ($packageDir in @($nsisDir, $portableDir)) {
    $smartctl = Join-Path $packageDir "smartmontools/bin/smartctl.exe"
    $hash = (Get-FileHash -LiteralPath $smartctl -Algorithm SHA256).Hash
    if ($hash -ne $SmartctlSha256) {
      throw "The smartctl.exe in '$(Split-Path -Leaf $packageDir)' differs from the pinned upstream binary."
    }
  }
  Write-Host "Both packages contain the pinned upstream smartctl.exe."
} finally {
  $verifiedStage = (Resolve-Path -LiteralPath $stage).ProviderPath
  if ($verifiedStage -eq $stage -and (Split-Path -Parent $verifiedStage) -eq $tempRoot) {
    Remove-Item -LiteralPath $verifiedStage -Recurse -Force
  }
}
