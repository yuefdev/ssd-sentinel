use crate::{
    core::{lock, Core, JobRequest},
    jobs::{add_block, cancelled, classify, phase, OwnedArea, BLOCKS},
    model::{normalise, Disk},
    platform, process,
    surface_triage::{self, Action, Target, TargetKind},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const THERMAL_THROTTLE_C: f64 = 53.0;
const CASCADE_REALLOCATED_DELTA: u64 = 8;
const SLOW_LBA_MS: f64 = 80.0;
const QUARANTINE_DIR_NAME: &str = ".ssd-sentinel-quarantine";

/// Human Turkish duration for live ETA / elapsed text ("~2 sa 05 dk", "~4 dk", "<1 dk").
pub fn fmt_eta(secs: f64) -> String {
    if !secs.is_finite() || secs < 60.0 {
        return "<1 dk".into();
    }
    let mins = (secs / 60.0).round() as u64;
    if mins < 60 {
        format!("~{mins} dk")
    } else {
        format!("~{} sa {:02} dk", mins / 60, mins % 60)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SmartSnapshot {
    pub health_percent: Option<u32>,
    pub temperature_c: Option<f64>,
    pub reallocated_5: u64,
    pub pending_197: u64,
    pub uncorrectable_198: u64,
}

impl SmartSnapshot {
    pub fn from_disk(disk: &Disk) -> Self {
        let attr_u64 = |id: &str| -> u64 {
            disk.smart
                .iter()
                .find(|r| r.id == id)
                .and_then(|r| r.raw.parse::<u128>().ok())
                .map(|v| (v & 0xffff) as u64)
                .unwrap_or(0)
        };
        Self {
            health_percent: disk.health.percent,
            temperature_c: disk.metrics["temperature_c"].as_f64(),
            reallocated_5: attr_u64("5"),
            pending_197: attr_u64("197"),
            uncorrectable_198: attr_u64("198"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WatchdogDecision {
    Continue,
    CoolDown { temp_c: f64 },
    AbortCascade { delta_reallocated: u64 },
}

pub fn evaluate_watchdog(baseline: &SmartSnapshot, current: &SmartSnapshot) -> WatchdogDecision {
    let delta_reallocated = current.reallocated_5.saturating_sub(baseline.reallocated_5);
    if delta_reallocated >= CASCADE_REALLOCATED_DELTA {
        return WatchdogDecision::AbortCascade { delta_reallocated };
    }
    if let Some(temp_c) = current.temperature_c {
        if temp_c >= THERMAL_THROTTLE_C {
            return WatchdogDecision::CoolDown { temp_c };
        }
    }
    WatchdogDecision::Continue
}

#[derive(Clone, Debug)]
pub struct ZbrZonePoint {
    pub index: usize,
    pub mb_s: Option<f64>,
    pub status: &'static str,
}

pub fn compute_zbr_profile(zones: &[ZbrZonePoint]) -> Value {
    let avg = |slice: &[ZbrZonePoint]| -> Option<f64> {
        let vals: Vec<f64> = slice.iter().filter_map(|z| z.mb_s).collect();
        if vals.is_empty() {
            None
        } else {
            Some(vals.iter().sum::<f64>() / vals.len() as f64)
        }
    };
    let head_len = zones.len().min(20);
    let tail_start = zones.len().saturating_sub(20);
    let outer_mb_s = avg(&zones[..head_len]);
    let inner_mb_s = avg(&zones[tail_start..]);
    let drop_percent = match (outer_mb_s, inner_mb_s) {
        (Some(o), Some(i)) if o > 0.0 => Some((((o - i) / o) * 100.0).clamp(0.0, 99.9)),
        _ => None,
    };
    let tail_bad_count = zones
        .iter()
        .filter(|z| z.index >= (zones.len() * 85 / 100) && matches!(z.status, "bad" | "warning"))
        .count();
    let tail_degraded = tail_bad_count >= 2;
    let suggested_tail_guard_pct = if tail_bad_count >= 8 {
        Some(15)
    } else if tail_bad_count >= 4 {
        Some(10)
    } else if tail_degraded {
        Some(5)
    } else {
        None
    };
    json!({
        "outer_mb_s": outer_mb_s,
        "inner_mb_s": inner_mb_s,
        "drop_percent": drop_percent,
        "tail_bad_count": tail_bad_count,
        "tail_degraded": tail_degraded,
        "suggested_tail_guard_pct": suggested_tail_guard_pct,
    })
}

/// Latency verification ceiling for `pin_region` re-read after rewriting a bad/weak sector.
/// Uses a strict smaller limit `(slow_ms_per_mib * 0.45).clamp(18.0, 45.0)` for `<= 128 KiB`
/// reads so weak 60 ms 4K sectors are quarantined rather than falsely marked Healed, ramps
/// smoothly to `slow_ms_per_mib` at `1 MiB`, and scales linearly above `1 MiB`.
pub fn pin_verify_limit_ms(slow_ms_per_mib: f64, len_bytes: usize) -> f64 {
    const SMALL_BYTES: f64 = 128.0 * 1024.0;
    const ONE_MIB: f64 = 1_048_576.0;
    let base = if slow_ms_per_mib.is_finite() && slow_ms_per_mib > 0.0 {
        slow_ms_per_mib
    } else {
        SLOW_LBA_MS
    };
    let small = (base * 0.45).clamp(18.0, 45.0).min(base);
    let len = len_bytes as f64;
    if len <= SMALL_BYTES {
        small
    } else if len < ONE_MIB {
        let t = (len - SMALL_BYTES) / (ONE_MIB - SMALL_BYTES);
        small + (base - small) * t
    } else {
        base * (len / ONE_MIB)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedZonePoint {
    pub index: usize,
    pub mb_s: Option<f64>,
    pub status: String,
}

impl From<&ZbrZonePoint> for SavedZonePoint {
    fn from(z: &ZbrZonePoint) -> Self {
        Self {
            index: z.index,
            mb_s: z.mb_s,
            status: z.status.to_owned(),
        }
    }
}

#[cfg(any(windows, test))]
fn zone_status_str(s: &str) -> &'static str {
    match s {
        "fast" => "fast",
        "normal" => "normal",
        "slow" => "slow",
        "warning" => "warning",
        "bad" => "bad",
        _ => "normal",
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SurfaceCheckpoint {
    pub disk_id: String,
    pub disk_bytes: u64,
    pub letter: String,
    pub next_zone: usize,
    pub total_zones: usize,
    pub done_bytes: u64,
    #[serde(default)]
    pub elapsed_seconds: f64,
    pub zones: Vec<SavedZonePoint>,
    pub blocks: Vec<Value>,
    pub targets: Vec<Target>,
    pub bad_lbas: Vec<Value>,
    pub bad_byte_offsets: Vec<u64>,
    pub slow_chunks_total: u64,
    #[serde(default)]
    pub transient_slow: u64,
    pub error_chunks: u64,
    pub unreadable_bytes: u64,
    pub micro_probed_sectors: u64,
    #[serde(default)]
    pub thermal_pauses: u64,
    pub lat_base: surface_triage::LatencyBaseline,
    #[serde(default)]
    pub updated_at: String,
}

/// Lightweight UI summary of a stored `surface_checkpoints` row.
pub fn checkpoint_summary(checkpoint: &Value) -> Value {
    let next_zone = checkpoint["next_zone"].as_u64().unwrap_or(0);
    let total_zones = checkpoint["total_zones"]
        .as_u64()
        .unwrap_or(BLOCKS as u64)
        .max(1);
    let disk_bytes = checkpoint["disk_bytes"].as_u64().unwrap_or(0);
    let done_bytes = checkpoint["done_bytes"].as_u64().unwrap_or(0);
    let progress_percent = done_bytes
        .saturating_mul(100)
        .checked_div(disk_bytes)
        .unwrap_or_else(|| next_zone.saturating_mul(100) / total_zones)
        .min(99);
    let targets_count = checkpoint["targets"].as_array().map_or(0, Vec::len);
    json!({
        "disk_id": checkpoint["disk_id"],
        "letter": checkpoint["letter"],
        "next_zone": next_zone,
        "total_zones": total_zones,
        "progress_percent": progress_percent,
        "done_bytes": done_bytes,
        "disk_bytes": disk_bytes,
        "elapsed_seconds": checkpoint["elapsed_seconds"].as_f64().unwrap_or(0.0),
        "slow_chunks": checkpoint["slow_chunks_total"].as_u64().unwrap_or(0),
        "error_chunks": checkpoint["error_chunks"].as_u64().unwrap_or(0),
        "targets_count": targets_count,
        "updated_at": checkpoint["updated_at"]
    })
}

/// Legacy WAL undo journal removed in v1.7.0 (pinpoint repair uses crash-safe NTFS
/// `FSCTL_MOVE_FILE` without in-place user data overwrites); kept as minimal no-op
/// for startup compatibility until `core.rs` drops the call.
pub fn recover_if_needed(_data_dir: &Path) -> Result<Option<String>, String> {
    Ok(None)
}

pub fn quarantine_dir(root: &Path) -> PathBuf {
    root.join(QUARANTINE_DIR_NAME)
}

pub fn release_quarantine_vault(root: &Path) -> Result<u64, String> {
    let dir = quarantine_dir(root);
    if !dir.exists() {
        return Ok(0);
    }
    if platform::reparse(&dir) {
        return Err("Karantina klasörü reparse point olduğundan işlem reddedildi.".into());
    }
    let mut removed = 0u64;
    #[cfg(windows)]
    win::clear_readonly_and_attributes(&dir);
    for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        if p.is_file() {
            #[cfg(windows)]
            win::clear_readonly_and_attributes(&p);
            std::fs::remove_file(&p).map_err(|e| e.to_string())?;
            removed += 1;
        }
    }
    std::fs::remove_dir(&dir).map_err(|e| e.to_string())?;
    Ok(removed)
}

fn poll_smart_snapshot(core: &Core, disk: &Disk) -> Result<SmartSnapshot, String> {
    let val = platform::smart(
        &core.config.smartctl,
        &[
            "-i".into(),
            "-H".into(),
            "-A".into(),
            "-d".into(),
            disk.device_type.clone(),
            disk.device.clone(),
        ],
        &core.job_cancel,
    )?;
    Ok(SmartSnapshot::from_disk(&normalise(&val)))
}

/// SMART + heat watchdog. `expected_remaps` is how many reallocations the current job is
/// *supposed* to cause (pinpoint repair deliberately remaps pending sectors), so only
/// growth beyond that counts as a cascading failure.
fn check_thermal_and_cascade(
    core: &Core,
    disk: &Disk,
    baseline: &SmartSnapshot,
    expected_remaps: u64,
    thermal_pauses: &mut u64,
) -> Result<(), String> {
    let Ok(current) = poll_smart_snapshot(core, disk) else {
        return Ok(());
    };
    let mut base = baseline.clone();
    base.reallocated_5 = base.reallocated_5.saturating_add(expected_remaps);
    match evaluate_watchdog(&base, &current) {
        WatchdogDecision::Continue => Ok(()),
        WatchdogDecision::AbortCascade { delta_reallocated } => Err(format!(
            "Akıllı SMART koruması tetiklendi: yeniden eşlenen sektör sayısı (+{delta_reallocated}) arttı; veri güvenliği için tarama durduruldu."
        )),
        WatchdogDecision::CoolDown { temp_c } => {
            *thermal_pauses += 1;
            phase(
                core,
                &format!("Termal koruma · disk sıcaklığı {temp_c:.0} °C · soğuma bekleniyor"),
            );
            for _ in 0..250 {
                cancelled(core)?;
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(())
        }
    }
}

pub(crate) fn run(
    core: &Core,
    request: &JobRequest,
    disk: &Disk,
    root: &Path,
    area: &mut Option<OwnedArea>,
) -> Result<(), String> {
    let mode = request.options["mode"].as_str().unwrap_or("physical_scan");
    if !platform::is_admin() {
        return Err(
            "Fiziksel yüzey tarama ve kurtarma işlemleri yönetici yetkisi gerektirir.".into(),
        );
    }
    match mode {
        "physical_scan" => {
            #[cfg(windows)]
            {
                win::run_physical_scan(core, request, disk, root)
            }
            #[cfg(not(windows))]
            {
                let _ = (core, request, disk, root);
                Err("Fiziksel LBA taraması yalnız Windows'ta desteklenir.".into())
            }
        }
        "magnetic_refresh" | "bad_zone_isolate" => {
            let isolate = mode == "bad_zone_isolate";
            if isolate && disk.kind != "HDD" {
                return Err(
                    "Ölü bölge kapatma yalnız mekanik HDD'lerde desteklenir; SSD'ler bozuk hücreleri kendisi değiştirir."
                        .into(),
                );
            }
            // The pinpoint engine works through NTFS and never needs the temp area.
            let _ = area;
            #[cfg(windows)]
            {
                win::run_targeted_repair(core, request, disk, root, isolate)
            }
            #[cfg(not(windows))]
            {
                let _ = (core, request, disk, root);
                Err("Nokta atışı onarım yalnız Windows'ta desteklenir.".into())
            }
        }
        _ => Err("Desteklenmeyen yüzey onarım modu.".into()),
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, GetDiskFreeSpaceW, ReadFile, SetFileAttributesW,
        SetFilePointerEx, WriteFile, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM, FILE_BEGIN, FILE_FLAG_NO_BUFFERING,
        FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{IOCTL_DISK_GET_LENGTH_INFO, VOLUME_DISK_EXTENTS};
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
    const FSCTL_GET_VOLUME_BITMAP: u32 = 0x0009_006F;
    const FSCTL_GET_RETRIEVAL_POINTERS: u32 = 0x0009_0073;
    const FSCTL_MOVE_FILE: u32 = 0x0009_0074;
    const FSCTL_MARK_HANDLE: u32 = 0x0009_00FC;
    const MARK_HANDLE_PROTECT_CLUSTERS: u32 = 1;

    struct WinHandle(HANDLE);
    impl Drop for WinHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    struct AlignedBuf {
        ptr: *mut u8,
        len: usize,
    }
    impl AlignedBuf {
        fn alloc(len: usize) -> Result<Self, String> {
            let ptr = unsafe { VirtualAlloc(null(), len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) }
                as *mut u8;
            if ptr.is_null() {
                return Err("Doğrudan disk erişim tamponu ayrılamadı.".into());
            }
            Ok(Self { ptr, len })
        }
        fn as_slice(&self) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
        fn as_mut_slice(&mut self) -> &mut [u8] {
            unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
        }
    }
    impl Drop for AlignedBuf {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                unsafe {
                    VirtualFree(self.ptr.cast(), 0, MEM_RELEASE);
                }
            }
        }
    }

    #[repr(C)]
    struct RetrievalHeader {
        extent_count: u32,
        _pad: u32,
        starting_vcn: i64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RetrievalExtent {
        next_vcn: i64,
        lcn: i64,
    }

    #[repr(C)]
    struct MoveFileData {
        file_handle: HANDLE,
        starting_vcn: i64,
        starting_lcn: i64,
        cluster_count: u32,
        _pad: u32,
    }

    #[repr(C)]
    struct MarkHandleInfo {
        usn_source_info: u32,
        _pad: u32,
        volume_handle: HANDLE,
        handle_info: u32,
        _pad2: u32,
    }

    #[repr(C)]
    struct VolumeBitmapHeader {
        starting_lcn: i64,
        bitmap_size: i64,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn open_win_path(
        path: &str,
        access: u32,
        creation: u32,
        flags: u32,
    ) -> Result<WinHandle, String> {
        let w = wide(path);
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                creation,
                flags,
                null_mut(),
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Err(format!(
                "{path} açılamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(WinHandle(h))
    }

    pub fn clear_readonly_and_attributes(path: &Path) {
        let w = wide(&path.to_string_lossy());
        unsafe {
            SetFileAttributesW(w.as_ptr(), FILE_ATTRIBUTE_NORMAL);
        }
    }

    fn mark_quarantine_attrs(path: &Path) {
        let w = wide(&path.to_string_lossy());
        unsafe {
            SetFileAttributesW(
                w.as_ptr(),
                FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM,
            );
        }
    }

    fn mark_hidden_system(path: &Path) {
        let w = wide(&path.to_string_lossy());
        unsafe {
            SetFileAttributesW(w.as_ptr(), FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM);
        }
    }

    fn physical_drive_length(h: HANDLE) -> Result<u64, String> {
        let mut len = 0i64;
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                IOCTL_DISK_GET_LENGTH_INFO,
                null(),
                0,
                (&mut len as *mut i64).cast(),
                std::mem::size_of::<i64>() as u32,
                &mut ret,
                null_mut(),
            )
        };
        if ok == 0 || len <= 0 {
            return Err(format!(
                "Fiziksel disk kapasitesi okunamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(len as u64)
    }

    fn volume_geometry(letter: &str) -> Result<(u32, u64, u64, u64), String> {
        let vol_handle = open_win_path(&format!("\\\\.\\{letter}:"), 0, OPEN_EXISTING, 0)?;
        let mut ext_buf = [0u8; 512];
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                vol_handle.0,
                IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                null(),
                0,
                ext_buf.as_mut_ptr().cast(),
                ext_buf.len() as u32,
                &mut ret,
                null_mut(),
            )
        };
        if ok == 0 || (ret as usize) < std::mem::size_of::<VOLUME_DISK_EXTENTS>() {
            return Err("Bölüm fiziksel disk ofseti okunamadı.".into());
        }
        let ext: VOLUME_DISK_EXTENTS = unsafe { std::ptr::read_unaligned(ext_buf.as_ptr().cast()) };
        if ext.NumberOfDiskExtents != 1 {
            return Err("Çok diskli (dinamik) bölümler fiziksel eşlemede desteklenmez.".into());
        }
        let disk_num = ext.Extents[0].DiskNumber;
        let start_off = ext.Extents[0].StartingOffset as u64;
        let len = ext.Extents[0].ExtentLength as u64;
        let root_w = wide(&format!("{letter}:\\"));
        let mut spc = 0u32;
        let mut bps = 0u32;
        let mut fc = 0u32;
        let mut tc = 0u32;
        let g_ok =
            unsafe { GetDiskFreeSpaceW(root_w.as_ptr(), &mut spc, &mut bps, &mut fc, &mut tc) };
        let cluster_bytes = if g_ok != 0 && spc > 0 && bps > 0 {
            u64::from(spc) * u64::from(bps)
        } else {
            4096
        };
        Ok((disk_num, start_off, len, cluster_bytes))
    }

    fn seek_and_read(h: HANDLE, offset: u64, buf: &mut [u8]) -> Result<u32, String> {
        let mut new_pos = 0i64;
        if unsafe { SetFilePointerEx(h, offset as i64, &mut new_pos, FILE_BEGIN) } == 0 {
            return Err(format!(
                "Konumlanamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                h,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(format!("Okuma hatası: {}", std::io::Error::last_os_error()));
        }
        Ok(read)
    }

    fn seek_and_write(h: HANDLE, offset: u64, buf: &[u8]) -> Result<(), String> {
        let mut new_pos = 0i64;
        if unsafe { SetFilePointerEx(h, offset as i64, &mut new_pos, FILE_BEGIN) } == 0 {
            return Err(format!(
                "Yazma konumlanamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut written = 0u32;
        let ok = unsafe {
            WriteFile(
                h,
                buf.as_ptr().cast(),
                buf.len() as u32,
                &mut written,
                null_mut(),
            )
        };
        if ok == 0 || written as usize != buf.len() {
            return Err(format!("Yazma hatası: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn query_file_extents(h: HANDLE) -> Vec<(i64, i64, i64)> {
        let start_vcn = 0i64;
        let mut out = vec![0u8; 4096];
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                FSCTL_GET_RETRIEVAL_POINTERS,
                (&start_vcn as *const i64).cast(),
                std::mem::size_of::<i64>() as u32,
                out.as_mut_ptr().cast(),
                out.len() as u32,
                &mut ret,
                null_mut(),
            )
        };
        let hdr_sz = std::mem::size_of::<RetrievalHeader>();
        let ext_sz = std::mem::size_of::<RetrievalExtent>();
        if ok == 0 || (ret as usize) < hdr_sz {
            return Vec::new();
        }
        let hdr: RetrievalHeader = unsafe { std::ptr::read_unaligned(out.as_ptr().cast()) };
        let mut prev_vcn = hdr.starting_vcn;
        let mut extents = Vec::new();
        for i in 0..hdr.extent_count as usize {
            let off = hdr_sz + i * ext_sz;
            if off + ext_sz > ret as usize {
                break;
            }
            let ext: RetrievalExtent =
                unsafe { std::ptr::read_unaligned(out[off..].as_ptr().cast()) };
            let clusters = ext.next_vcn.saturating_sub(prev_vcn);
            if ext.lcn >= 0 && clusters > 0 {
                extents.push((prev_vcn, ext.lcn, clusters));
            }
            prev_vcn = ext.next_vcn;
        }
        extents
    }

    fn map_bad_lcns_to_files(
        core: &Core,
        root: &Path,
        vol_start: u64,
        vol_len: u64,
        cluster_bytes: u64,
        bad_byte_offsets: &[u64],
    ) -> Vec<Value> {
        let target_lcns: Vec<i64> = bad_byte_offsets
            .iter()
            .filter_map(|&off| {
                if off >= vol_start && off < vol_start.saturating_add(vol_len) {
                    Some(((off - vol_start) / cluster_bytes) as i64)
                } else {
                    None
                }
            })
            .collect();
        if target_lcns.is_empty() {
            return Vec::new();
        }
        let mut found = Vec::new();
        let mut dirs = vec![root.to_path_buf()];
        let mut checked = 0usize;
        while let Some(dir) = dirs.pop() {
            if core.job_cancel.load(Ordering::SeqCst) || core.stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if checked >= 1500 || found.len() >= 32 {
                    break;
                }
                let p = entry.path();
                if platform::reparse(&p) {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if meta.is_dir() {
                    if dirs.len() < 512 {
                        dirs.push(p);
                    }
                    continue;
                }
                if !meta.is_file() || meta.len() == 0 {
                    continue;
                }
                checked += 1;
                let Ok(fh) = open_win_path(&p.to_string_lossy(), GENERIC_READ, OPEN_EXISTING, 0)
                else {
                    continue;
                };
                for (_vcn, lcn, count) in query_file_extents(fh.0) {
                    if target_lcns
                        .iter()
                        .any(|&t| t >= lcn && t < lcn.saturating_add(count))
                    {
                        let rel = p
                            .strip_prefix(root)
                            .unwrap_or(&p)
                            .to_string_lossy()
                            .into_owned();
                        found.push(json!({
                            "path": rel,
                            "block": 0,
                            "error": format!("Bozuk/yavaş fiziksel LCN kümesi ile çakışıyor (LCN {lcn})")
                        }));
                        break;
                    }
                }
            }
            if checked >= 1500 || found.len() >= 32 {
                break;
            }
        }
        found
    }

    /// (byte offset, latency if slow, error if unreadable)
    type ProbeHit = (u64, Option<f64>, Option<String>);

    /// Reads `[off, off+len)` sector by sector (4 KiB) to pinpoint the exact bad or slow LBAs
    /// inside a chunk that failed or stalled. Stops after 16 consecutive hard errors so a dead
    /// region does not hang the scan; the remainder is reported as one unreadable span.
    fn probe_chunk_4k(
        core: &Core,
        h: HANDLE,
        off: u64,
        len: u64,
        micro_buf: &mut AlignedBuf,
    ) -> (Vec<ProbeHit>, u64) {
        let mut hits = Vec::new();
        let mut probed = 0u64;
        let mut consecutive_errors = 0u32;
        let mut pos = off;
        while pos + 4096 <= off + len {
            if core.job_cancel.load(Ordering::SeqCst) {
                break;
            }
            probed += 1;
            let t = Instant::now();
            let res = seek_and_read(h, pos, micro_buf.as_mut_slice());
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            match res {
                Err(err) => {
                    consecutive_errors += 1;
                    hits.push((pos, None, Some(err)));
                    if consecutive_errors >= 16 {
                        let rest = off + len - pos - 4096;
                        if rest > 0 {
                            hits.push((
                                pos + 4096,
                                None,
                                Some(format!(
                                    "Ardışık okuma hatası; kalan {} KiB okunamaz kabul edildi",
                                    rest / 1024
                                )),
                            ));
                        }
                        break;
                    }
                }
                Ok(_) => {
                    consecutive_errors = 0;
                    if ms > SLOW_LBA_MS {
                        hits.push((pos, Some(ms), None));
                    }
                }
            }
            pos += 4096;
        }
        (hits, probed)
    }

    /// True full-surface scan: every byte of `\\.\PhysicalDriveN` is read sequentially in
    /// 1 MiB unbuffered chunks (like Victoria / HD Tune "full scan"). The 200 map cells are
    /// 200 equal slices of the disk; each cell reports its worst chunk latency and average
    /// throughput. Failed or stalled chunks are re-read and then probed sector by sector.
    pub fn run_physical_scan(
        core: &Core,
        request: &JobRequest,
        disk: &Disk,
        root: &Path,
    ) -> Result<(), String> {
        const CHUNK: u64 = 1024 * 1024;
        const MAX_ERROR_CHUNKS: u64 = 2000;
        let mapped = platform::verify_disk_mapping(
            &core.disks()?,
            disk,
            &platform::inventory(&core.job_cancel)?,
        )?;
        let disk_num = mapped
            .os_disk_number
            .ok_or("Fiziksel sürücü numarası doğrulanamadı.")?;
        let drive_path = format!("\\\\.\\PhysicalDrive{disk_num}");
        let h = open_win_path(
            &drive_path,
            GENERIC_READ,
            OPEN_EXISTING,
            FILE_FLAG_NO_BUFFERING,
        )?;
        let disk_bytes = physical_drive_length(h.0)?;
        let sector = mapped.logical_block_size.unwrap_or(512).clamp(512, 4096);
        let baseline = SmartSnapshot::from_disk(&mapped);
        let mut buf = AlignedBuf::alloc(CHUNK as usize)?;
        let mut micro_buf = AlignedBuf::alloc(4096)?;
        let is_hdd = mapped.kind == "HDD";
        let want_resume = request.options.get("resume") == Some(&json!(true));
        let resumed = if want_resume {
            lock(&core.store)
                .get_surface_checkpoint(&mapped.id)
                .ok()
                .flatten()
                .and_then(|v| serde_json::from_value::<SurfaceCheckpoint>(v).ok())
                .filter(|cp| {
                    cp.disk_id == mapped.id
                        && cp.disk_bytes == disk_bytes
                        && cp.total_zones == BLOCKS
                        && cp.next_zone > 0
                        && cp.next_zone < BLOCKS
                        && cp.zones.len() == cp.next_zone
                })
        } else {
            None
        };

        let mut start_zone = 0usize;
        let mut lat_base = surface_triage::LatencyBaseline::new();
        let mut targets: Vec<Target> = Vec::new();
        let mut zones = Vec::with_capacity(BLOCKS);
        let mut bad_byte_offsets = Vec::new();
        let mut bad_lbas = Vec::new();
        let mut micro_probed_sectors = 0u64;
        let mut thermal_pauses = 0u64;
        let mut slow_chunks_total = 0u64;
        let mut transient_slow = 0u64;
        let mut error_chunks = 0u64;
        let mut unreadable_bytes = 0u64;
        let mut done_bytes = 0u64;
        let mut prior_elapsed = 0.0f64;

        lock(&core.job)["total"] = json!(BLOCKS);
        if let Some(cp) = resumed {
            start_zone = cp.next_zone;
            lat_base = cp.lat_base;
            targets = cp.targets;
            zones = cp
                .zones
                .iter()
                .map(|z| ZbrZonePoint {
                    index: z.index,
                    mb_s: z.mb_s,
                    status: zone_status_str(&z.status),
                })
                .collect();
            bad_byte_offsets = cp.bad_byte_offsets;
            bad_lbas = cp.bad_lbas;
            micro_probed_sectors = cp.micro_probed_sectors;
            thermal_pauses = cp.thermal_pauses;
            slow_chunks_total = cp.slow_chunks_total;
            transient_slow = cp.transient_slow;
            error_chunks = cp.error_chunks;
            unreadable_bytes = cp.unreadable_bytes;
            done_bytes = cp.done_bytes;
            prior_elapsed = cp.elapsed_seconds.max(0.0);

            let latencies: Vec<f64> = cp
                .blocks
                .iter()
                .filter_map(|b| b["latency_ms"].as_f64())
                .collect();
            let mut job = lock(&core.job);
            job["read_bytes"] = json!(done_bytes);
            job["completed"] = json!(start_zone);
            job["progress"] = json!((done_bytes * 100 / disk_bytes.max(1)).min(99));
            job["summary"] = json!({
                "tested_blocks": cp.blocks.len(),
                "bad_blocks": cp.blocks.iter().filter(|b| b["status"] == "bad").count(),
                "warning_blocks": cp.blocks.iter().filter(|b| b["status"] == "warning").count(),
                "avg_latency_ms": if latencies.is_empty() { None } else { Some(latencies.iter().sum::<f64>() / latencies.len() as f64) },
                "max_latency_ms": latencies.iter().copied().reduce(f64::max)
            });
            job["blocks"] = json!(cp.blocks);
        }
        let resumed_bytes = done_bytes;
        let scan_start = Instant::now();
        let mut last_phase = Instant::now() - Duration::from_secs(5);
        let mut last_smart = Instant::now();

        struct ZoneBoundary {
            next_zone: usize,
            done_bytes: u64,
            targets_len: usize,
            bad_offsets_len: usize,
            bad_lbas_len: usize,
            slow_chunks_total: u64,
            transient_slow: u64,
            error_chunks: u64,
            unreadable_bytes: u64,
            micro_probed_sectors: u64,
            thermal_pauses: u64,
            lat_base: surface_triage::LatencyBaseline,
        }

        let persist_checkpoint = |b: &ZoneBoundary,
                                  zones: &[ZbrZonePoint],
                                  targets: &[Target],
                                  bad_lbas: &[Value],
                                  bad_byte_offsets: &[u64]| {
            if b.next_zone == 0 || b.next_zone >= BLOCKS {
                return;
            }
            let blocks: Vec<Value> = lock(&core.job)["blocks"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .take(b.next_zone)
                .collect();
            let cp = SurfaceCheckpoint {
                disk_id: mapped.id.clone(),
                disk_bytes,
                letter: request.letter.clone(),
                next_zone: b.next_zone,
                total_zones: BLOCKS,
                done_bytes: b.done_bytes,
                elapsed_seconds: prior_elapsed + scan_start.elapsed().as_secs_f64(),
                zones: zones
                    .iter()
                    .take(b.next_zone)
                    .map(SavedZonePoint::from)
                    .collect(),
                blocks,
                targets: targets[..b.targets_len.min(targets.len())].to_vec(),
                bad_lbas: bad_lbas[..b.bad_lbas_len.min(bad_lbas.len())].to_vec(),
                bad_byte_offsets: bad_byte_offsets[..b.bad_offsets_len.min(bad_byte_offsets.len())]
                    .to_vec(),
                slow_chunks_total: b.slow_chunks_total,
                transient_slow: b.transient_slow,
                error_chunks: b.error_chunks,
                unreadable_bytes: b.unreadable_bytes,
                micro_probed_sectors: b.micro_probed_sectors,
                thermal_pauses: b.thermal_pauses,
                lat_base: b.lat_base.clone(),
                updated_at: String::new(),
            };
            if let Ok(val) = serde_json::to_value(&cp) {
                let _ = lock(&core.store).save_surface_checkpoint(&mapped.id, &val);
            }
        };

        for i in start_zone..BLOCKS {
            let boundary = ZoneBoundary {
                next_zone: i,
                done_bytes,
                targets_len: targets.len(),
                bad_offsets_len: bad_byte_offsets.len(),
                bad_lbas_len: bad_lbas.len(),
                slow_chunks_total,
                transient_slow,
                error_chunks,
                unreadable_bytes,
                micro_probed_sectors,
                thermal_pauses,
                lat_base: lat_base.clone(),
            };
            let zone_start = (disk_bytes / BLOCKS as u64 * i as u64) / sector * sector;
            let zone_end = if i + 1 == BLOCKS {
                disk_bytes / sector * sector
            } else {
                (disk_bytes / BLOCKS as u64 * (i as u64 + 1)) / sector * sector
            };
            let mut off = zone_start;
            let mut zone_read = 0u64;
            let mut zone_ms_total = 0.0f64;
            let mut worst_ms = 0.0f64;
            let mut zone_errors = 0u64;
            let mut zone_slow = 0u64;
            let mut first_error: Option<String> = None;

            while off < zone_end {
                if let Err(e) = cancelled(core) {
                    persist_checkpoint(&boundary, &zones, &targets, &bad_lbas, &bad_byte_offsets);
                    return Err(e);
                }
                if last_smart.elapsed() >= Duration::from_secs(60) {
                    if let Err(e) =
                        check_thermal_and_cascade(core, &mapped, &baseline, 0, &mut thermal_pauses)
                    {
                        if e == process::CANCELLED {
                            persist_checkpoint(
                                &boundary,
                                &zones,
                                &targets,
                                &bad_lbas,
                                &bad_byte_offsets,
                            );
                        }
                        return Err(e);
                    }
                    last_smart = Instant::now();
                }
                let len = (zone_end - off).min(CHUNK) / sector * sector;
                if len == 0 {
                    break;
                }
                let slow_ms = surface_triage::slow_threshold_ms(&lat_base, is_hdd);
                let slice = &mut buf.as_mut_slice()[..len as usize];
                let t = Instant::now();
                let mut res = seek_and_read(h.0, off, slice);
                let mut ms = t.elapsed().as_secs_f64() * 1000.0;
                if res.is_ok() && ms > slow_ms {
                    // One retry separates a transient OS / seek hiccup from a weak area.
                    let t2 = Instant::now();
                    let retry = seek_and_read(h.0, off, &mut buf.as_mut_slice()[..len as usize]);
                    let ms2 = t2.elapsed().as_secs_f64() * 1000.0;
                    if retry.is_ok() && ms2 <= slow_ms {
                        transient_slow += 1;
                        ms = ms2;
                    } else {
                        res = retry;
                        ms = ms.max(ms2);
                    }
                }
                let failed = res.is_err();
                if !failed && ms <= slow_ms {
                    lat_base.push(ms);
                }
                if failed || ms > slow_ms {
                    if failed {
                        zone_errors += 1;
                        error_chunks += 1;
                        if first_error.is_none() {
                            first_error = res.as_ref().err().cloned();
                        }
                    } else {
                        zone_slow += 1;
                        slow_chunks_total += 1;
                    }
                    // SSD slowness is per area (stale NAND pages), so the whole chunk is the
                    // target; HDD and hard errors are narrowed down to exact 4K sectors.
                    let (hits, probed) = if !failed && !is_hdd {
                        (Vec::new(), 0)
                    } else {
                        probe_chunk_4k(core, h.0, off, len, &mut micro_buf)
                    };
                    micro_probed_sectors += probed;
                    if hits.is_empty() && targets.len() < surface_triage::MAX_TARGETS {
                        targets.push(Target {
                            offset: off,
                            len,
                            kind: if failed {
                                TargetKind::Unreadable
                            } else {
                                TargetKind::SlowArea
                            },
                            latency_ms: (!failed).then_some(ms),
                        });
                    }
                    for (pos, lat, err) in hits {
                        if err.is_some() {
                            unreadable_bytes += 4096;
                        }
                        if targets.len() < surface_triage::MAX_TARGETS {
                            targets.push(Target {
                                offset: pos,
                                len: 4096,
                                kind: if err.is_some() {
                                    TargetKind::Unreadable
                                } else {
                                    TargetKind::WeakSector
                                },
                                latency_ms: lat,
                            });
                        }
                        if bad_byte_offsets.len() < 4096 {
                            bad_byte_offsets.push(pos);
                        }
                        if bad_lbas.len() < 64 {
                            bad_lbas.push(json!({
                                "lba": pos / 512,
                                "offset_bytes": pos,
                                "latency_ms": lat,
                                "error": err
                            }));
                        }
                    }
                    if error_chunks >= MAX_ERROR_CHUNKS {
                        return Err(format!(
                            "{error_chunks} bölgede okuma hatası bulundu; diski korumak için tarama durduruldu. Verilerinizi hemen başka bir diske yedekleyin."
                        ));
                    }
                }
                if !failed {
                    zone_read += len;
                    zone_ms_total += ms;
                }
                worst_ms = worst_ms.max(ms);
                off += len;
                done_bytes += len;

                if last_phase.elapsed() >= Duration::from_secs(1) {
                    last_phase = Instant::now();
                    let session_done = done_bytes.saturating_sub(resumed_bytes);
                    let secs = scan_start.elapsed().as_secs_f64().max(0.001);
                    let mb_s = session_done as f64 / 1e6 / secs;
                    let remaining = if mb_s > 0.0 {
                        (disk_bytes.saturating_sub(done_bytes)) as f64 / 1e6 / mb_s
                    } else {
                        0.0
                    };
                    phase(
                        core,
                        &format!(
                            "Tam yüzey taraması · Bölge {}/{BLOCKS} · {:.1} / {:.1} GB · {:.0} MB/sn · kalan {}",
                            i + 1,
                            done_bytes as f64 / 1e9,
                            disk_bytes as f64 / 1e9,
                            mb_s,
                            fmt_eta(remaining)
                        ),
                    );
                    let mut job = lock(&core.job);
                    job["progress"] = json!((done_bytes * 100 / disk_bytes.max(1)).min(99));
                }
            }

            let zone_mb_s = if zone_ms_total > 0.0 {
                Some((zone_read as f64 / 1e6) / (zone_ms_total / 1000.0))
            } else {
                None
            };
            let st = if zone_errors > 0 {
                "bad"
            } else {
                classify(worst_ms)
            };
            zones.push(ZbrZonePoint {
                index: i,
                mb_s: zone_mb_s,
                status: st,
            });
            add_block(
                core,
                json!({
                    "index": i,
                    "status": st,
                    "latency_ms": worst_ms,
                    "mb_s": zone_mb_s,
                    "offset_bytes": zone_start,
                    "bytes": zone_end - zone_start,
                    "file": format!("PhysicalDrive{disk_num} · LBA {}–{}", zone_start / 512, zone_end / 512),
                    "error": first_error.map(|e| format!("{zone_errors} okunamayan parça · {e}")).or_else(|| (zone_slow > 0).then(|| format!("{zone_slow} yavaş parça (>{SLOW_LBA_MS:.0} ms)"))),
                    "slow_chunks": zone_slow,
                    "error_chunks": zone_errors
                }),
                zone_end - zone_start,
                0,
            );
            let completed_zones = i + 1;
            if completed_zones.is_multiple_of(5) && completed_zones < BLOCKS {
                let done_boundary = ZoneBoundary {
                    next_zone: completed_zones,
                    done_bytes,
                    targets_len: targets.len(),
                    bad_offsets_len: bad_byte_offsets.len(),
                    bad_lbas_len: bad_lbas.len(),
                    slow_chunks_total,
                    transient_slow,
                    error_chunks,
                    unreadable_bytes,
                    micro_probed_sectors,
                    thermal_pauses,
                    lat_base: lat_base.clone(),
                };
                persist_checkpoint(
                    &done_boundary,
                    &zones,
                    &targets,
                    &bad_lbas,
                    &bad_byte_offsets,
                );
            }
        }
        drop(h);

        let zbr_profile = compute_zbr_profile(&zones);
        let mapped_files = match volume_geometry(&request.letter) {
            Ok((v_disk, v_off, v_len, cluster_bytes)) if u64::from(v_disk) == disk_num => {
                map_bad_lcns_to_files(core, root, v_off, v_len, cluster_bytes, &bad_byte_offsets)
            }
            _ => Vec::new(),
        };
        let smart_after = poll_smart_snapshot(core, &mapped).unwrap_or_else(|_| baseline.clone());
        let elapsed = prior_elapsed + scan_start.elapsed().as_secs_f64();
        let diagnosis = surface_triage::diagnose(
            &targets,
            is_hdd,
            smart_after.pending_197,
            zbr_profile["tail_degraded"].as_bool().unwrap_or(false),
        );
        {
            let mut job = lock(&core.job);
            if !mapped_files.is_empty() {
                job["damaged_files"] = json!(mapped_files);
            }
            job["repair_summary"] = json!({
                "mode": "physical_scan",
                "full_surface": true,
                "resumed_from_zone": (start_zone > 0).then_some(start_zone),
                "targets": targets,
                "diagnosis": diagnosis,
                "slow_threshold_ms": surface_triage::slow_threshold_ms(&lat_base, is_hdd),
                "median_chunk_ms": lat_base.median(),
                "disk_bytes": disk_bytes,
                "scanned_bytes": done_bytes,
                "elapsed_seconds": elapsed,
                "avg_mb_s": if elapsed > 0.0 { done_bytes as f64 / 1e6 / elapsed } else { 0.0 },
                "slow_chunks": slow_chunks_total,
                "transient_slow": transient_slow,
                "error_chunks": error_chunks,
                "unreadable_bytes": unreadable_bytes,
                "micro_probed_sectors": micro_probed_sectors,
                "bad_lbas": bad_lbas,
                "zbr_profile": zbr_profile,
                "thermal_pauses": thermal_pauses,
                "smart_before": baseline,
                "smart_after": smart_after,
                "scope": format!(
                    "Tam yüzey: {:.1} GB baştan sona okundu · {} · {} yavaş, {} okunamayan parça.",
                    done_bytes as f64 / 1e9,
                    fmt_eta(elapsed),
                    slow_chunks_total,
                    error_chunks
                )
            });
        }
        let _ = lock(&core.store).clear_surface_checkpoint(&mapped.id);
        Ok(())
    }

    const FSCTL_LOOKUP_STREAM_FROM_CLUSTER: u32 = 0x0009_03F3;
    const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const SHARE_DELETE: u32 = 0x0000_0004;
    const FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const CREATE_ALWAYS: u32 = 2;
    const ERROR_MORE_DATA: i32 = 234;
    /// Largest piece pinned in one go (16 MiB at 4 KiB clusters).
    const PIN_CHUNK_CLUSTERS: u64 = 4096;
    /// OS errors that mean "the source sectors could not be read" during a move.
    const DATA_LOSS_ERRORS: [i32; 5] = [23, 483, 1117, 1127, 1392];

    fn open_for_move(path: &str) -> Result<WinHandle, String> {
        let w = wide(path);
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FLAG_BACKUP_SEMANTICS,
                null_mut(),
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Err(format!("açılamadı: {}", std::io::Error::last_os_error()));
        }
        Ok(WinHandle(h))
    }

    /// Every extent of a file, following `ERROR_MORE_DATA` for fragmented files.
    fn file_extents_all(h: HANDLE) -> Vec<(i64, i64, i64)> {
        let hdr_sz = std::mem::size_of::<RetrievalHeader>();
        let ext_sz = std::mem::size_of::<RetrievalExtent>();
        let mut out = vec![0u8; 64 * 1024];
        let mut extents = Vec::new();
        let mut start_vcn = 0i64;
        for _ in 0..4096 {
            let mut ret = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    h,
                    FSCTL_GET_RETRIEVAL_POINTERS,
                    (&start_vcn as *const i64).cast(),
                    std::mem::size_of::<i64>() as u32,
                    out.as_mut_ptr().cast(),
                    out.len() as u32,
                    &mut ret,
                    null_mut(),
                )
            };
            let more =
                ok == 0 && std::io::Error::last_os_error().raw_os_error() == Some(ERROR_MORE_DATA);
            if (ok == 0 && !more) || (ret as usize) < hdr_sz {
                break;
            }
            let hdr: RetrievalHeader = unsafe { std::ptr::read_unaligned(out.as_ptr().cast()) };
            let mut prev = hdr.starting_vcn;
            for i in 0..hdr.extent_count as usize {
                let off = hdr_sz + i * ext_sz;
                if off + ext_sz > ret as usize {
                    break;
                }
                let ext: RetrievalExtent =
                    unsafe { std::ptr::read_unaligned(out[off..].as_ptr().cast()) };
                let n = ext.next_vcn.saturating_sub(prev);
                if ext.lcn >= 0 && n > 0 {
                    extents.push((prev, ext.lcn, n));
                }
                prev = ext.next_vcn;
            }
            if !more || prev <= start_vcn {
                break;
            }
            start_vcn = prev;
        }
        extents
    }

    /// Allocation state of `[lcn, lcn + n)` read from the NTFS volume bitmap.
    fn cluster_allocation(vol: HANDLE, lcn: u64, n: u64) -> Result<Vec<bool>, String> {
        let start = (lcn / 8 * 8) as i64;
        let hdr_sz = std::mem::size_of::<VolumeBitmapHeader>();
        let mut out = vec![0u8; hdr_sz + ((lcn - start as u64 + n) / 8 + 16) as usize];
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                vol,
                FSCTL_GET_VOLUME_BITMAP,
                (&start as *const i64).cast(),
                std::mem::size_of::<i64>() as u32,
                out.as_mut_ptr().cast(),
                out.len() as u32,
                &mut ret,
                null_mut(),
            )
        };
        if ok == 0 && std::io::Error::last_os_error().raw_os_error() != Some(ERROR_MORE_DATA) {
            return Err(format!(
                "Küme haritası okunamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        if (ret as usize) <= hdr_sz {
            return Err("Küme haritası boş döndü.".into());
        }
        let hdr: VolumeBitmapHeader = unsafe { std::ptr::read_unaligned(out.as_ptr().cast()) };
        let bits = &out[hdr_sz..ret as usize];
        Ok((0..n)
            .map(|k| {
                let b = (lcn + k) as i64 - hdr.starting_lcn;
                if b < 0 || b >= hdr.bitmap_size {
                    return true;
                }
                let b = b as usize;
                b / 8 >= bits.len() || (bits[b / 8] >> (b % 8)) & 1 == 1
            })
            .collect())
    }

    /// First free run of `need` clusters in `[from, to)` that keeps 256 clusters away from
    /// every damaged region, so relocated data never lands next to a bad spot.
    fn free_run_in(
        vol: HANDLE,
        need: u64,
        from: u64,
        to: u64,
        avoid: &[(u64, u64)],
    ) -> Option<u64> {
        let hdr_sz = std::mem::size_of::<VolumeBitmapHeader>();
        let mut out = vec![0u8; hdr_sz + 1024 * 1024];
        let mut pos = (from / 8 * 8) as i64;
        let mut run_start = 0u64;
        let mut run_len = 0u64;
        let clear = |s: u64| {
            !avoid
                .iter()
                .any(|(a, b)| s < b.saturating_add(256) && s + need + 256 > *a)
        };
        loop {
            let mut ret = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    vol,
                    FSCTL_GET_VOLUME_BITMAP,
                    (&pos as *const i64).cast(),
                    std::mem::size_of::<i64>() as u32,
                    out.as_mut_ptr().cast(),
                    out.len() as u32,
                    &mut ret,
                    null_mut(),
                )
            };
            let more =
                ok == 0 && std::io::Error::last_os_error().raw_os_error() == Some(ERROR_MORE_DATA);
            if (ok == 0 && !more) || (ret as usize) <= hdr_sz {
                return None;
            }
            let hdr: VolumeBitmapHeader = unsafe { std::ptr::read_unaligned(out.as_ptr().cast()) };
            let bm = &out[hdr_sz..ret as usize];
            let here = (hdr.bitmap_size.max(0) as u64).min(bm.len() as u64 * 8);
            let mut b = 0u64;
            while b < here {
                let lcn = hdr.starting_lcn as u64 + b;
                if lcn >= to {
                    return None;
                }
                if b.is_multiple_of(8) && b + 8 <= here && bm[(b / 8) as usize] == 0xFF {
                    run_len = 0;
                    b += 8;
                    continue;
                }
                let used = (bm[(b / 8) as usize] >> (b % 8)) & 1 == 1;
                if used || lcn < from {
                    run_len = 0;
                } else {
                    if run_len == 0 {
                        run_start = lcn;
                    }
                    run_len += 1;
                    if run_len >= need {
                        if clear(run_start) {
                            return Some(run_start);
                        }
                        run_len = 0;
                    }
                }
                b += 1;
            }
            if !more || here == 0 {
                return None;
            }
            pos = hdr.starting_lcn + here as i64;
        }
    }

    fn find_free_run(vol: HANDLE, need: u64, hint: u64, avoid: &[(u64, u64)]) -> Option<u64> {
        free_run_in(vol, need, hint, u64::MAX, avoid)
            .or_else(|| free_run_in(vol, need, 0, hint.saturating_add(need), avoid))
    }

    struct Owner {
        lcn: u64,
        path: String,
        flags: u32,
    }

    /// Reverse map clusters to the files that own them (`FSCTL_LOOKUP_STREAM_FROM_CLUSTER`).
    fn lookup_cluster_owners(vol: HANDLE, lcns: &[u64]) -> Vec<Owner> {
        let mut owners = Vec::new();
        for batch in lcns.chunks(256) {
            // u64 words keep the LARGE_INTEGER array 8-byte aligned for the kernel.
            let mut input: Vec<u64> = Vec::with_capacity(batch.len() + 1);
            input.push((batch.len() as u64) << 32);
            input.extend(batch.iter().copied());
            let mut out: Vec<u64> = vec![0; 32 * 1024];
            for _ in 0..3 {
                let mut ret = 0u32;
                let ok = unsafe {
                    DeviceIoControl(
                        vol,
                        FSCTL_LOOKUP_STREAM_FROM_CLUSTER,
                        input.as_ptr().cast(),
                        (input.len() * 8) as u32,
                        out.as_mut_ptr().cast(),
                        (out.len() * 8) as u32,
                        &mut ret,
                        null_mut(),
                    )
                };
                let bytes: &[u8] =
                    unsafe { std::slice::from_raw_parts(out.as_ptr().cast(), out.len() * 8) };
                let rd = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
                if ok == 0 {
                    let need = rd(8) as usize;
                    if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_MORE_DATA)
                        && need > out.len() * 8
                        && need < 64 * 1024 * 1024
                    {
                        out = vec![0; need.div_ceil(8)];
                        continue;
                    }
                    break;
                }
                let ret = ret as usize;
                let matches = rd(4) as usize;
                let mut pos = rd(0) as usize;
                for _ in 0..matches {
                    if pos + 24 > ret {
                        break;
                    }
                    let next = rd(pos) as usize;
                    let flags = rd(pos + 4);
                    let cluster = u64::from_le_bytes(bytes[pos + 16..pos + 24].try_into().unwrap());
                    let end = if next == 0 {
                        ret
                    } else {
                        (pos + next).min(ret)
                    };
                    let name: Vec<u16> = (pos + 24..end.saturating_sub(1))
                        .step_by(2)
                        .map(|o| u16::from_le_bytes([bytes[o], bytes[o + 1]]))
                        .take_while(|&c| c != 0)
                        .collect();
                    owners.push(Owner {
                        lcn: cluster,
                        path: String::from_utf16_lossy(&name),
                        flags,
                    });
                    if next == 0 {
                        break;
                    }
                    pos += next;
                }
                break;
            }
        }
        owners
    }

    enum MoveErr {
        DataLost(String),
        Blocked(String),
    }

    fn clean_stream_path(raw: &str) -> String {
        let s = raw.trim_start_matches('\\');
        let s = s.strip_suffix("::$DATA").unwrap_or(s);
        s.strip_suffix(":$DATA").unwrap_or(s).to_string()
    }

    /// Moves exactly the clusters of one file that sit on damaged LCNs to healthy free space
    /// with `FSCTL_MOVE_FILE` (the crash-safe NTFS defrag primitive; data is never rewritten
    /// in place). Returns moved cluster count and the first new LCN.
    fn relocate_clusters(
        vol: HANDLE,
        root: &Path,
        owner_path: &str,
        lcns: &[u64],
        avoid: &[(u64, u64)],
        hint: &mut u64,
    ) -> Result<(u64, u64), MoveErr> {
        let full = root.join(clean_stream_path(owner_path));
        let fh = open_for_move(&full.to_string_lossy()).map_err(MoveErr::Blocked)?;
        let extents = file_extents_all(fh.0);
        let mut vcns: Vec<i64> = lcns
            .iter()
            .filter_map(|&t| {
                extents.iter().find_map(|&(vcn, lcn, n)| {
                    let t = t as i64;
                    (t >= lcn && t < lcn + n).then_some(vcn + (t - lcn))
                })
            })
            .collect();
        vcns.sort_unstable();
        vcns.dedup();
        if vcns.is_empty() {
            return Err(MoveErr::Blocked("küme artık bu dosyada değil".into()));
        }
        let mut runs: Vec<(i64, u64)> = Vec::new();
        for v in vcns {
            match runs.last_mut() {
                Some((s, n)) if *s + *n as i64 == v && *n < PIN_CHUNK_CLUSTERS => *n += 1,
                _ => runs.push((v, 1)),
            }
        }
        let mut moved = 0u64;
        let mut first_new = 0u64;
        for (start_vcn, n) in runs {
            let target = find_free_run(vol, n, *hint, avoid)
                .ok_or_else(|| MoveErr::Blocked("taşımak için sağlam boş alan yok".into()))?;
            let mv = MoveFileData {
                file_handle: fh.0,
                starting_vcn: start_vcn,
                starting_lcn: target as i64,
                cluster_count: n as u32,
                _pad: 0,
            };
            let mut ret = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    vol,
                    FSCTL_MOVE_FILE,
                    (&mv as *const MoveFileData).cast(),
                    std::mem::size_of::<MoveFileData>() as u32,
                    null_mut(),
                    0,
                    &mut ret,
                    null_mut(),
                )
            };
            if ok == 0 {
                let e = std::io::Error::last_os_error();
                let msg = e.to_string();
                return Err(
                    if DATA_LOSS_ERRORS.contains(&e.raw_os_error().unwrap_or(0)) {
                        MoveErr::DataLost(msg)
                    } else {
                        MoveErr::Blocked(msg)
                    },
                );
            }
            if moved == 0 {
                first_new = target;
            }
            moved += n;
            *hint = target + n;
        }
        Ok((moved, first_new))
    }

    enum PinResult {
        Healed,
        Quarantined,
        Unplaceable(String),
    }

    /// Places a private file exactly on `[lcn, lcn + n)`. The move itself writes those
    /// sectors (an HDD remaps a pending sector on write). In heal mode the area is then
    /// rewritten and re-read: healthy → file deleted, space returned; still bad → file kept
    /// and pinned so NTFS never hands those clusters to user data again.
    fn pin_region(
        vol: HANDLE,
        qdir: &Path,
        lcn: u64,
        n: u64,
        cluster: u64,
        isolate: bool,
        slow_ms_per_mib: f64,
    ) -> PinResult {
        let len = (n * cluster) as usize;
        let path = qdir.join(format!("pin_{lcn:012}.qbin"));
        clear_readonly_and_attributes(&path);
        let fh = match open_win_path(
            &path.to_string_lossy(),
            GENERIC_READ | GENERIC_WRITE,
            CREATE_ALWAYS,
            FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH,
        ) {
            Ok(h) => h,
            Err(e) => return PinResult::Unplaceable(e),
        };
        let fail = |fh: WinHandle, e: String| {
            drop(fh);
            let _ = std::fs::remove_file(&path);
            PinResult::Unplaceable(e)
        };
        let (mut wbuf, mut rbuf) = match (AlignedBuf::alloc(len), AlignedBuf::alloc(len)) {
            (Ok(a), Ok(b)) => (a, b),
            _ => return fail(fh, "tampon ayrılamadı".into()),
        };
        if getrandom::fill(wbuf.as_mut_slice()).is_err() {
            return fail(fh, "rastgele desen üretilemedi".into());
        }
        if let Err(e) = seek_and_write(fh.0, 0, wbuf.as_slice()) {
            return fail(fh, e);
        }
        unsafe {
            FlushFileBuffers(fh.0);
        }
        let mv = MoveFileData {
            file_handle: fh.0,
            starting_vcn: 0,
            starting_lcn: lcn as i64,
            cluster_count: n as u32,
            _pad: 0,
        };
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                vol,
                FSCTL_MOVE_FILE,
                (&mv as *const MoveFileData).cast(),
                std::mem::size_of::<MoveFileData>() as u32,
                null_mut(),
                0,
                &mut ret,
                null_mut(),
            )
        };
        if ok == 0 {
            return fail(
                fh,
                format!("sektöre yazılamadı ({})", std::io::Error::last_os_error()),
            );
        }
        if file_extents_all(fh.0).first().map(|e| e.1) != Some(lcn as i64) {
            return fail(fh, "küme hedefe yerleşmedi".into());
        }
        let keep = |fh: WinHandle| {
            let mh = MarkHandleInfo {
                usn_source_info: 0,
                _pad: 0,
                volume_handle: vol,
                handle_info: MARK_HANDLE_PROTECT_CLUSTERS,
                _pad2: 0,
            };
            let mut r = 0u32;
            unsafe {
                DeviceIoControl(
                    fh.0,
                    FSCTL_MARK_HANDLE,
                    (&mh as *const MarkHandleInfo).cast(),
                    std::mem::size_of::<MarkHandleInfo>() as u32,
                    null_mut(),
                    0,
                    &mut r,
                    null_mut(),
                );
            }
            drop(fh);
            mark_quarantine_attrs(&path);
            PinResult::Quarantined
        };
        if isolate {
            return keep(fh);
        }
        let _ = getrandom::fill(wbuf.as_mut_slice());
        if seek_and_write(fh.0, 0, wbuf.as_slice()).is_err() {
            return keep(fh);
        }
        unsafe {
            FlushFileBuffers(fh.0);
        }
        let t = Instant::now();
        let read_ok = seek_and_read(fh.0, 0, rbuf.as_mut_slice()).is_ok();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let limit = pin_verify_limit_ms(slow_ms_per_mib, len);
        if read_ok && rbuf.as_slice() == wbuf.as_slice() && ms <= limit {
            drop(fh);
            let _ = std::fs::remove_file(&path);
            PinResult::Healed
        } else {
            keep(fh)
        }
    }

    fn time_phys(h: HANDLE, off: u64, len: u64, buf: &mut AlignedBuf, sector: u64) -> Option<f64> {
        let off = off / sector * sector;
        let len = len.clamp(sector, 1024 * 1024).div_ceil(sector) * sector;
        let t = Instant::now();
        seek_and_read(h, off, &mut buf.as_mut_slice()[..len as usize])
            .ok()
            .map(|_| t.elapsed().as_secs_f64() * 1000.0)
    }

    type ScanMap = (Vec<Target>, String, Value, Option<f64>);

    fn latest_full_scan(core: &Core, disk_id: &str) -> Result<ScanMap, String> {
        let jobs = lock(&core.store).jobs("surface", Some(disk_id))?;
        for j in jobs.as_array().into_iter().flatten() {
            let rs = &j["repair_summary"];
            if j["options"]["mode"] == "physical_scan"
                && j["status"] == "completed"
                && rs["full_surface"] == true
            {
                let targets: Vec<Target> =
                    serde_json::from_value(rs["targets"].clone()).unwrap_or_default();
                return Ok((
                    targets,
                    j["finished_at"].as_str().unwrap_or("").to_string(),
                    rs["zbr_profile"].clone(),
                    rs["slow_threshold_ms"].as_f64(),
                ));
            }
        }
        Err("Önce “Tam yüzey taraması” yapın. Onarım, taramanın bulduğu bozuk ve yavaş sektörleri nokta atışı hedefler.".into())
    }

    fn action_label(a: Action) -> &'static str {
        match a {
            Action::Heal => "onarılıyor",
            Action::Isolate => "kapatılıyor",
            Action::Relocate => "tazeleniyor",
        }
    }

    /// Pinpoint repair driven by the last full surface scan.
    ///
    /// For every damaged region: user data is moved off it with `FSCTL_MOVE_FILE`; on HDDs
    /// the freed sectors are then rewritten in place (remap / refresh) and re-tested, and
    /// only what still fails is fenced off. On SSDs the stale data is rewritten to fresh
    /// cells, which restores read speed. Before/after latency is measured per region.
    pub fn run_targeted_repair(
        core: &Core,
        request: &JobRequest,
        disk: &Disk,
        root: &Path,
        isolate_mode: bool,
    ) -> Result<(), String> {
        let mapped = platform::verify_disk_mapping(
            &core.disks()?,
            disk,
            &platform::inventory(&core.job_cancel)?,
        )?;
        let is_hdd = mapped.kind == "HDD";
        if isolate_mode && !is_hdd {
            return Err("Ölü bölge kapatma yalnız mekanik HDD'lerde anlamlıdır.".into());
        }
        let disk_num = mapped
            .os_disk_number
            .ok_or("Fiziksel sürücü numarası doğrulanamadı.")?;
        let baseline = SmartSnapshot::from_disk(&mapped);
        phase(
            core,
            "1/4 · Son tam yüzey taramasının hedef haritası yükleniyor",
        );
        let (targets, scan_at, scan_zbr, scan_slow) = latest_full_scan(core, &mapped.id)?;
        let (v_disk, vol_start, vol_len, cluster) = volume_geometry(&request.letter)?;
        if u64::from(v_disk) != disk_num {
            return Err("Seçilen bölüm bu fiziksel diske ait değil.".into());
        }
        let (regions, outside) =
            surface_triage::plan_regions(&targets, vol_start, vol_len, cluster);

        let chkdsk = if isolate_mode {
            phase(
                core,
                "1/4 · NTFS çevrimiçi bütünlük kontrolü (chkdsk /scan)",
            );
            Some(process::run(
                &platform::system_executable("chkdsk.exe"),
                &[format!("{}:", request.letter), "/scan".into()],
                Duration::from_secs(600),
                &core.job_cancel,
            )?)
        } else {
            None
        };

        let sector = mapped.logical_block_size.unwrap_or(512).clamp(512, 4096);
        let slow_ms = scan_slow.unwrap_or(if is_hdd { 80.0 } else { 30.0 });
        let total = regions.len();
        lock(&core.job)["total"] = json!(total.max(1));
        let vol_rw = open_win_path(
            &format!("\\\\.\\{}:", request.letter),
            GENERIC_READ | GENERIC_WRITE,
            OPEN_EXISTING,
            0,
        )?;
        let phys = open_win_path(
            &format!("\\\\.\\PhysicalDrive{disk_num}"),
            GENERIC_READ,
            OPEN_EXISTING,
            FILE_FLAG_NO_BUFFERING,
        )?;
        let mut tbuf = AlignedBuf::alloc(1024 * 1024)?;
        let qdir = quarantine_dir(root);
        if is_hdd && total > 0 {
            std::fs::create_dir_all(&qdir).map_err(|e| e.to_string())?;
            mark_hidden_system(&qdir);
        }
        let avoid: Vec<(u64, u64)> = regions
            .iter()
            .map(|r| (r.lcn, r.lcn + r.clusters))
            .collect();
        let allowance = targets
            .iter()
            .filter(|t| t.kind == TargetKind::Unreadable)
            .count() as u64;
        let mut hint = 0u64;
        let mut last_smart = Instant::now();
        let mut thermal_pauses = 0u64;
        let (mut healed, mut quarantined_runs, mut unplaceable) = (0u64, 0u64, 0u64);
        let (mut quarantine_bytes, mut quarantine_clusters) = (0u64, 0u64);
        let (mut relocated_files, mut relocated_clusters) = (0u64, 0u64);
        let mut written = 0u64;
        let mut repairs: Vec<Value> = Vec::new();
        let mut damaged: Vec<Value> = Vec::new();
        let mut after_probe: Vec<(usize, u64, u64)> = Vec::new();

        for (i, r) in regions.iter().enumerate() {
            cancelled(core)?;
            if last_smart.elapsed() >= Duration::from_secs(60) {
                check_thermal_and_cascade(
                    core,
                    &mapped,
                    &baseline,
                    allowance,
                    &mut thermal_pauses,
                )?;
                last_smart = Instant::now();
            }
            let action = surface_triage::decide(r, is_hdd, isolate_mode);
            phase(
                core,
                &format!(
                    "2/4 · Hedef {}/{total} · LBA {} · {} {}",
                    i + 1,
                    r.phys_offset / 512,
                    r.kind.label(),
                    action_label(action)
                ),
            );
            let span = r.clusters * cluster;
            let before_ms = time_phys(phys.0, r.phys_offset, span, &mut tbuf, sector);

            // 1) Move user data off the damaged clusters (never rewritten in place).
            let used: Vec<u64> = cluster_allocation(vol_rw.0, r.lcn, r.clusters)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .filter(|(_, u)| **u)
                .map(|(k, _)| r.lcn + k as u64)
                .collect();
            let mut groups: std::collections::BTreeMap<String, (u32, Vec<u64>)> =
                Default::default();
            for o in lookup_cluster_owners(vol_rw.0, &used) {
                let e = groups.entry(o.path).or_insert((o.flags, Vec::new()));
                e.0 |= o.flags;
                e.1.push(o.lcn);
            }
            let (mut moved, mut lost, mut system, mut blocked) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let mut new_lcn = None;
            for (path, (flags, lcns)) in &groups {
                let shown = clean_stream_path(path);
                let movable = flags & 0xFF00_0000 == 0x0100_0000 && flags & 0x0F == 0;
                if !movable {
                    system.push(shown);
                    continue;
                }
                match relocate_clusters(vol_rw.0, root, path, lcns, &avoid, &mut hint) {
                    Ok((n, at)) => {
                        relocated_files += 1;
                        relocated_clusters += n;
                        written += n * cluster;
                        new_lcn.get_or_insert(at);
                        moved.push(shown);
                    }
                    Err(MoveErr::DataLost(e)) => {
                        damaged.push(json!({"path": shown, "block": i, "error": format!("Okunamayan sektörde veri kaybı: {e}")}));
                        lost.push(shown);
                    }
                    Err(MoveErr::Blocked(e)) => blocked.push(format!("{shown} ({e})")),
                }
            }

            // 2) HDD: rewrite the now-free sectors (heal) or fence them off (isolate).
            let mut region_healed = 0u64;
            let mut region_fenced = 0u64;
            let mut region_dead = Vec::new();
            if is_hdd {
                let alloc = cluster_allocation(vol_rw.0, r.lcn, r.clusters).unwrap_or_default();
                let max_pin_clusters = (16 * 1024 * 1024 / cluster.max(512)).max(1);
                let mut k = 0usize;
                while k < alloc.len() {
                    if alloc[k] {
                        k += 1;
                        continue;
                    }
                    let s = k;
                    while k < alloc.len() && !alloc[k] && ((k - s) as u64) < max_pin_clusters {
                        k += 1;
                    }
                    let (lcn, n) = (r.lcn + s as u64, (k - s) as u64);
                    match pin_region(
                        vol_rw.0,
                        &qdir,
                        lcn,
                        n,
                        cluster,
                        action == Action::Isolate,
                        slow_ms,
                    ) {
                        PinResult::Healed => {
                            healed += 1;
                            region_healed += n;
                            written += 2 * n * cluster;
                        }
                        PinResult::Quarantined => {
                            quarantined_runs += 1;
                            region_fenced += n;
                            quarantine_clusters += n;
                            quarantine_bytes += n * cluster;
                            written += n * cluster;
                        }
                        PinResult::Unplaceable(e) => {
                            unplaceable += 1;
                            region_dead.push(e);
                        }
                    }
                }
            }

            let (result, status) = if !lost.is_empty() {
                ("Veri kaybı · dosya kısmen okunamıyor", "bad")
            } else if !region_dead.is_empty() {
                ("Kalıcı ölü sektör · chkdsk /r gerekli", "bad")
            } else if region_fenced > 0 && action == Action::Isolate {
                ("Kalıcı olarak kapatıldı", "warning")
            } else if region_fenced > 0 {
                ("Onarılamadı · kapatıldı", "warning")
            } else if !system.is_empty() && moved.is_empty() && region_healed == 0 {
                ("Sistem alanı · chkdsk /r önerilir", "warning")
            } else if !blocked.is_empty() && moved.is_empty() && region_healed == 0 {
                ("Dosya kilitli · yeniden deneyin", "warning")
            } else if !is_hdd && !moved.is_empty() {
                ("Tazelendi · veri yeni hücrelere yazıldı", "ok")
            } else if !is_hdd {
                ("Boş alan · işlem gerekmez", "ok")
            } else {
                ("Onarıldı", "ok")
            };
            let probe_at = if is_hdd {
                Some(r.phys_offset)
            } else {
                new_lcn.map(|l| vol_start + l * cluster)
            };
            if let Some(at) = probe_at {
                after_probe.push((repairs.len(), at, span));
            }
            repairs.push(json!({
                "lba": r.phys_offset / 512,
                "offset_bytes": r.phys_offset,
                "bytes": span,
                "kind": r.kind,
                "kind_label": r.kind.label(),
                "dead_zone": r.dead_zone,
                "action": action,
                "result": result,
                "status": status,
                "scan_ms": r.latency_ms,
                "before_ms": before_ms,
                "after_ms": null,
                "moved_files": moved.iter().take(3).collect::<Vec<_>>(),
                "moved_count": moved.len(),
                "lost_files": lost,
                "system_files": system.iter().take(3).collect::<Vec<_>>(),
                "blocked": blocked.iter().take(3).collect::<Vec<_>>(),
                "dead_errors": region_dead.iter().take(2).collect::<Vec<_>>()
            }));
            let block = json!({
                "index": i,
                "status": if status == "ok" { "fast" } else if status == "warning" { "warning" } else { "bad" },
                "latency_ms": before_ms,
                "mb_s": null,
                "offset_bytes": r.phys_offset,
                "bytes": span,
                "file": format!("LBA {} · {} · {result}", r.phys_offset / 512, r.kind.label()),
                "error": if status == "bad" { json!(result) } else { Value::Null }
            });
            if i < BLOCKS {
                add_block(core, block, span, 0);
            } else {
                let mut job = lock(&core.job);
                let done = job["completed"].as_u64().unwrap_or(0) + 1;
                job["completed"] = json!(done);
                job["progress"] = json!(done * 100 / total.max(1) as u64);
            }
        }

        // 3) Measure every repaired spot again, after other I/O has flushed drive caches.
        phase(core, "3/4 · Onarım sonrası hız doğrulaması");
        let (mut before_sum, mut after_sum, mut paired) = (0.0f64, 0.0f64, 0u64);
        for (idx, at, span) in &after_probe {
            cancelled(core)?;
            let after = time_phys(phys.0, *at, *span, &mut tbuf, sector);
            repairs[*idx]["after_ms"] = json!(after);
            if let (Some(b), Some(a)) = (repairs[*idx]["before_ms"].as_f64(), after) {
                before_sum += b;
                after_sum += a;
                paired += 1;
            }
        }
        drop(phys);
        drop(vol_rw);

        phase(core, "4/4 · SMART değişimi ve sonuç raporu hazırlanıyor");
        if quarantine_bytes > 0 {
            let key = format!("quarantine:{}:{}", mapped.id, request.letter);
            let _ = lock(&core.store).record_optimization(
                &key,
                &format!("{}:{}", mapped.id, request.letter),
                &format!("HDD Ölü Bölge Karantinası · {}:", request.letter),
                "0",
                &format!("{quarantine_bytes}"),
            );
        } else {
            let _ = std::fs::remove_dir(&qdir);
        }
        let smart_after = poll_smart_snapshot(core, &mapped).unwrap_or_else(|_| baseline.clone());
        let recovered_pending = baseline.pending_197.saturating_sub(smart_after.pending_197);
        let mut advice: Vec<String> = Vec::new();
        if unplaceable > 0 {
            advice.push(format!("{unplaceable} bölgeye hiç yazılamadı: disk yedek sektörü kalmamış olabilir. Windows'ta “chkdsk {}: /r” çalıştırıp bu alanları kalıcı bozuk küme olarak işaretleyin ve diski değiştirmeyi planlayın.", request.letter));
        }
        if !damaged.is_empty() {
            advice.push(format!(
                "{} dosyanın bir kısmı okunamıyor; bu dosyaları yedekten geri yükleyin.",
                damaged.len()
            ));
        }
        if regions.iter().any(|r| r.dead_zone) && !isolate_mode {
            advice.push("Bazı bozuk sektörler kümelenmiş; bu bölgeler otomatik kapatıldı.".into());
        }
        if outside > 0 {
            advice.push(format!(
                "{outside} hedef başka bir bölümde; o bölümü seçip tekrar çalıştırın."
            ));
        }
        if total == 0 {
            advice.push(if targets.is_empty() {
                "Son tam tarama sorun bulmadı; onarılacak bir şey yok.".into()
            } else {
                "Taramanın bulduğu hedeflerin hiçbiri bu bölümde değil.".into()
            });
        }
        if !is_hdd && relocated_files > 0 {
            advice.push(
                "Tazelenen veriler artık yeni hücrelerde; okuma hızları geri gelmiş olmalı.".into(),
            );
        }
        {
            let mut job = lock(&core.job);
            if !damaged.is_empty() {
                job["damaged_files"] = json!(damaged);
            }
            job["write_bytes"] = json!(written);
            job["progress"] = json!(100);
            job["repair_summary"] = json!({
                "mode": if isolate_mode { "bad_zone_isolate" } else { "magnetic_refresh" },
                "engine": "pinpoint",
                "scan_finished_at": scan_at,
                "targets_total": targets.len(),
                "regions": total,
                "outside_volume": outside,
                "healed": healed,
                "quarantined_runs": quarantined_runs,
                "unplaceable": unplaceable,
                "relocated_files": relocated_files,
                "relocated_bytes": relocated_clusters * cluster,
                "isolated_clusters": quarantine_clusters,
                "isolated_bytes": quarantine_bytes,
                "data_loss_files": damaged.len(),
                "speed": {
                    "before_avg_ms": if paired > 0 { Some(before_sum / paired as f64) } else { None },
                    "after_avg_ms": if paired > 0 { Some(after_sum / paired as f64) } else { None },
                    "measured": paired
                },
                "repairs": repairs.iter().take(400).collect::<Vec<_>>(),
                "advice": advice,
                "zbr_profile": scan_zbr,
                "filesystem_check": chkdsk.as_ref().map(|c| json!({
                    "ok": c.code == 0,
                    "code": c.code,
                    "output": c.stdout.chars().take(6000).collect::<String>()
                })),
                "thermal_pauses": thermal_pauses,
                "smart_before": baseline,
                "smart_after": smart_after,
                "recovered_pending": recovered_pending,
                "scope": format!(
                    "Nokta atışı {}: {} hedef bölge · {} onarıldı · {} dosya taşındı · {} kapatıldı.",
                    if isolate_mode { "ölü bölge kapatma" } else if is_hdd { "sektör onarımı" } else { "SSD tazeleme" },
                    total,
                    healed,
                    relocated_files,
                    quarantined_runs
                )
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_verify_limit_quarantines_weak_4k_sectors_and_scales_smoothly() {
        // 4 KiB on a standard 80 ms/MiB HDD: strict 36 ms limit, so a 60 ms sector is Quarantined.
        let lim_4k = pin_verify_limit_ms(80.0, 4096);
        assert!((lim_4k - 36.0).abs() < 1e-6, "lim_4k={lim_4k}");
        assert!(
            60.0 > lim_4k,
            "Weak 60 ms 4K sector must exceed verify limit"
        );

        // Even on a slow 150 ms/MiB USB HDD, 4K limit caps at 45 ms so 60 ms is still Quarantined.
        assert!((pin_verify_limit_ms(150.0, 4096) - 45.0).abs() < 1e-6);
        assert!(60.0 > pin_verify_limit_ms(150.0, 4096));

        // 128 KiB stays at small limit; 576 KiB (midpoint) interpolates; 1 MiB equals base; 4 MiB scales 4x.
        assert!((pin_verify_limit_ms(80.0, 128 * 1024) - 36.0).abs() < 1e-6);
        assert!((pin_verify_limit_ms(80.0, 576 * 1024) - 58.0).abs() < 1e-6);
        assert!((pin_verify_limit_ms(80.0, 1_048_576) - 80.0).abs() < 1e-6);
        assert!((pin_verify_limit_ms(80.0, 4 * 1_048_576) - 320.0).abs() < 1e-6);

        // Floor clamp for fast drives, and monotonic across all sizes.
        assert!((pin_verify_limit_ms(30.0, 4096) - 18.0).abs() < 1e-6);
        let sizes = [
            4096, 65_536, 131_072, 262_144, 524_288, 1_048_576, 2_097_152,
        ];
        for pair in sizes.windows(2) {
            assert!(pin_verify_limit_ms(80.0, pair[0]) <= pin_verify_limit_ms(80.0, pair[1]));
        }
        // Invalid threshold falls back safely to SLOW_LBA_MS (80 ms).
        assert!((pin_verify_limit_ms(f64::NAN, 4096) - 36.0).abs() < 1e-6);
    }

    #[test]
    fn surface_checkpoint_roundtrip_with_store_and_summary() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::store::Store::open(&dir.path().join("scan.sqlite3")).unwrap();
        let mut base = surface_triage::LatencyBaseline::new();
        for _ in 0..300 {
            base.push(9.5);
        }
        let cp = SurfaceCheckpoint {
            disk_id: "723ba7cbb815e46c1c2a90a8".into(),
            disk_bytes: 1_000_000_000_000,
            letter: "D".into(),
            next_zone: 45,
            total_zones: BLOCKS,
            done_bytes: 225_000_000_000,
            elapsed_seconds: 312.5,
            zones: (0..45)
                .map(|idx| SavedZonePoint {
                    index: idx,
                    mb_s: Some(150.0),
                    status: "normal".into(),
                })
                .collect(),
            blocks: vec![json!({"index": 0, "status": "normal", "latency_ms": 9.5})],
            targets: vec![Target {
                offset: 100 * 1024 * 1024,
                len: 4096,
                kind: TargetKind::WeakSector,
                latency_ms: Some(92.0),
            }],
            bad_lbas: vec![json!({"lba": 204800, "offset_bytes": 104_857_600u64})],
            bad_byte_offsets: vec![104_857_600],
            slow_chunks_total: 3,
            transient_slow: 1,
            error_chunks: 1,
            unreadable_bytes: 4096,
            micro_probed_sectors: 256,
            thermal_pauses: 0,
            lat_base: base.clone(),
            updated_at: String::new(),
        };
        let encoded = serde_json::to_value(&cp).unwrap();
        store
            .save_surface_checkpoint(&cp.disk_id, &encoded)
            .unwrap();

        let stored = store.get_surface_checkpoint(&cp.disk_id).unwrap().unwrap();
        let summary = checkpoint_summary(&stored);
        assert_eq!(summary["disk_id"], cp.disk_id);
        assert_eq!(summary["letter"], "D");
        assert_eq!(summary["next_zone"], 45);
        assert_eq!(summary["total_zones"], BLOCKS);
        assert_eq!(summary["progress_percent"], 22);
        assert_eq!(summary["targets_count"], 1);
        assert_eq!(summary["slow_chunks"], 3);
        assert_eq!(summary["error_chunks"], 1);
        assert!(summary["updated_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));

        let decoded: SurfaceCheckpoint = serde_json::from_value(stored).unwrap();
        assert_eq!(decoded.lat_base, base);
        assert_eq!(decoded.targets.len(), 1);
        assert_eq!(zone_status_str(&decoded.zones[0].status), "normal");

        store.clear_surface_checkpoint(&cp.disk_id).unwrap();
        assert!(store.get_surface_checkpoint(&cp.disk_id).unwrap().is_none());
    }

    #[test]
    fn eta_text_is_human_turkish() {
        assert_eq!(fmt_eta(12.0), "<1 dk");
        assert_eq!(fmt_eta(f64::NAN), "<1 dk");
        assert_eq!(fmt_eta(4.0 * 60.0), "~4 dk");
        assert_eq!(fmt_eta(2.0 * 3600.0 + 5.0 * 60.0), "~2 sa 05 dk");
    }

    #[test]
    fn thermal_and_cascade_watchdog_throttles_heat_and_aborts_platter_cascade() {
        let base = SmartSnapshot {
            health_percent: Some(95),
            temperature_c: Some(40.0),
            reallocated_5: 2,
            pending_197: 1,
            uncorrectable_198: 0,
        };
        assert_eq!(
            evaluate_watchdog(
                &base,
                &SmartSnapshot {
                    temperature_c: Some(46.0),
                    ..base.clone()
                }
            ),
            WatchdogDecision::Continue
        );
        assert_eq!(
            evaluate_watchdog(
                &base,
                &SmartSnapshot {
                    temperature_c: Some(53.0),
                    ..base.clone()
                }
            ),
            WatchdogDecision::CoolDown { temp_c: 53.0 }
        );
        assert_eq!(
            evaluate_watchdog(
                &base,
                &SmartSnapshot {
                    reallocated_5: 10,
                    ..base.clone()
                }
            ),
            WatchdogDecision::AbortCascade {
                delta_reallocated: 8
            }
        );
    }

    #[test]
    fn zbr_profiler_detects_outer_inner_drop_and_tail_degradation() {
        let mut zones = Vec::new();
        for i in 0..200 {
            let speed = if i < 20 {
                190.0
            } else if i >= 180 {
                95.0
            } else {
                140.0
            };
            let status = if (185..190).contains(&i) {
                "warning"
            } else {
                "normal"
            };
            zones.push(ZbrZonePoint {
                index: i,
                mb_s: Some(speed),
                status,
            });
        }
        let zbr = compute_zbr_profile(&zones);
        assert_eq!(zbr["outer_mb_s"], 190.0);
        assert_eq!(zbr["inner_mb_s"], 95.0);
        assert_eq!(zbr["drop_percent"], 50.0);
        assert_eq!(zbr["tail_degraded"], true);
        assert_eq!(zbr["suggested_tail_guard_pct"], 10);
    }

    #[test]
    fn quarantine_vault_lifecycle_creates_and_releases_cleanly_in_tempdir() {
        let root = tempfile::tempdir().unwrap();
        let qdir = quarantine_dir(root.path());
        std::fs::create_dir_all(&qdir).unwrap();
        let shield = qdir.join("qzone_000.qbin");
        std::fs::write(&shield, vec![0xFFu8; 4096]).unwrap();
        #[cfg(windows)]
        {
            let w: Vec<u16> = shield
                .to_string_lossy()
                .encode_utf16()
                .chain(Some(0))
                .collect();
            unsafe {
                windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(
                    w.as_ptr(),
                    windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_READONLY
                        | windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN
                        | windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_SYSTEM,
                );
            }
        }
        let removed = release_quarantine_vault(root.path()).unwrap();
        assert_eq!(removed, 1);
        assert!(!qdir.exists());
    }
}
