use crate::{
    core::{lock, Core, JobRequest},
    model::Disk,
    platform, process,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const BLOCKS: usize = 200;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub block_kib: u64,
    pub queue: u64,
    pub threads: u64,
    pub random: bool,
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub path: PathBuf,
    pub offset: u64,
    pub bytes: usize,
}

pub fn engine_info() -> Value {
    let available = cfg!(windows);
    let name = if available {
        "SSD Sentinel Direct-I/O (Win32 IOCP)"
    } else {
        "Desteklenmiyor (yalnız Windows)"
    };
    json!({"available":available,"name":name,"native":available})
}
pub fn check_engine() -> Result<(), String> {
    if cfg!(windows) {
        Ok(())
    } else {
        Err("Native Direct-I/O hız testi yalnız Windows'ta çalışır.".into())
    }
}

fn row(block: u64, queue: u64, threads: u64, random: bool) -> Row {
    let name = format!(
        "{}{}{} Q{queue}T{threads}",
        if random { "RND" } else { "SEQ" },
        if block >= 1024 { block / 1024 } else { block },
        if block >= 1024 { "MiB" } else { "KiB" }
    );
    Row {
        id: name.replace(' ', "-"),
        name,
        block_kib: block,
        queue,
        threads,
        random,
    }
}

pub fn profiles() -> std::collections::BTreeMap<String, Vec<Row>> {
    [
        (
            "standard",
            vec![
                row(1024, 8, 1, false),
                row(1024, 1, 1, false),
                row(4, 32, 1, true),
                row(4, 1, 1, true),
            ],
        ),
        (
            "nvme",
            vec![
                row(1024, 8, 1, false),
                row(128, 32, 1, false),
                row(4, 32, 16, true),
                row(4, 1, 1, true),
            ],
        ),
        ("real", vec![row(1024, 1, 1, false), row(4, 1, 1, true)]),
        ("peak", vec![row(1024, 8, 1, false), row(4, 32, 16, true)]),
        (
            "sustained",
            vec![row(1024, 8, 1, false), row(128, 32, 1, false)],
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}

/// Picks the most thorough safe budget (MiB) for a job so users never have to.
///
/// * Recovery deep carving: every free cluster (`0` = full volume, capped in the engine).
/// * Physical scan reads the whole disk regardless; magnetic / isolation: 256 MiB already
///   saturates the per-zone sample clamp.
/// * File read checks: 1 GiB of samples.
/// * Write-verify on SSD: 256 MiB to spare NAND; HDD: 1 GiB.
///
/// Write-based modes are additionally capped to 1/10 of free space (min 64 MiB).
pub fn auto_budget_mib(kind: &str, mode: &str, is_hdd: bool, free_bytes: Option<u64>) -> u64 {
    if kind == "recovery" {
        return 0;
    }
    let wanted: u64 = match mode {
        "physical_scan" | "magnetic_refresh" | "bad_zone_isolate" => 256,
        "data_refresh" if !is_hdd => 256,
        _ => 1024,
    };
    let writes = matches!(
        mode,
        "data_refresh" | "magnetic_refresh" | "bad_zone_isolate"
    );
    match (writes, free_bytes) {
        (true, Some(free)) => wanted.min((free / 10) / (1024 * 1024)).max(64),
        _ => wanted,
    }
}

pub fn validate_options(kind: &str, options: &Value) -> Result<(), String> {
    let Some(map) = options.as_object() else {
        return Err("İş ayarları nesne olmalı.".into());
    };
    let (keys, allowed): (&[&str], Vec<(&str, Vec<Value>)>) = match kind {
        "surface" => (
            &["mode", "size_mib"],
            vec![
                (
                    "mode",
                    vec![
                        json!("read_scan"),
                        json!("targeted_repair"),
                        json!("data_refresh"),
                        json!("physical_scan"),
                        json!("magnetic_refresh"),
                        json!("bad_zone_isolate"),
                    ],
                ),
                (
                    "size_mib",
                    vec![
                        json!("auto"),
                        json!(64),
                        json!(256),
                        json!(512),
                        json!(1024),
                    ],
                ),
            ],
        ),
        "defrag" => (
            &["mode"],
            vec![(
                "mode",
                vec![json!("analyze"), json!("defrag"), json!("consolidate")],
            )],
        ),
        "smart_tier" => (
            &["mode"],
            vec![(
                "mode",
                vec![json!("ssd_extent_optimize"), json!("hdd_outer_optimize")],
            )],
        ),
        "autopilot" => (&["profile"], vec![("profile", vec![json!("auto")])]),
        "recovery" => (
            &["mode", "size_mib"],
            vec![
                (
                    "mode",
                    vec![
                        json!("quick"),
                        json!("deep"),
                        json!("apps_only"),
                        json!("dev"),
                    ],
                ),
                (
                    "size_mib",
                    vec![
                        json!("auto"),
                        json!(256),
                        json!(1024),
                        json!(4096),
                        json!(16384),
                        json!(65536),
                        json!(0),
                    ],
                ),
            ],
        ),
        "benchmark" => (
            &[
                "profile", "mode", "size_mib", "repeats", "seconds", "interval", "data", "row",
            ],
            vec![
                (
                    "profile",
                    vec![
                        json!("standard"),
                        json!("nvme"),
                        json!("real"),
                        json!("peak"),
                        json!("sustained"),
                    ],
                ),
                (
                    "mode",
                    vec![json!("read"), json!("write"), json!("both"), json!("mix")],
                ),
                ("size_mib", vec![json!(256), json!(1024), json!(4096)]),
                ("repeats", vec![json!(1), json!(3), json!(5)]),
                ("seconds", vec![json!(5), json!(10), json!(30)]),
                ("interval", vec![json!(0), json!(5), json!(10)]),
                ("data", vec![json!("random"), json!("zero")]),
            ],
        ),
        _ => return Err("Geçersiz iş türü.".into()),
    };
    let optional_keys: &[&str] = if kind == "surface" { &["resume"] } else { &[] };
    if keys.iter().any(|k| !map.contains_key(*k))
        || map
            .keys()
            .any(|k| !keys.contains(&k.as_str()) && !optional_keys.contains(&k.as_str()))
    {
        return Err("Beklenmeyen / eksik iş ayarı.".into());
    }
    if kind == "surface" {
        if let Some(resume) = map.get("resume") {
            if !resume.is_boolean() {
                return Err("Geçersiz iş ayarı: resume".into());
            }
        }
    }
    for (key, values) in allowed {
        if !values.contains(&options[key]) {
            return Err(format!("Geçersiz iş ayarı: {key}"));
        }
    }
    if kind == "benchmark" {
        let p = profiles();
        let rows = &p[options["profile"].as_str().unwrap_or("")];
        if options["row"] != "all" && !rows.iter().any(|r| options["row"] == r.id) {
            return Err("Geçersiz test satırı.".into());
        }
    }
    Ok(())
}

#[cfg(windows)]
mod native_io {
    use super::*;
    use std::marker::PhantomPinned;
    use std::pin::Pin;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED,
        FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
    };
    use windows_sys::Win32::System::IO::{
        CancelIoEx, CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
    };

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    struct PageAlignedBuf {
        ptr: *mut u8,
        len: usize,
    }
    impl PageAlignedBuf {
        fn alloc(len: usize, zero: bool) -> Result<Self, String> {
            let ptr = unsafe { VirtualAlloc(null(), len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) }
                as *mut u8;
            if ptr.is_null() {
                return Err("Doğrudan I/O tamponu ayrılamadı.".into());
            }
            if !zero {
                let slice = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
                getrandom::fill(slice).map_err(|e| {
                    unsafe {
                        VirtualFree(ptr.cast(), 0, MEM_RELEASE);
                    }
                    e.to_string()
                })?;
            }
            Ok(Self { ptr, len })
        }
    }
    impl Drop for PageAlignedBuf {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                unsafe {
                    VirtualFree(self.ptr.cast(), 0, MEM_RELEASE);
                }
            }
        }
    }

    #[repr(C)]
    struct IoSlot {
        overlapped: OVERLAPPED,
        buf: PageAlignedBuf,
        started: Instant,
        is_write: bool,
        active: bool,
        _pin: PhantomPinned,
    }

    fn xorshift64(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    #[derive(Default)]
    struct WorkerStats {
        read_bytes: u128,
        write_bytes: u128,
        reads: u128,
        writes: u128,
        latency_ns_sum: u128,
    }

    #[allow(clippy::too_many_arguments)]
    fn submit_slot(
        handle: HANDLE,
        mut slot: Pin<&mut IoSlot>,
        total_blocks: u64,
        random: bool,
        direction: &str,
        rng: &mut u64,
        seq: &mut u64,
    ) -> Result<(), String> {
        let slot_ref = unsafe { slot.as_mut().get_unchecked_mut() };
        slot_ref.overlapped = unsafe { std::mem::zeroed() };
        let block_idx = if random {
            xorshift64(rng) % total_blocks
        } else {
            let cur = *seq % total_blocks;
            *seq = seq.wrapping_add(1);
            cur
        };
        let offset = block_idx * slot_ref.buf.len as u64;
        slot_ref.overlapped.Anonymous.Anonymous.Offset = offset as u32;
        slot_ref.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        let is_write = match direction {
            "write" => true,
            "mix" => (xorshift64(rng) % 10) < 3,
            _ => false,
        };
        slot_ref.is_write = is_write;
        slot_ref.started = Instant::now();
        let ov_ptr = &raw mut slot_ref.overlapped;
        let len = slot_ref.buf.len as u32;
        let ok = unsafe {
            if is_write {
                WriteFile(handle, slot_ref.buf.ptr, len, null_mut(), ov_ptr)
            } else {
                ReadFile(handle, slot_ref.buf.ptr, len, null_mut(), ov_ptr)
            }
        };
        if ok != 0 || unsafe { GetLastError() } == ERROR_IO_PENDING {
            slot_ref.active = true;
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_worker(
        worker_id: usize,
        path_w: &[u16],
        file_size: u64,
        block_bytes: usize,
        queue: usize,
        random: bool,
        direction: &str,
        deadline: Instant,
        zero_data: bool,
        cancel: &AtomicBool,
    ) -> Result<WorkerStats, String> {
        let access = GENERIC_READ
            | if direction == "read" {
                0
            } else {
                GENERIC_WRITE
            };
        let raw_handle = unsafe {
            CreateFileW(
                path_w.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH | FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        if raw_handle == INVALID_HANDLE_VALUE || raw_handle.is_null() {
            return Err(format!(
                "Doğrudan I/O hedefi açılamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        let handle = OwnedHandle(raw_handle);
        let raw_iocp = unsafe { CreateIoCompletionPort(handle.0, null_mut(), 0, 1) };
        if raw_iocp.is_null() {
            return Err(format!(
                "IOCP oluşturulamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        let iocp = OwnedHandle(raw_iocp);

        let mut slots = Vec::with_capacity(queue);
        for _ in 0..queue {
            let buf = PageAlignedBuf::alloc(block_bytes, zero_data)?;
            slots.push(Box::pin(IoSlot {
                overlapped: unsafe { std::mem::zeroed() },
                buf,
                started: Instant::now(),
                is_write: false,
                active: false,
                _pin: PhantomPinned,
            }));
        }

        let total_blocks = (file_size / block_bytes as u64).max(1);
        let mut rng = 0x9E37_79B9_7F4A_7C15u64 ^ ((worker_id as u64 + 1) * 0xBF58_476D_1CE4_E5B9);
        let mut seq = (worker_id as u64 * (total_blocks / 16).max(1)) % total_blocks;
        let mut in_flight = 0usize;
        let mut stopping = false;
        let mut cancelled_io = false;
        let mut aborted_by_cancel = false;
        let mut worker_err: Option<String> = None;
        let mut stats = WorkerStats::default();

        for slot in &mut slots {
            if cancel.load(Ordering::SeqCst) {
                aborted_by_cancel = true;
                stopping = true;
                break;
            }
            if Instant::now() >= deadline {
                stopping = true;
                break;
            }
            match submit_slot(
                handle.0,
                slot.as_mut(),
                total_blocks,
                random,
                direction,
                &mut rng,
                &mut seq,
            ) {
                Ok(()) => in_flight += 1,
                Err(e) => {
                    worker_err = Some(e);
                    stopping = true;
                    break;
                }
            }
        }

        if stopping && in_flight > 0 && (aborted_by_cancel || worker_err.is_some()) {
            unsafe {
                CancelIoEx(handle.0, null());
            }
            cancelled_io = true;
        }

        while in_flight > 0 {
            if !stopping && (cancel.load(Ordering::SeqCst) || Instant::now() >= deadline) {
                stopping = true;
                if cancel.load(Ordering::SeqCst) {
                    aborted_by_cancel = true;
                    if !cancelled_io {
                        unsafe {
                            CancelIoEx(handle.0, null());
                        }
                        cancelled_io = true;
                    }
                }
            }

            let mut bytes_xfer = 0u32;
            let mut key = 0usize;
            let mut ov_ptr: *mut OVERLAPPED = null_mut();
            let timeout_ms = if stopping { 500 } else { 50 };
            let gqcs_ok = unsafe {
                GetQueuedCompletionStatus(
                    iocp.0,
                    &mut bytes_xfer,
                    &mut key,
                    &mut ov_ptr,
                    timeout_ms,
                )
            };
            if !ov_ptr.is_null() {
                if let Some(idx) = slots
                    .iter()
                    .position(|s| (&raw const s.overlapped) == ov_ptr.cast_const())
                {
                    let slot_mut = unsafe { slots[idx].as_mut().get_unchecked_mut() };
                    slot_mut.active = false;
                    in_flight -= 1;
                    if gqcs_ok != 0 {
                        if !aborted_by_cancel && worker_err.is_none() {
                            let ns = slot_mut.started.elapsed().as_nanos();
                            if slot_mut.is_write {
                                stats.writes += 1;
                                stats.write_bytes += u128::from(bytes_xfer);
                            } else {
                                stats.reads += 1;
                                stats.read_bytes += u128::from(bytes_xfer);
                            }
                            stats.latency_ns_sum += ns;
                        }
                    } else if !aborted_by_cancel && !cancelled_io && worker_err.is_none() {
                        worker_err = Some(std::io::Error::last_os_error().to_string());
                        stopping = true;
                        unsafe {
                            CancelIoEx(handle.0, null());
                        }
                        cancelled_io = true;
                    }

                    if !stopping && !cancel.load(Ordering::SeqCst) && Instant::now() < deadline {
                        if let Err(e) = submit_slot(
                            handle.0,
                            slots[idx].as_mut(),
                            total_blocks,
                            random,
                            direction,
                            &mut rng,
                            &mut seq,
                        ) {
                            worker_err = Some(e);
                            stopping = true;
                            if !cancelled_io {
                                unsafe {
                                    CancelIoEx(handle.0, null());
                                }
                                cancelled_io = true;
                            }
                        } else {
                            in_flight += 1;
                        }
                    }
                }
            } else if cancel.load(Ordering::SeqCst) {
                aborted_by_cancel = true;
                stopping = true;
                if !cancelled_io {
                    unsafe {
                        CancelIoEx(handle.0, null());
                    }
                    cancelled_io = true;
                }
            } else if Instant::now() >= deadline {
                stopping = true;
                if Instant::now() >= deadline + Duration::from_secs(2) && !cancelled_io {
                    unsafe {
                        CancelIoEx(handle.0, null());
                    }
                    cancelled_io = true;
                }
            }
        }

        // Handles drop strictly before slots so no IRP references freed memory.
        drop(iocp);
        drop(handle);
        drop(slots);

        if aborted_by_cancel || cancel.load(Ordering::SeqCst) {
            return Err(process::CANCELLED.into());
        }
        if let Some(err) = worker_err {
            return Err(err);
        }
        Ok(stats)
    }

    pub fn run(
        path: &Path,
        file_size: u64,
        row: &Row,
        direction: &str,
        seconds: u64,
        zero_data: bool,
        cancel: &AtomicBool,
    ) -> Result<Value, String> {
        if !matches!(direction, "read" | "write" | "mix") {
            return Err("Geçersiz benchmark yönü.".into());
        }
        let block_bytes = (row.block_kib as usize).saturating_mul(1024).max(4096);
        let workers = (row.threads as usize).clamp(1, 16);
        let queue = (row.queue as usize).clamp(1, 128);
        let path_w: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let started = Instant::now();
        let deadline = started + Duration::from_secs(seconds.max(1));

        let all_stats = std::thread::scope(|scope| -> Result<Vec<WorkerStats>, String> {
            let mut handles = Vec::with_capacity(workers);
            for id in 0..workers {
                let pw = &path_w;
                handles.push(scope.spawn(move || {
                    run_worker(
                        id,
                        pw,
                        file_size,
                        block_bytes,
                        queue,
                        row.random,
                        direction,
                        deadline,
                        zero_data,
                        cancel,
                    )
                }));
            }
            let mut results = Vec::with_capacity(workers);
            for h in handles {
                results.push(h.join().map_err(|_| "I/O işçisi beklenmeden sonlandı.")??);
            }
            Ok(results)
        })?;

        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let mut total_read = 0u128;
        let mut total_write = 0u128;
        let mut total_ops = 0u128;
        let mut total_lat_ns = 0u128;
        for s in all_stats {
            total_read += s.read_bytes;
            total_write += s.write_bytes;
            total_ops += s.reads + s.writes;
            total_lat_ns += s.latency_ns_sum;
        }
        let latency_us = if total_ops > 0 {
            Some((total_lat_ns as f64 / total_ops as f64) / 1000.0)
        } else {
            None
        };
        let mb_s = (total_read + total_write) as f64 / elapsed / 1_000_000.0;
        let iops = total_ops as f64 / elapsed;
        Ok(json!({
            "mb_s": mb_s,
            "iops": iops,
            "latency_us": latency_us,
            "seconds": elapsed,
            "read_bytes": total_read.to_string(),
            "write_bytes": total_write.to_string()
        }))
    }
}

pub fn run_native_io_step(
    path: &Path,
    file_size: u64,
    row: &Row,
    direction: &str,
    seconds: u64,
    zero_data: bool,
    cancel: &AtomicBool,
) -> Result<Value, String> {
    #[cfg(windows)]
    {
        native_io::run(path, file_size, row, direction, seconds, zero_data, cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, file_size, row, direction, seconds, zero_data, cancel);
        Err("Native Direct-I/O hız testi yalnız Windows'ta çalışır.".into())
    }
}

fn defrag(core: &Core, request: &JobRequest, disk: &Disk) -> Result<(), String> {
    let en = core.is_en();
    if disk.kind != "HDD" {
        return Err(core
            .t(
                "SSD'lerde disk birleştirme (Defrag) NAND ömrünü kısaltır ve engellenmiştir; ReTrim kullanın.",
                "Defragmenting SSDs shortens NAND lifetime and is blocked; use ReTrim.",
            )
            .into());
    }
    if !platform::is_admin() {
        return Err(core
            .t(
                "Disk birleştirme / analiz işlemi yönetici erişimi gerektirir.",
                "Defragmentation / analysis requires administrator access.",
            )
            .into());
    }
    let mode = request.options["mode"].as_str().unwrap_or("analyze");
    let (flag, title_tr, title_en) = match mode {
        "defrag" => (
            "/O",
            "HDD akıllı birleştirme · defrag /O",
            "HDD smart optimization · defrag /O",
        ),
        "consolidate" => (
            "/X",
            "HDD boş alan birleştirme · defrag /X",
            "HDD free-space consolidation · defrag /X",
        ),
        _ => (
            "/A",
            "HDD parçalanma analizi · defrag /A",
            "HDD fragmentation analysis · defrag /A",
        ),
    };
    lock(&core.job)["total"] = json!(1);
    phase_bi(core, title_tr, title_en);
    let output = process::run(
        &platform::system_executable("defrag.exe"),
        &[format!("{}:", request.letter), flag.into(), "/V".into()],
        Duration::from_secs(1800),
        &core.job_cancel,
    )?;
    let mut job = lock(&core.job);
    job["completed"] = json!(1);
    job["progress"] = json!(100);
    job["repair_summary"] = json!({
        "mode": mode,
        "code": output.code,
        "output": output.stdout.chars().take(12000).collect::<String>()
    });
    if output.code != 0 {
        return Err(if en {
            format!("defrag.exe returned error code: {}", output.code)
        } else {
            format!("defrag.exe hata kodu döndürdü: {}", output.code)
        });
    }
    Ok(())
}

pub fn command(path: &Path, row: &Row, direction: &str, options: &Value) -> Vec<String> {
    let write = match direction {
        "write" => 100,
        "mix" => 30,
        _ => 0,
    };
    let mut args = vec![
        format!("-b{}K", row.block_kib),
        format!("-o{}", row.queue),
        format!("-t{}", row.threads),
        "-W0".into(),
        "-Su".into(),
        format!("-w{write}"),
        format!("-d{}", options["seconds"]),
        "-L".into(),
        "-Rxml".into(),
    ];
    if row.random {
        args.push("-r".into());
    }
    if write > 0 {
        args.push(
            if options["data"] == "zero" {
                "-Z"
            } else {
                "-Z1M"
            }
            .into(),
        );
    }
    args.push(path.to_string_lossy().into_owned());
    args
}

pub fn parse_benchmark(
    xml: &str,
    path: &Path,
    direction: &str,
    row: &Row,
) -> Result<Value, String> {
    let error = || "DiskSpd XML ölçüm sözleşmesi geçersiz.".to_owned();
    if xml.len() > 8 * 1024 * 1024
        || xml.to_uppercase().contains("<!DOCTYPE")
        || xml.to_uppercase().contains("<!ENTITY")
        || !matches!(direction, "read" | "write" | "mix")
    {
        return Err(error());
    }
    let xml = strip_score(xml).0.trim();
    let doc = roxmltree::Document::parse(xml).map_err(|_| error())?;
    let root = doc.root_element();
    let spans: Vec<_> = root
        .children()
        .filter(|n| n.has_tag_name("TimeSpan"))
        .collect();
    if !root.has_tag_name("Results") || spans.len() != 1 {
        return Err(error());
    }
    let span = spans[0];
    let child_text = |node: roxmltree::Node<'_, '_>, tag: &str| -> Option<String> {
        node.children()
            .find(|n| n.has_tag_name(tag))
            .and_then(|n| n.text())
            .map(str::to_owned)
    };
    let seconds = child_text(span, "TestTimeSeconds")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .ok_or_else(error)?;
    let targets: Vec<_> = span
        .children()
        .filter(|n| n.has_tag_name("Thread"))
        .flat_map(|t| t.children().filter(|n| n.has_tag_name("Target")))
        .collect();
    if targets.is_empty() || targets.len() > 256 {
        return Err(error());
    }
    let mut totals = [0u128; 4];
    for target in targets {
        let target_path = child_text(target, "Path").ok_or_else(error)?;
        if Path::new(&target_path) != path {
            return Err("Sonuç başka bir hedef dosyaya ait.".into());
        }
        for (i, key) in ["ReadBytes", "WriteBytes", "ReadCount", "WriteCount"]
            .iter()
            .enumerate()
        {
            let s = child_text(target, key).ok_or_else(error)?;
            if s.is_empty() || s.len() > 30 || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(error());
            }
            let n = s.parse::<u128>().map_err(|_| error())?;
            totals[i] = totals[i].checked_add(n).ok_or_else(error)?;
        }
    }
    let [read_bytes, write_bytes, reads, writes] = totals;
    if (direction == "read" && (reads == 0 || writes > 0 || write_bytes > 0))
        || (direction == "write" && (writes == 0 || reads > 0 || read_bytes > 0))
        || (direction == "mix" && (reads == 0 || writes == 0))
    {
        return Err(error());
    }
    if reads.checked_mul(u128::from(row.block_kib) * 1024) != Some(read_bytes)
        || writes.checked_mul(u128::from(row.block_kib) * 1024) != Some(write_bytes)
    {
        return Err("Blok boyutu / I/O sayacı uyuşmuyor.".into());
    }
    let tag = format!(
        "Average{}Milliseconds",
        match direction {
            "read" => "Read",
            "write" => "Write",
            _ => "Total",
        }
    );
    let latency = span
        .children()
        .find(|n| n.has_tag_name("Latency"))
        .and_then(|n| child_text(n, &tag));
    let latency_us = if let Some(s) = latency {
        Some(
            s.parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && *n >= 0.0)
                .ok_or_else(error)?
                * 1000.0,
        )
    } else {
        None
    };
    let mb_s = (read_bytes + write_bytes) as f64 / seconds / 1_000_000.0;
    let iops = (reads + writes) as f64 / seconds;
    if !mb_s.is_finite() || !iops.is_finite() || latency_us.is_some_and(|x| !x.is_finite()) {
        return Err(error());
    }
    Ok(
        json!({"mb_s":mb_s,"iops":iops,"latency_us":latency_us,"seconds":seconds,"read_bytes":read_bytes.to_string(),"write_bytes":write_bytes.to_string()}),
    )
}

fn strip_score(xml: &str) -> (&str, Option<u32>) {
    if let Some((body, score)) = xml.trim_end().rsplit_once("Score: ") {
        if let Ok(score) = score.trim().parse::<u32>() {
            return (body, Some(score));
        }
    }
    (xml, None)
}

pub fn candidate_samples(root: &Path, budget: u64) -> Result<Vec<Sample>, String> {
    candidate_samples_with_cancel(root, budget, &std::sync::atomic::AtomicBool::new(false))
}

pub fn candidate_samples_with_cancel(
    root: &Path,
    budget: u64,
    stop: &std::sync::atomic::AtomicBool,
) -> Result<Vec<Sample>, String> {
    let check = || {
        if stop.load(Ordering::SeqCst) {
            Err(process::CANCELLED.to_owned())
        } else {
            Ok(())
        }
    };
    check()?;
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut dirs = vec![root.to_path_buf()];
    let mut files = Vec::new();
    let mut bytes = 0;
    let mut visited = 0;
    while let Some(dir) = dirs.pop() {
        check()?;
        if platform::reparse(&dir) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            check()?;
            visited += 1;
            if visited > 20000 {
                break;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.starts_with("ssd-sentinel-")
                || matches!(
                    name.as_str(),
                    "$recycle.bin"
                        | "system volume information"
                        | "windows"
                        | "pagefile.sys"
                        | "hiberfil.sys"
                        | "swapfile.sys"
                )
                || platform::reparse(&path)
            {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                if dirs.len() < 1024 {
                    dirs.push(path);
                }
                continue;
            }
            if !meta.is_file() || meta.len() == 0 {
                continue;
            }
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            if !canonical.starts_with(&canonical_root) {
                continue;
            }
            let size = meta
                .len()
                .min(4 * 1024 * 1024)
                .min(budget.saturating_sub(bytes));
            if size > 0 {
                files.push((path, size));
                bytes += size;
            }
            if bytes >= budget || files.len() >= BLOCKS {
                break;
            }
        }
        if bytes >= budget || files.len() >= BLOCKS || visited > 20000 {
            break;
        }
    }
    let chunk = (budget / BLOCKS as u64).clamp(4096, 4 * 1024 * 1024);
    let mut samples = Vec::new();
    for (path, size) in files {
        let mut offset = 0;
        while offset < size && samples.len() < BLOCKS {
            let n = (size - offset).min(chunk);
            samples.push(Sample {
                path: path.clone(),
                offset,
                bytes: n as usize,
            });
            offset += n;
        }
        if samples.len() >= BLOCKS {
            break;
        }
    }
    Ok(samples)
}

pub fn validate_target(
    core: &Core,
    disk: &Disk,
    letter: &str,
    needed: u64,
    refresh: bool,
    ntfs_only: bool,
) -> Result<PathBuf, String> {
    core.live_disk(&disk.id)?;
    let inventory = platform::inventory(&core.job_cancel)?;
    let mapped = platform::verify_disk_mapping(&core.disks()?, disk, &inventory)?;
    let volumes: Vec<_> = inventory["disks"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|d| d["volumes"].as_array().into_iter().flatten())
        .filter(|v| v["letter"] == letter)
        .collect();
    if mapped.os_disk_number != disk.os_disk_number
        || mapped.os_serial != disk.os_serial
        || mapped.os_model != disk.os_model
        || mapped.os_disk_number.is_none()
        || !mapped.volumes.iter().any(|v| v["letter"] == letter)
        || volumes.len() != 1
    {
        return Err("Bölüm kimliği değişti veya eşleme belirsiz.".into());
    }
    let volume = volumes[0];
    platform::check_filesystem(volume, ntfs_only)?;
    if needed > 0 {
        let total = volume["size_bytes"]
            .as_u64()
            .ok_or("Bölüm kapasitesi bilinmiyor.")?;
        let free = volume["free_bytes"]
            .as_u64()
            .ok_or("Boş alan bilinmiyor.")?;
        let reserve = if refresh {
            (512 * 1024 * 1024).max(total / 100 * 3)
        } else {
            (1024 * 1024 * 1024).max(total / 20)
        };
        if free < needed.saturating_add(reserve) {
            return Err("Geçici dosya ve güvenli boş alan payı için yeterli yer yok.".into());
        }
    }
    platform::volume_root(letter)
}

pub(crate) struct OwnedArea {
    directory: PathBuf,
    pub(crate) path: PathBuf,
    directory_identity: same_file::Handle,
    file_identity: Option<same_file::Handle>,
    directory_pin: Option<File>,
    file_pin: Option<File>,
}

impl OwnedArea {
    pub(crate) fn create(root: &Path) -> Result<Self, String> {
        let directory = root.join(format!("SSD-Sentinel-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
        let directory_identity =
            same_file::Handle::from_path(&directory).map_err(|e| e.to_string())?;
        let directory_pin = Some(platform::pin_directory(&directory)?);
        Ok(Self {
            path: directory.join("payload.bin"),
            directory,
            directory_identity,
            file_identity: None,
            directory_pin,
            file_pin: None,
        })
    }
    pub(crate) fn open(&mut self) -> Result<File, String> {
        self.check()?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(3);
        }
        let file = options.open(&self.path).map_err(|e| e.to_string())?;
        self.file_pin = Some(file.try_clone().map_err(|e| e.to_string())?);
        self.file_identity =
            Some(same_file::Handle::from_path(&self.path).map_err(|e| e.to_string())?);
        self.check()?;
        Ok(file)
    }
    pub(crate) fn check(&self) -> Result<(), String> {
        if platform::reparse(&self.directory)
            || same_file::Handle::from_path(&self.directory).map_err(|e| e.to_string())?
                != self.directory_identity
        {
            return Err("Geçici klasör sahipliği değişti; erişim / silme reddedildi.".into());
        }
        if let Some(identity) = &self.file_identity {
            if platform::reparse(&self.path)
                || same_file::Handle::from_path(&self.path).map_err(|e| e.to_string())? != *identity
            {
                return Err("Geçici dosya sahipliği değişti; erişim / silme reddedildi.".into());
            }
        }
        Ok(())
    }
    pub(crate) fn cleanup(mut self) -> Result<(), String> {
        self.check()?;
        self.file_pin.take();
        if let Some(identity) = self.file_identity.take() {
            platform::delete_owned(&self.path, &identity)
                .map_err(|e| format!("{}: {e}", self.path.display()))?;
            drop(identity);
        }
        self.directory_pin.take();
        platform::delete_owned(&self.directory, &self.directory_identity)
            .map_err(|e| format!("{}: {e}", self.directory.display()))
    }
}

pub(crate) fn cancelled(core: &Core) -> Result<(), String> {
    if core.stop.load(Ordering::SeqCst) || core.job_cancel.load(Ordering::SeqCst) {
        Err(process::CANCELLED.into())
    } else {
        Ok(())
    }
}
pub(crate) fn phase(core: &Core, text: &str) {
    let mut job = lock(&core.job);
    job["status"] = json!("running");
    job["phase"] = json!(text);
}
/// Bilingual progress text: picks the Turkish or English phase for the active UI language.
pub(crate) fn phase_bi(core: &Core, tr: &str, en: &str) {
    phase(core, crate::i18n::pick(core.is_en(), tr, en));
}
fn increment(job: &mut Value, key: &str, amount: u64) {
    job[key] = json!(job[key].as_u64().unwrap_or(0).saturating_add(amount));
}

/// Safety gate before running HDD `defrag /O` inside Autopilot.
/// Never stress a failing mechanical drive with heavy head seeks when SMART reports
/// critical status, pending (#197), uncorrectable (#198), or >= 50 reallocated (#5) sectors.
pub fn autopilot_hdd_defrag_guard(disk: &Disk) -> Option<String> {
    autopilot_hdd_defrag_guard_for_lang(disk, false)
}

pub fn autopilot_hdd_defrag_guard_for_lang(disk: &Disk, en: bool) -> Option<String> {
    let snap = crate::surface_heal::SmartSnapshot::from_disk(disk);
    let mut reasons = Vec::new();
    if disk.health.status == "critical" {
        reasons.push(
            crate::i18n::pick(
                en,
                "disk sağlık durumu kritik",
                "drive health status is critical",
            )
            .to_owned(),
        );
    }
    if snap.pending_197 > 0 {
        reasons.push(if en {
            format!("{} pending sectors (#197)", snap.pending_197)
        } else {
            format!("{} bekleyen sektör (#197)", snap.pending_197)
        });
    }
    if snap.uncorrectable_198 > 0 {
        reasons.push(if en {
            format!("{} uncorrectable sectors (#198)", snap.uncorrectable_198)
        } else {
            format!("{} düzeltilemeyen sektör (#198)", snap.uncorrectable_198)
        });
    }
    if snap.reallocated_5 >= 50 {
        reasons.push(if en {
            format!("{} reallocated sectors (#5)", snap.reallocated_5)
        } else {
            format!("{} yeniden eşlenen sektör (#5)", snap.reallocated_5)
        });
    }
    if reasons.is_empty() {
        return None;
    }
    Some(if en {
        format!(
            "Safety warning: HDD defragmentation (defrag /O) was skipped ({}). To avoid stressing a failing drive, run a Full Surface Scan and Pinpoint Repair first, then back up your important data.",
            reasons.join(", ")
        )
    } else {
        format!(
            "Güvenlik uyarısı: HDD birleştirme (defrag /O) atlandı ({}). Arızalı diski zorlamamak için önce Tam Yüzey Taraması ve Nokta Atışı Onarım çalıştırın, ardından önemli verilerinizi yedekleyin.",
            reasons.join(", ")
        )
    })
}

/// Read-only advisory steps appended by Autopilot on SSDs when large hibernation
/// files (> 4 GiB) or low over-provisioning (< 5%) are detected. Never mutates silently.
pub fn autopilot_ssd_advisories(disk: &Disk, windows: &Value) -> Vec<String> {
    autopilot_ssd_advisories_for_lang(disk, windows, false)
}

pub fn autopilot_ssd_advisories_for_lang(disk: &Disk, windows: &Value, en: bool) -> Vec<String> {
    const FOUR_GIB: u64 = 4 * 1024 * 1024 * 1024;
    let mut notes = Vec::new();
    if let Some(hiber_bytes) = windows["system_tuning"]["hiberfil_bytes"]
        .as_u64()
        .filter(|b| *b > FOUR_GIB)
    {
        let gib = hiber_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        notes.push(if en {
            format!(
                "Tip: The hibernation file (hiberfil.sys) uses {gib:.1} GiB. If you do not use hibernation, you can turn it off under Optimize → Manual tuning to gain free space and write endurance on the SSD."
            )
        } else {
            format!(
                "Öneri: Hazırda bekletme dosyası (hiberfil.sys) {gib:.1} GiB yer kaplıyor. Hazırda bekletmeyi kullanmıyorsanız Hızlandır → Elle Ayarla bölümünden kapatarak SSD'de boş alan ve yazma dayanımı kazanabilirsiniz."
            )
        });
    }
    if let Some(op_pct) = disk.optimization["over_provisioning_percent"]
        .as_f64()
        .filter(|p| p.is_finite() && *p < 5.0)
    {
        notes.push(if en {
            format!(
                "Tip: SSD unallocated reserve (Over-Provisioning) is {op_pct:.1}% (below 5%). For long-term write speed and NAND endurance, consider reserving 7–10% Over-Provisioning under Optimize → Manual tuning."
            )
        } else {
            format!(
                "Öneri: SSD ayrılmamış koruma alanı (Over-Provisioning) %{op_pct:.1} düzeyinde (%5'in altında). Uzun vadeli yazma hızı ve NAND ömrü için Hızlandır → Elle Ayarla altından %7–%10 Over-Provisioning ayırmayı değerlendirebilirsiniz."
            )
        });
    }
    notes
}

fn run_autopilot(
    core: &Core,
    request: &JobRequest,
    disk: &Disk,
    root: &Path,
) -> Result<(), String> {
    let en = core.is_en();
    if !platform::is_admin() {
        return Err(core
            .t(
                "Tek Tıkla Otopilot için Yönetici erişimi gereklidir.",
                "One-click Autopilot requires administrator access.",
            )
            .into());
    }
    let _ = root;
    let is_hdd = disk.kind == "HDD";
    let mut steps = Vec::new();
    lock(&core.job)["total"] = json!(4);

    if is_hdd {
        phase_bi(
            core,
            "1/4 · Donanım yazma önbelleği ve APM kafa park kontrolü uygulanıyor",
            "1/4 · Applying hardware write cache and APM head-parking control",
        );
        if disk.optimization["write_cache"]["changeable"] == true
            && disk.optimization["write_cache"]["enabled"] != true
        {
            if let Ok(res) = crate::optimize::execute_unlocked(
                core,
                &crate::optimize::OptimizeRequest {
                    action: "write_cache".into(),
                    target: disk.id.clone(),
                    value: "enable".into(),
                    confirmation: format!("optimize:write_cache:{}", disk.id),
                },
            ) {
                steps.push(
                    res["message"]
                        .as_str()
                        .unwrap_or(core.t("Yazma önbelleği açıldı.", "Write cache enabled."))
                        .to_owned(),
                );
            }
        } else {
            steps.push(
                core.t(
                    "Donanım yazma önbelleği zaten optimum durumda.",
                    "Hardware write cache is already optimal.",
                )
                .into(),
            );
        }
        if crate::optimize::execute_unlocked(
            core,
            &crate::optimize::OptimizeRequest {
                action: "hdd_apm".into(),
                target: disk.id.clone(),
                value: "254".into(),
                confirmation: format!("optimize:hdd_apm:{}", disk.id),
            },
        )
        .is_ok()
        {
            steps.push(
                core.t(
                    "HDD APM seviyesi 254 (gereksiz kafa park azaltıldı) ayarlandı.",
                    "HDD APM level set to 254 (unnecessary head parking reduced).",
                )
                .into(),
            );
        }
        {
            let mut j = lock(&core.job);
            j["completed"] = json!(1);
            j["progress"] = json!(25);
        }
        cancelled(core)?;

        phase_bi(
            core,
            "2/4 · Çevrimiçi dosya sistemi bütünlüğü denetleniyor (chkdsk /scan)",
            "2/4 · Checking online file system integrity (chkdsk /scan)",
        );
        if disk
            .volumes
            .iter()
            .any(|v| v["letter"] == request.letter && v["filesystem"] == "NTFS")
        {
            let chk = process::run(
                &platform::system_executable("chkdsk.exe"),
                &[format!("{}:", request.letter), "/scan".into()],
                Duration::from_secs(600),
                &core.job_cancel,
            )?;
            steps.push(if en {
                format!(
                    "{}: NTFS online integrity scan completed (code {}).",
                    request.letter, chk.code
                )
            } else {
                format!(
                    "{}: NTFS çevrimiçi bütünlük taraması tamamlandı (kod {}).",
                    request.letter, chk.code
                )
            });
        }
        {
            let mut j = lock(&core.job);
            j["completed"] = json!(2);
            j["progress"] = json!(50);
        }
        cancelled(core)?;

        if let Some(warning) = autopilot_hdd_defrag_guard_for_lang(disk, en) {
            phase_bi(
                core,
                "3/4 · Güvenlik kontrolü: sağlık uyarısı nedeniyle defrag /O atlandı",
                "3/4 · Safety check: defrag /O skipped because of a health warning",
            );
            steps.push(warning);
        } else {
            phase_bi(
                core,
                "3/4 · HDD akıllı dosya & boş alan hızlandırması çalışıyor (defrag /O)",
                "3/4 · Running HDD smart file & free-space optimization (defrag /O)",
            );
            let defrag_out = process::run(
                &platform::system_executable("defrag.exe"),
                &[format!("{}:", request.letter), "/O".into(), "/V".into()],
                Duration::from_secs(1800),
                &core.job_cancel,
            )?;
            if defrag_out.code != 0 {
                return Err(if en {
                    format!("defrag /O returned error code: {}", defrag_out.code)
                } else {
                    format!("defrag /O hata kodu döndürdü: {}", defrag_out.code)
                });
            }
            steps.push(if en {
                format!(
                    "{}: volume arranged onto the fastest tracks with defrag /O.",
                    request.letter
                )
            } else {
                format!(
                    "{}: bölümü defrag /O ile en hızlı izlere düzenlendi.",
                    request.letter
                )
            });
        }
        {
            let mut j = lock(&core.job);
            j["completed"] = json!(3);
            j["progress"] = json!(85);
        }
    } else {
        phase_bi(
            core,
            "1/4 · Windows TRIM (DisableDeleteNotify) doğrulanıyor",
            "1/4 · Verifying Windows TRIM (DisableDeleteNotify)",
        );
        if lock(&core.data)["windows"]["trim_enabled"] != true {
            if let Ok(res) = crate::optimize::execute_unlocked(
                core,
                &crate::optimize::OptimizeRequest {
                    action: "ntfs_trim".into(),
                    target: "system".into(),
                    value: "enable".into(),
                    confirmation: "optimize:ntfs_trim:system".into(),
                },
            ) {
                steps.push(
                    res["message"]
                        .as_str()
                        .unwrap_or(core.t("TRIM etkinleştirildi.", "TRIM enabled."))
                        .to_owned(),
                );
            }
        } else {
            steps.push(
                core.t(
                    "Windows TRIM zaten etkin.",
                    "Windows TRIM is already enabled.",
                )
                .into(),
            );
        }
        {
            let mut j = lock(&core.job);
            j["completed"] = json!(1);
            j["progress"] = json!(25);
        }
        cancelled(core)?;

        phase_bi(
            core,
            "2/4 · PCIe ASPM düşük gecikme güç ayarı uygulanıyor",
            "2/4 · Applying low-latency PCIe ASPM power setting",
        );
        if lock(&core.data)["windows"]["system_tuning"]["pcie_aspm"] != 0 {
            if let Ok(res) = crate::optimize::execute_unlocked(
                core,
                &crate::optimize::OptimizeRequest {
                    action: "pcie_aspm".into(),
                    target: "system".into(),
                    value: "off".into(),
                    confirmation: "optimize:pcie_aspm:system".into(),
                },
            ) {
                steps.push(
                    res["message"]
                        .as_str()
                        .unwrap_or(core.t(
                            "PCIe ASPM tam performans moduna alındı.",
                            "PCIe ASPM switched to full performance mode.",
                        ))
                        .to_owned(),
                );
            }
        } else {
            steps.push(
                core.t(
                    "PCIe ASPM zaten düşük gecikme modunda.",
                    "PCIe ASPM is already in low-latency mode.",
                )
                .into(),
            );
        }
        {
            let mut j = lock(&core.job);
            j["completed"] = json!(2);
            j["progress"] = json!(50);
        }
        cancelled(core)?;

        phase_bi(
            core,
            "3/4 · Yazma önbelleği ve SSD ReTrim uygulanıyor",
            "3/4 · Applying write cache and SSD ReTrim",
        );
        if disk.optimization["write_cache"]["changeable"] == true
            && disk.optimization["write_cache"]["enabled"] != true
        {
            if let Ok(res) = crate::optimize::execute_unlocked(
                core,
                &crate::optimize::OptimizeRequest {
                    action: "write_cache".into(),
                    target: disk.id.clone(),
                    value: "enable".into(),
                    confirmation: format!("optimize:write_cache:{}", disk.id),
                },
            ) {
                steps.push(
                    res["message"]
                        .as_str()
                        .unwrap_or(core.t("Yazma önbelleği açıldı.", "Write cache enabled."))
                        .to_owned(),
                );
            }
        }
        let retrim_out = process::run(
            &platform::system_executable("defrag.exe"),
            &[format!("{}:", request.letter), "/L".into(), "/V".into()],
            Duration::from_secs(600),
            &core.job_cancel,
        );
        match retrim_out {
            Ok(out) if out.code == 0 => {
                steps.push(if en {
                    format!("{}: SSD ReTrim (/L) completed.", request.letter)
                } else {
                    format!("{}: SSD ReTrim (/L) tamamlandı.", request.letter)
                });
            }
            _ => steps.push(
                core.t(
                    "SSD ReTrim adımı sistem tarafından atlandı.",
                    "The SSD ReTrim step was skipped by the system.",
                )
                .into(),
            ),
        }
        {
            let data = lock(&core.data);
            steps.extend(autopilot_ssd_advisories_for_lang(
                disk,
                &data["windows"],
                en,
            ));
        }
        {
            let mut j = lock(&core.job);
            j["completed"] = json!(3);
            j["progress"] = json!(85);
        }
    }

    phase_bi(
        core,
        "4/4 · Otopilot özeti hazırlanıyor",
        "4/4 · Preparing the autopilot summary",
    );
    let mut j = lock(&core.job);
    j["completed"] = json!(4);
    j["progress"] = json!(100);
    j["repair_summary"] = json!({
        "mode": "autopilot",
        "disk_type": disk.kind,
        "steps": steps,
        "scope": if en {
            "One-click Autopilot: every step is applied with a SQLite rollback record."
        } else {
            "Tek Tıkla Otopilot: Tüm adımlar SQLite rollback kaydıyla uygulanır."
        }
    });
    Ok(())
}

pub fn run(core: &Core, request: &JobRequest, disk: &Disk) {
    let mut area = None;
    let result = (|| {
        cancelled(core)?;
        let mode = request.options["mode"].as_str().unwrap_or("");
        let needed = if request.kind == "benchmark"
            || (request.kind == "surface"
                && matches!(
                    mode,
                    "data_refresh" | "magnetic_refresh" | "bad_zone_isolate"
                )) {
            request.options["size_mib"].as_u64().unwrap_or(0) * 1024 * 1024
        } else {
            0
        };
        let root = validate_target(
            core,
            disk,
            &request.letter,
            needed,
            request.kind == "surface",
            request.kind == "smart_tier"
                || (request.kind == "surface"
                    && matches!(
                        mode,
                        "targeted_repair" | "magnetic_refresh" | "bad_zone_isolate"
                    )),
        )?;
        match request.kind.as_str() {
            "benchmark" => benchmark(core, request, disk, &root, &mut area),
            "defrag" => defrag(core, request, disk),
            "smart_tier" => crate::tiering::run_smart_tier_job(core, request, disk, &root),
            "autopilot" => run_autopilot(core, request, disk, &root),
            "recovery" => crate::recovery::run(core, request, disk, &root),
            _ => surface(core, request, disk, &root, &mut area),
        }
    })();
    let cleanup = area.map(OwnedArea::cleanup).transpose();
    let status = match &result {
        Ok(()) => "completed",
        Err(e) if e == process::CANCELLED => "cancelled",
        _ => "failed",
    };
    let saved = {
        let mut job = lock(&core.job);
        job["status"] = json!(if cleanup.is_err() { "failed" } else { status });
        let en = core.is_en();
        job["phase"] = json!(match (job["status"].as_str(), en) {
            (Some("completed"), false) => "Tamamlandı",
            (Some("completed"), true) => "Completed",
            (Some("cancelled"), false) => "İptal edildi",
            (Some("cancelled"), true) => "Cancelled",
            (_, false) => "Tamamlanamadı",
            (_, true) => "Failed",
        });
        job["finished_at"] = json!(Utc::now().to_rfc3339());
        if let Err(error) = result {
            job["error"] = json!(error);
        }
        if let Err(error) = cleanup {
            job["cleanup_error"] = json!(error);
        }
        job.clone()
    };
    if let Err(error) = lock(&core.store).save_job(&request.kind, &saved) {
        lock(&core.job)["error"] = json!(if core.is_en() {
            format!("Result could not be saved: {error}")
        } else {
            format!("Sonuç kaydedilemedi: {error}")
        });
    }
}

fn benchmark(
    core: &Core,
    request: &JobRequest,
    disk: &Disk,
    root: &Path,
    area: &mut Option<OwnedArea>,
) -> Result<(), String> {
    let options = &request.options;
    *area = Some(OwnedArea::create(root)?);
    let owned = area.as_mut().unwrap();
    let mut file = owned.open()?;
    let mut buffer = vec![0u8; 1024 * 1024];
    if options["data"] != "zero" {
        getrandom::fill(&mut buffer).map_err(|e| e.to_string())?;
    }
    phase_bi(
        core,
        "Geçici test dosyası hazırlanıyor · bu aşama yazma yapar",
        "Preparing temporary test file · this stage writes data",
    );
    for _ in 0..options["size_mib"].as_u64().unwrap_or(0) {
        cancelled(core)?;
        owned.check()?;
        file.write_all(&buffer).map_err(|e| e.to_string())?;
        increment(&mut lock(&core.job), "prepared_bytes", buffer.len() as u64);
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    let profiles = profiles();
    let rows: Vec<_> = profiles[options["profile"].as_str().unwrap_or("")]
        .iter()
        .filter(|r| options["row"] == "all" || options["row"] == r.id)
        .collect();
    let directions = if options["mode"] == "both" {
        vec!["read", "write"]
    } else {
        vec![options["mode"].as_str().unwrap_or("read")]
    };
    let repeats = options["repeats"].as_u64().unwrap_or(1);
    let total = rows.len() as u64 * directions.len() as u64 * (repeats + 1);
    lock(&core.job)["total"] = json!(total);
    for direction in directions {
        for row in &rows {
            let mut samples = Vec::new();
            for repeat in 0..=repeats {
                cancelled(core)?;
                validate_target(core, disk, &request.letter, 1, false, false)?;
                owned.check()?;
                let pass = if repeat == 0 {
                    None
                } else {
                    Some(format!("{repeat}/{repeats}"))
                };
                phase_bi(
                    core,
                    &format!(
                        "{} · {direction} · {}",
                        row.name,
                        pass.as_deref().unwrap_or("ısınma")
                    ),
                    &format!(
                        "{} · {direction} · {}",
                        row.name,
                        pass.as_deref().unwrap_or("warm-up")
                    ),
                );
                check_engine()?;
                let file_size = options["size_mib"].as_u64().unwrap_or(256) * 1024 * 1024;
                let measured = run_native_io_step(
                    &owned.path,
                    file_size,
                    row,
                    direction,
                    options["seconds"].as_u64().unwrap_or(5),
                    options["data"] == "zero",
                    &core.job_cancel,
                )?;
                cancelled(core)?;
                let mut job = lock(&core.job);
                increment(
                    &mut job,
                    "read_bytes",
                    measured["read_bytes"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .ok_or("Ölçülen byte sayısı sınırı aşıyor.")?,
                );
                increment(
                    &mut job,
                    "write_bytes",
                    measured["write_bytes"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .ok_or("Ölçülen byte sayısı sınırı aşıyor.")?,
                );
                increment(&mut job, "completed", 1);
                job["progress"] = json!(job["completed"].as_u64().unwrap_or(0) * 100 / total);
                if repeat > 0 {
                    samples.push(measured);
                    let best = samples
                        .iter()
                        .max_by(|a, b| {
                            a["mb_s"]
                                .as_f64()
                                .unwrap_or(0.0)
                                .total_cmp(&b["mb_s"].as_f64().unwrap_or(0.0))
                        })
                        .unwrap();
                    let results = job["results"].as_array_mut().unwrap();
                    results.retain(|r| !(r["row"]["id"] == row.id && r["direction"] == direction));
                    results.push(
                        json!({"row":row,"direction":direction,"samples":samples,"best":best}),
                    );
                }
            }
            for _ in 0..options["interval"].as_u64().unwrap_or(0) * 10 {
                cancelled(core)?;
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Ok(())
}

pub fn classify(latency: f64) -> &'static str {
    if latency < 5.0 {
        "fast"
    } else if latency < 25.0 {
        "normal"
    } else if latency <= 100.0 {
        "slow"
    } else {
        "warning"
    }
}

pub(crate) fn add_block(core: &Core, block: Value, read: u64, written: u64) {
    let mut job = lock(&core.job);
    increment(&mut job, "read_bytes", read);
    increment(&mut job, "write_bytes", written);
    increment(&mut job, "completed", 1);
    job["progress"] = json!(
        job["completed"].as_u64().unwrap_or(0) * 100 / job["total"].as_u64().unwrap_or(200).max(1)
    );
    job["blocks"].as_array_mut().unwrap().push(block);
    let blocks = job["blocks"].as_array().unwrap();
    let latencies: Vec<_> = blocks
        .iter()
        .filter_map(|b| b["latency_ms"].as_f64())
        .collect();
    job["summary"] = json!({"tested_blocks":blocks.len(),"bad_blocks":blocks.iter().filter(|b| b["status"] == "bad").count(),"warning_blocks":blocks.iter().filter(|b| b["status"] == "warning").count(),"avg_latency_ms":if latencies.is_empty(){None}else{Some(latencies.iter().sum::<f64>()/latencies.len() as f64)},"max_latency_ms":latencies.iter().cloned().reduce(f64::max)});
}

fn surface(
    core: &Core,
    request: &JobRequest,
    disk: &Disk,
    root: &Path,
    area: &mut Option<OwnedArea>,
) -> Result<(), String> {
    let mode = request.options["mode"].as_str().unwrap_or("read_scan");
    if matches!(
        mode,
        "physical_scan" | "magnetic_refresh" | "bad_zone_isolate"
    ) {
        return crate::surface_heal::run(core, request, disk, root, area);
    }
    let budget = request.options["size_mib"].as_u64().unwrap_or(64) * 1024 * 1024;
    let findings: Vec<_> = disk
        .smart
        .iter()
        .filter(|r| {
            matches!(r.id.as_str(), "5" | "196" | "197" | "198" | "media_errors")
                && crate::store::decimal_greater(&r.raw, "0")
        })
        .collect();
    lock(&core.job)["repair_summary"] = json!({"smart_findings":findings,"filesystem_check":null,"scope":core.t("Dosya tabanlı örnekleme; fiziksel LBA / tüm yüzey kapsamı yok. İşletim sistemi önbelleği gecikmeyi etkiler.","File-based sampling; no physical LBA / full-surface coverage. The operating system cache affects latency.")});
    if mode == "targeted_repair" {
        if !platform::is_admin() {
            return Err("Çevrimiçi dosya sistemi kontrolü yönetici erişimi gerektirir.".into());
        }
        if !disk
            .volumes
            .iter()
            .any(|v| v["letter"] == request.letter && v["filesystem"] == "NTFS")
        {
            return Err("chkdsk /scan yalnız NTFS bölümlerinde sunulur.".into());
        }
        phase_bi(
            core,
            "Windows çevrimiçi NTFS kontrolü · chkdsk /scan",
            "Windows online NTFS check · chkdsk /scan",
        );
        let result = process::run(
            &platform::system_executable("chkdsk.exe"),
            &[format!("{}:", request.letter), "/scan".into()],
            Duration::from_secs(600),
            &core.job_cancel,
        )?;
        lock(&core.job)["repair_summary"]["filesystem_check"] = json!({"ok":result.code==0,"code":result.code,"output":result.stdout.chars().take(8000).collect::<String>()});
        if result.code != 0 {
            return Err(format!(
                "Dosya sistemi kontrolü uyarı/hata kodu döndürdü: {}. Günlüğü inceleyin.",
                result.code
            ));
        }
    }
    if mode == "data_refresh" {
        *area = Some(OwnedArea::create(root)?);
        let owned = area.as_mut().unwrap();
        let mut file = owned.open()?;
        let bytes = budget / BLOCKS as u64;
        let mut buffer = vec![0u8; bytes as usize];
        let mut read = vec![0u8; bytes as usize];
        for i in 0..BLOCKS {
            cancelled(core)?;
            owned.check()?;
            if i % 20 == 0 {
                validate_target(core, disk, &request.letter, 1, true, false)?;
            }
            getrandom::fill(&mut buffer).map_err(|e| e.to_string())?;
            phase_bi(
                core,
                &format!("Geçici dosya doğrulaması · {}/{BLOCKS}", i + 1),
                &format!("Temporary file verification · {}/{BLOCKS}", i + 1),
            );
            let start = Instant::now();
            file.seek(SeekFrom::Start(i as u64 * bytes))
                .map_err(|e| e.to_string())?;
            file.write_all(&buffer).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            file.seek(SeekFrom::Start(i as u64 * bytes))
                .map_err(|e| e.to_string())?;
            file.read_exact(&mut read).map_err(|e| e.to_string())?;
            if read != buffer {
                return Err("Yazılan ve geri okunan blok eşleşmedi.".into());
            }
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            add_block(
                core,
                json!({"index":i,"status":classify(elapsed),"latency_ms":elapsed,"mb_s":bytes as f64/elapsed/1000.0,"offset_bytes":i as u64*bytes,"bytes":bytes,"file":core.t("Geçici doğrulama dosyası","Temporary verification file"),"error":null}),
                bytes,
                bytes,
            );
        }
        return Ok(());
    }
    phase_bi(
        core,
        "Erişilebilir dosyalar örnekleniyor · fiziksel sektör taraması değil",
        "Sampling accessible files · not a physical sector scan",
    );
    let samples = candidate_samples_with_cancel(root, budget, &core.job_cancel)?;
    if samples.is_empty() {
        return Err("Okunabilir dosya örneği bulunamadı. Hiçbir blok ölçülmedi.".into());
    }
    lock(&core.job)["total"] = json!(samples.len());
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    for (index, sample) in samples.iter().enumerate() {
        cancelled(core)?;
        let rel = sample
            .path
            .strip_prefix(root)
            .unwrap_or(&sample.path)
            .to_string_lossy()
            .into_owned();
        let start = Instant::now();
        let result = (|| -> Result<(), String> {
            if platform::reparse(&sample.path) {
                return Err("Dosya reparse point oldu; okunmadı.".into());
            }
            let canonical = sample.path.canonicalize().map_err(|e| e.to_string())?;
            if !canonical.starts_with(&canonical_root) {
                return Err("Dosya bölüm dışına yönlendi.".into());
            }
            let identity = same_file::Handle::from_path(&sample.path).map_err(|e| e.to_string())?;
            let mut file = File::open(&sample.path).map_err(|e| e.to_string())?;
            platform::file_within(&file, root)?;
            if same_file::Handle::from_file(file.try_clone().map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?
                != identity
            {
                return Err("Dosya kimliği değişti.".into());
            }
            let mut buffer = vec![0u8; sample.bytes];
            file.seek(SeekFrom::Start(sample.offset))
                .map_err(|e| e.to_string())?;
            file.read_exact(&mut buffer).map_err(|e| e.to_string())?;
            Ok(())
        })();
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        phase_bi(
            core,
            &format!("Dosya örneği {}/{} · {rel}", index + 1, samples.len()),
            &format!("File sample {}/{} · {rel}", index + 1, samples.len()),
        );
        if let Err(error) = result {
            lock(&core.job)["damaged_files"]
                .as_array_mut()
                .unwrap()
                .push(json!({"path":rel,"block":index,"error":error}));
            add_block(
                core,
                json!({"index":index,"status":"bad","latency_ms":null,"mb_s":null,"offset_bytes":sample.offset,"bytes":0,"file":rel,"error":error}),
                0,
                0,
            );
        } else {
            add_block(
                core,
                json!({"index":index,"status":classify(ms),"latency_ms":ms,"mb_s":sample.bytes as f64/ms/1000.0,"offset_bytes":sample.offset,"bytes":sample.bytes,"file":rel,"error":null}),
                sample.bytes as u64,
                0,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn running_job_pins_file_and_directory_against_path_replacement() {
        let root = tempfile::tempdir().unwrap();
        let mut area = OwnedArea::create(root.path()).unwrap();
        let file = area.open().unwrap();
        drop(file);
        let rename = std::fs::rename(&area.path, area.path.with_extension("stolen"));
        assert!(
            rename.is_err(),
            "An active benchmark file must not be replaceable"
        );
        assert!(std::fs::rename(&area.directory, root.path().join("stolen-dir")).is_err());
        area.cleanup().unwrap();
    }
    #[test]
    fn cleanup_refuses_replaced_file_and_preserves_foreign_content() {
        let root = tempfile::tempdir().unwrap();
        let mut area = OwnedArea::create(root.path()).unwrap();
        let mut file = area.open().unwrap();
        file.write_all(b"owned").unwrap();
        drop(file);
        // Model a replacement at the handover to delete-capable handles.
        area.file_pin.take();
        std::fs::rename(&area.path, area.path.with_extension("old")).unwrap();
        std::fs::write(&area.path, b"foreign").unwrap();
        let path = area.path.clone();
        assert!(area.cleanup().is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"foreign");
    }
    #[test]
    #[cfg(windows)]
    fn native_direct_io_reads_writes_and_cancels_cleanly_on_temp_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("io-test.bin");
        let size_bytes = 4 * 1024 * 1024u64;
        {
            let mut f = File::create(&path).unwrap();
            let chunk = vec![0x5Au8; 1024 * 1024];
            for _ in 0..4 {
                f.write_all(&chunk).unwrap();
            }
            f.sync_all().unwrap();
        }
        let row = Row {
            id: "RND4KiB-Q4T1".into(),
            name: "RND4KiB Q4T1".into(),
            block_kib: 4,
            queue: 4,
            threads: 1,
            random: true,
        };
        let cancel = AtomicBool::new(false);
        let read_res =
            run_native_io_step(&path, size_bytes, &row, "read", 1, false, &cancel).unwrap();
        assert!(read_res["mb_s"].as_f64().unwrap() > 0.0);
        assert!(read_res["iops"].as_f64().unwrap() > 0.0);
        assert!(read_res["latency_us"].as_f64().unwrap() >= 0.0);
        assert_eq!(read_res["write_bytes"], "0");

        let mix_res = run_native_io_step(&path, size_bytes, &row, "mix", 1, true, &cancel).unwrap();
        assert!(
            mix_res["read_bytes"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
                > 0
        );
        assert!(
            mix_res["write_bytes"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
                > 0
        );

        let cancel_flag = std::sync::Arc::new(AtomicBool::new(false));
        let trigger = cancel_flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            trigger.store(true, Ordering::SeqCst);
        });
        let aborted = run_native_io_step(&path, size_bytes, &row, "read", 10, false, &cancel_flag);
        assert_eq!(aborted.unwrap_err(), process::CANCELLED);
        // Verify all handles were drained and closed so file can be deleted cleanly.
        std::fs::remove_file(&path).unwrap();
    }

    fn test_disk(
        kind: &str,
        status: &str,
        smart_rows: &[(&str, &str)],
        op_pct: Option<f64>,
    ) -> Disk {
        Disk {
            id: "test-disk-id".into(),
            stable_identity: true,
            device: "/dev/sda".into(),
            device_type: "ata".into(),
            model: "Test Disk".into(),
            serial: Some("SER123".into()),
            firmware: None,
            kind: kind.into(),
            protocol: Some(if kind == "NVMe SSD" { "NVMe" } else { "ATA" }.into()),
            metrics: json!({}),
            health: crate::model::Health {
                percent: Some(90),
                status: status.into(),
                method: None,
                explanation: String::new(),
            },
            smart: smart_rows
                .iter()
                .map(|(id, raw)| crate::model::SmartRow {
                    id: (*id).into(),
                    name: (*id).into(),
                    original: (*id).into(),
                    raw: (*raw).into(),
                    value: None,
                    worst: None,
                    threshold: None,
                    description: String::new(),
                    level: "ok".into(),
                })
                .collect(),
            alerts: Vec::new(),
            capabilities: json!({}),
            interface: None,
            logical_block_size: Some(512),
            physical_block_size: Some(4096),
            rotation_rate: (kind == "HDD").then_some(7200),
            nvme_eui64: Vec::new(),
            nvme_nguid: Vec::new(),
            wwn: None,
            source_time: None,
            os_disk_number: Some(0),
            os_serial: Some("SER123".into()),
            os_model: Some("Test Disk".into()),
            volumes: Vec::new(),
            bus_link: Value::Null,
            optimization: json!({ "over_provisioning_percent": op_pct }),
            read_incomplete: false,
            read_warning: None,
        }
    }

    #[test]
    fn surface_validate_options_accepts_optional_resume_boolean_only() {
        assert!(validate_options(
            "surface",
            &json!({"mode":"physical_scan","size_mib":"auto"})
        )
        .is_ok());
        assert!(validate_options(
            "surface",
            &json!({"mode":"physical_scan","size_mib":"auto","resume":true})
        )
        .is_ok());
        assert!(validate_options(
            "surface",
            &json!({"mode":"physical_scan","size_mib":256,"resume":false})
        )
        .is_ok());
        assert!(validate_options(
            "surface",
            &json!({"mode":"physical_scan","size_mib":"auto","resume":"yes"})
        )
        .is_err());
        assert!(validate_options(
            "surface",
            &json!({"mode":"physical_scan","size_mib":"auto","resume":1})
        )
        .is_err());
        assert!(validate_options(
            "surface",
            &json!({"mode":"physical_scan","size_mib":"auto","resume":null})
        )
        .is_err());
        assert!(validate_options(
            "surface",
            &json!({"mode":"physical_scan","size_mib":"auto","extra":true})
        )
        .is_err());
        assert!(validate_options("defrag", &json!({"mode":"analyze","resume":true})).is_err());
    }

    #[test]
    fn autopilot_hdd_defrag_guard_skips_unhealthy_and_allows_healthy_drives() {
        let healthy = test_disk(
            "HDD",
            "ok",
            &[("5", "49"), ("197", "0"), ("198", "0")],
            None,
        );
        assert!(autopilot_hdd_defrag_guard(&healthy).is_none());

        let crit = test_disk(
            "HDD",
            "critical",
            &[("5", "0"), ("197", "0"), ("198", "0")],
            None,
        );
        let msg = autopilot_hdd_defrag_guard(&crit).unwrap();
        assert!(msg.contains("defrag /O"));
        assert!(msg.contains("Tam Yüzey Taraması"));

        let pending = test_disk(
            "HDD",
            "warning",
            &[("5", "0"), ("197", "1"), ("198", "0")],
            None,
        );
        assert!(autopilot_hdd_defrag_guard(&pending)
            .unwrap()
            .contains("#197"));

        let uncorr = test_disk(
            "HDD",
            "warning",
            &[("5", "0"), ("197", "0"), ("198", "2")],
            None,
        );
        assert!(autopilot_hdd_defrag_guard(&uncorr)
            .unwrap()
            .contains("#198"));

        let reallocated = test_disk(
            "HDD",
            "warning",
            &[("5", "50"), ("197", "0"), ("198", "0")],
            None,
        );
        assert!(autopilot_hdd_defrag_guard(&reallocated)
            .unwrap()
            .contains("#5"));
    }

    #[test]
    fn autopilot_ssd_advisories_warn_for_large_hiberfil_and_low_over_provisioning() {
        let gib = 1024 * 1024 * 1024u64;
        let ssd_low_op = test_disk("NVMe SSD", "ok", &[], Some(2.4));
        let win_big_hiber = json!({
            "system_tuning": { "hiberfil_bytes": 6 * gib }
        });
        let notes = autopilot_ssd_advisories(&ssd_low_op, &win_big_hiber);
        assert_eq!(notes.len(), 2);
        assert!(notes[0].contains("hiberfil.sys"));
        assert!(notes[1].contains("Over-Provisioning"));

        let ssd_good = test_disk("NVMe SSD", "ok", &[], Some(7.5));
        let win_small_hiber = json!({
            "system_tuning": { "hiberfil_bytes": 4 * gib }
        });
        assert!(autopilot_ssd_advisories(&ssd_good, &win_small_hiber).is_empty());
        assert!(autopilot_ssd_advisories(&ssd_good, &json!({})).is_empty());
    }

    #[test]
    fn autopilot_advisories_and_guard_render_clean_english() {
        let has_tr = |s: &str| s.chars().any(|c| "çğıöşüÇĞİÖŞÜ".contains(c));
        let crit = test_disk(
            "HDD",
            "critical",
            &[("5", "0"), ("197", "3"), ("198", "2")],
            None,
        );
        let guard = autopilot_hdd_defrag_guard_for_lang(&crit, true).unwrap();
        assert!(!has_tr(&guard), "Turkish leaked into EN guard: {guard}");
        assert!(guard.contains("defrag /O"));
        let guard_tr = autopilot_hdd_defrag_guard_for_lang(&crit, false).unwrap();
        assert!(has_tr(&guard_tr));

        let gib = 1024 * 1024 * 1024u64;
        let ssd = test_disk("NVMe SSD", "ok", &[], Some(2.4));
        let win =
            json!({ "system_tuning": { "hiberfil_bytes": 6 * gib, "sysmain_running": true } });
        let notes = autopilot_ssd_advisories_for_lang(&ssd, &win, true);
        assert!(!notes.is_empty());
        for note in &notes {
            assert!(!has_tr(note), "Turkish leaked into EN advisory: {note}");
        }
        assert!(notes.iter().any(|n| n.contains("hiberfil.sys")));
    }

    #[test]
    fn smart_tier_validate_options_accepts_only_known_modes() {
        assert!(validate_options("smart_tier", &json!({"mode": "ssd_extent_optimize"})).is_ok());
        assert!(validate_options("smart_tier", &json!({"mode": "hdd_outer_optimize"})).is_ok());
        assert!(validate_options("smart_tier", &json!({"mode": "full_defrag"})).is_err());
        assert!(validate_options("smart_tier", &json!({})).is_err());
        assert!(validate_options(
            "smart_tier",
            &json!({"mode": "ssd_extent_optimize", "extra": 1})
        )
        .is_err());
        // Existing defrag validation remains intact.
        assert!(validate_options("defrag", &json!({"mode": "analyze"})).is_ok());
        assert!(validate_options("defrag", &json!({"mode": "defrag"})).is_ok());
        assert!(validate_options("defrag", &json!({"mode": "consolidate"})).is_ok());
    }
}
