param(
    [ValidateSet('Inventory', 'Performance', 'ReTrim')][string]$Mode = 'Inventory',
    [ValidatePattern('^[A-Za-z]$')][string]$DriveLetter,
    [string]$ExpectedSerial,
    [string]$ExpectedModel,
    [string]$ExpectedDiskNumber = ''
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

if ($Mode -eq 'Performance') {
    $rawCounters = @{}
    Get-CimInstance Win32_PerfRawData_PerfDisk_PhysicalDisk | ForEach-Object { $rawCounters[$_.Name] = $_ }
    $rows = @(Get-CimInstance Win32_PerfFormattedData_PerfDisk_PhysicalDisk | Where-Object { $_.Name -ne '_Total' } | ForEach-Object {
        $raw = $rawCounters[$_.Name]
        $number = $null
        if ($_.Name -match '^(\d+)(?:\s|$)') { $number = [int]$Matches[1] }
        [pscustomobject]@{
            number = $number; name = $_.Name
            read_bps = [double]$_.DiskReadBytesPersec; write_bps = [double]$_.DiskWriteBytesPersec
            read_iops = [double]$_.DiskReadsPersec; write_iops = [double]$_.DiskWritesPersec
            queue_length = [double]$_.CurrentDiskQueueLength
            busy_percent = [Math]::Max(0, [Math]::Min(100, 100 - [double]$_.PercentIdleTime))
            read_timer = @{ ticks = [string]$raw.AvgDisksecPerRead; operations = [string]$raw.AvgDisksecPerRead_Base; frequency = [string]$raw.Frequency_PerfTime }
            write_timer = @{ ticks = [string]$raw.AvgDisksecPerWrite; operations = [string]$raw.AvgDisksecPerWrite_Base; frequency = [string]$raw.Frequency_PerfTime }
        }
    })
    @{ disks = $rows; error = $null } | ConvertTo-Json -Depth 5 -Compress
    exit
}

if ($Mode -eq 'ReTrim') {
    if (-not $DriveLetter -or -not $ExpectedSerial -or -not $ExpectedModel) { throw 'Missing target identity' }
    $partitions = @(Get-Partition -DriveLetter $DriveLetter)
    if ($partitions.Count -ne 1) { throw 'Ambiguous volume mapping' }
    if ($ExpectedDiskNumber -match '^\d+$' -and $partitions[0].DiskNumber -ne [int]$ExpectedDiskNumber) { throw 'Disk number changed' }
    $disk = Get-CimInstance Win32_DiskDrive -Filter "Index=$($partitions[0].DiskNumber)"
    $normSer = { param([string]$s) (($s -replace '[\s_.-]+', '').ToLowerInvariant()) }
    $normModel = { param([string]$s) (((($s.Trim().ToLowerInvariant() -replace '\s+', ' ') -replace '^(nvme|ata|scsi|sata|usb|raid)[\s-]+', '') -replace '\s+(scsi disk device|ata device|usb device|disk device)$', '').Trim()) }
    if (-not $disk.SerialNumber) { throw 'Disk identity changed' }
    $actualSer = (& $normSer $disk.SerialNumber)
    $wantSer = (& $normSer $ExpectedSerial)
    if (-not $actualSer -or $actualSer -ne $wantSer) { throw 'Disk identity changed' }
    $actualMod = (& $normModel ([string]$disk.Model))
    $wantMod = (& $normModel $ExpectedModel)
    if (-not $actualMod -or ($actualMod -ne $wantMod -and -not ($actualMod.Length -ge 8 -and $wantMod.Length -ge 8 -and ($actualMod.StartsWith($wantMod) -or $wantMod.StartsWith($actualMod))))) { throw 'Disk model changed' }
    $volume = Get-Volume -DriveLetter $DriveLetter
    if ($volume.FileSystemType -notin @('NTFS', 'ReFS')) { throw 'Unsupported filesystem for ReTrim' }
    Optimize-Volume -DriveLetter $DriveLetter -ReTrim -ErrorAction Stop
    @{ status = 'completed'; volume = $DriveLetter; message = 'Windows ReTrim completed.' } | ConvertTo-Json -Compress
    exit
}

$notes = @()
$disks = @(Get-CimInstance Win32_DiskDrive | ForEach-Object {
    $physical = $_
    $volumes = @()
    try {
        $volumes = @(Get-Partition -DiskNumber $physical.Index | Where-Object DriveLetter | ForEach-Object {
            $letter = [string]$_.DriveLetter
            $volume = Get-Volume -DriveLetter $letter
            [pscustomobject]@{
                letter = $letter; label = $volume.FileSystemLabel; filesystem = [string]$volume.FileSystemType
                size_bytes = $volume.Size; free_bytes = $volume.SizeRemaining
            }
        })
    } catch { $notes += $_.Exception.Message }
    [pscustomobject]@{ number = $physical.Index; serial = ([string]$physical.SerialNumber).Trim(); model = $physical.Model; volumes = $volumes }
})
$trim = (& fsutil.exe behavior query DisableDeleteNotify 2>&1 | Out-String).Trim()
@{ disks = $disks; trim_output = $trim; notes = $notes; error = $null } | ConvertTo-Json -Depth 6 -Compress
