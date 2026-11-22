[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "tools/vhdx_integration_test.ps1 requires Administrator privileges to attach temporary NTFS VHDX volumes."
}

$usedLetters = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
foreach ($d in [System.IO.DriveInfo]::GetDrives()) {
    [void]$usedLetters.Add($d.Name.Substring(0, 1))
}

$freeLetters = @()
foreach ($c in [char[]]"VWUTRQPNMLKJHG") {
    $l = [string]$c
    if (-not $usedLetters.Contains($l)) {
        $freeLetters += $l
    }
}
if ($freeLetters.Count -lt 2) {
    throw "Could not find two free drive letters for temporary VHDX volumes."
}

$srcLetter = $freeLetters[0]
$dstLetter = $freeLetters[1]
$runId = [Guid]::NewGuid().ToString("N").Substring(0, 8)
$workDir = Join-Path ([System.IO.Path]::GetTempPath()) "ssd-sentinel-vhdx-$runId"
New-Item -ItemType Directory -Force -Path $workDir | Out-Null

$srcVhdx = Join-Path $workDir "src-test.vhdx"
$dstVhdx = Join-Path $workDir "dst-test.vhdx"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).ProviderPath

function Invoke-DiskpartScript {
    param(
        [string[]]$Lines,
        [string]$Label,
        [switch]$IgnoreError
    )
    $scriptPath = Join-Path $workDir "dp-$Label.txt"
    [System.IO.File]::WriteAllLines($scriptPath, $Lines, [System.Text.Encoding]::ASCII)
    $out = & diskpart /s $scriptPath 2>&1
    if ($LASTEXITCODE -ne 0 -and -not $IgnoreError) {
        throw "diskpart ($Label) failed with exit $LASTEXITCODE`:`n$($out -join "`n")"
    }
}

try {
    Write-Host "Creating temporary NTFS VHDX volumes ($srcLetter`: -> $dstLetter`:) in $workDir..."

    Invoke-DiskpartScript -Label "create-src" -Lines @(
        "create vdisk file=`"$srcVhdx`" maximum=128 type=expandable",
        "select vdisk file=`"$srcVhdx`"",
        "attach vdisk",
        "convert mbr",
        "create partition primary",
        "format fs=ntfs quick label=`"SENTINEL_SRC`"",
        "assign letter=$srcLetter"
    )

    Invoke-DiskpartScript -Label "create-dst" -Lines @(
        "create vdisk file=`"$dstVhdx`" maximum=128 type=expandable",
        "select vdisk file=`"$dstVhdx`"",
        "attach vdisk",
        "convert mbr",
        "create partition primary",
        "format fs=ntfs quick label=`"SENTINEL_DST`"",
        "assign letter=$dstLetter"
    )

    for ($i = 0; $i -lt 30; $i++) {
        if ((Test-Path "${srcLetter}:\") -and (Test-Path "${dstLetter}:\")) { break }
        Start-Sleep -Milliseconds 200
    }
    if (-not (Test-Path "${srcLetter}:\") -or -not (Test-Path "${dstLetter}:\")) {
        throw "Mounted VHDX drive letters ${srcLetter}: or ${dstLetter}: did not become ready."
    }

    $env:SSD_SENTINEL_VHDX_SRC = $srcLetter
    $env:SSD_SENTINEL_VHDX_DST = $dstLetter

    Push-Location $repoRoot
    try {
        cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --test ntfs_safety_integration -- --ignored --nocapture
        if ($LASTEXITCODE -ne 0) {
            throw "VHDX NTFS integration tests failed with exit $LASTEXITCODE"
        }
    } finally {
        Pop-Location
        Remove-Item Env:SSD_SENTINEL_VHDX_SRC -ErrorAction SilentlyContinue
        Remove-Item Env:SSD_SENTINEL_VHDX_DST -ErrorAction SilentlyContinue
    }
} finally {
    Write-Host "Detaching and cleaning up temporary VHDX volumes..."
    if (Test-Path $srcVhdx) {
        Invoke-DiskpartScript -Label "detach-src" -IgnoreError -Lines @(
            "select vdisk file=`"$srcVhdx`"",
            "detach vdisk"
        )
    }
    if (Test-Path $dstVhdx) {
        Invoke-DiskpartScript -Label "detach-dst" -IgnoreError -Lines @(
            "select vdisk file=`"$dstVhdx`"",
            "detach vdisk"
        )
    }
    Remove-Item -Path $workDir -Recurse -Force -ErrorAction SilentlyContinue
}
