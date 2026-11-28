//! High-performance Linux Direct-I/O benchmark engine (`io_uring` + `O_DIRECT`, with
//! automatic fallback to `pread`/`pwrite` + `O_DIRECT` and `POSIX_FADV_DONTNEED`).

use crate::{jobs::Row, process};
use io_uring::{opcode, types, IoUring};
use serde_json::{json, Value};
use std::{
    alloc::{alloc_zeroed, dealloc, Layout},
    fs::{File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    os::unix::io::AsRawFd,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

struct AlignedBuf {
    ptr: *mut u8,
    layout: Layout,
}

unsafe impl Send for AlignedBuf {}
unsafe impl Sync for AlignedBuf {}

impl AlignedBuf {
    fn new(size: usize, fill_random: bool, seed: u64) -> Result<Self, String> {
        let alloc_size = size.max(4096).div_ceil(4096) * 4096;
        let layout = Layout::from_size_align(alloc_size, 4096)
            .map_err(|e| format!("4K hizalı bellek düzeni hatası: {e}"))?;
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err("4K hizalı I/O arabelleği ayrılamadı.".into());
        }
        if fill_random {
            let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
            let slice = unsafe { std::slice::from_raw_parts_mut(ptr, alloc_size) };
            for chunk in slice.chunks_mut(8) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let b = state.to_le_bytes();
                let n = chunk.len();
                chunk.copy_from_slice(&b[..n]);
            }
        }
        Ok(Self { ptr, layout })
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }

    fn as_ptr(&self) -> *const u8 {
        self.ptr
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, self.layout) };
    }
}

#[derive(Default)]
struct WorkerStats {
    read_bytes: u128,
    write_bytes: u128,
    reads: u128,
    writes: u128,
    latency_ns_sum: u128,
}

pub fn io_uring_supported() -> bool {
    IoUring::new(16).is_ok()
}

fn open_bench_file(path: &Path, writable: bool) -> Result<(File, bool), String> {
    let direct_res = OpenOptions::new()
        .read(true)
        .write(writable)
        .custom_flags(libc::O_DIRECT)
        .open(path);
    match direct_res {
        Ok(f) => Ok((f, true)),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {
            let f = OpenOptions::new()
                .read(true)
                .write(writable)
                .open(path)
                .map_err(|err| format!("Test dosyası açılamadı: {err}"))?;
            unsafe {
                libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
            }
            Ok((f, false))
        }
        Err(e) => Err(format!("Doğrudan I/O dosyası açılamadı: {e}")),
    }
}

struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        let mut buf = [0u8; 8];
        let _ = getrandom::fill(&mut buf);
        let r = u64::from_le_bytes(buf) ^ seed ^ 0x517c_c1b7_2722_0a95;
        Self(if r == 0 { 0xdead_beef_cafe_babe } else { r })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn next_offset(
    rng: &mut XorShift64,
    seq_cursor: &mut u64,
    random: bool,
    block_bytes: u64,
    max_slots: u64,
) -> u64 {
    if random {
        (rng.next() % max_slots) * block_bytes
    } else {
        let off = *seq_cursor * block_bytes;
        *seq_cursor = (*seq_cursor + 1) % max_slots;
        off
    }
}

struct UringSlot {
    buf: AlignedBuf,
    in_flight: bool,
    is_write: bool,
    started: Instant,
}

#[allow(clippy::too_many_arguments)]
fn run_worker_uring(
    worker_id: usize,
    fd: std::os::unix::io::RawFd,
    direct: bool,
    file_size: u64,
    block_bytes: usize,
    queue: usize,
    random: bool,
    direction: &str,
    deadline: Instant,
    zero_data: bool,
    cancel: &AtomicBool,
) -> Result<WorkerStats, String> {
    let ring_depth = (queue.next_power_of_two()).clamp(8, 256) as u32;
    let mut ring = IoUring::new(ring_depth).map_err(|e| format!("io_uring başlatılamadı: {e}"))?;

    let b64 = block_bytes as u64;
    let max_slots = (file_size / b64).max(1);
    let mut rng = XorShift64::new((worker_id as u64) + 1);
    let mut seq_cursor = (worker_id as u64).wrapping_mul(max_slots / 16) % max_slots;

    let mut slots = Vec::with_capacity(queue);
    for i in 0..queue {
        slots.push(UringSlot {
            buf: AlignedBuf::new(block_bytes, !zero_data, (worker_id * 1024 + i) as u64 + 1)?,
            in_flight: false,
            is_write: false,
            started: Instant::now(),
        });
    }

    let mut stats = WorkerStats::default();
    let mut in_flight_count = 0usize;
    let mut op_seq = 0u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            // Drain any pending CQEs before returning so buffers outlive kernel references.
            while in_flight_count > 0 {
                let _ = ring.submit_and_wait(1);
                for cqe in ring.completion() {
                    let idx = cqe.user_data() as usize;
                    if idx < slots.len() && slots[idx].in_flight {
                        slots[idx].in_flight = false;
                        in_flight_count -= 1;
                    }
                }
            }
            return Err(process::CANCELLED.into());
        }

        let timed_out = Instant::now() >= deadline;
        if !timed_out {
            let mut sq = ring.submission();
            for (idx, slot) in slots.iter_mut().enumerate() {
                if slot.in_flight {
                    continue;
                }
                let is_write = match direction {
                    "write" => true,
                    "mix" => op_seq.is_multiple_of(3),
                    _ => false,
                };
                op_seq += 1;
                let offset = next_offset(&mut rng, &mut seq_cursor, random, b64, max_slots);
                let sqe = if is_write {
                    opcode::Write::new(types::Fd(fd), slot.buf.as_ptr(), block_bytes as u32)
                        .offset(offset)
                        .build()
                } else {
                    opcode::Read::new(types::Fd(fd), slot.buf.as_mut_ptr(), block_bytes as u32)
                        .offset(offset)
                        .build()
                }
                .user_data(idx as u64);

                unsafe {
                    if sq.push(&sqe).is_err() {
                        break;
                    }
                }
                slot.in_flight = true;
                slot.is_write = is_write;
                slot.started = Instant::now();
                in_flight_count += 1;
            }
        }

        if in_flight_count == 0 {
            break;
        }

        if let Err(e) = ring.submit_and_wait(1) {
            if e.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            while in_flight_count > 0 {
                let _ = ring.submit_and_wait(1);
                for cqe in ring.completion() {
                    let idx = cqe.user_data() as usize;
                    if idx < slots.len() && slots[idx].in_flight {
                        slots[idx].in_flight = false;
                        in_flight_count -= 1;
                    }
                }
            }
            return Err(format!("io_uring submit_and_wait hatası: {e}"));
        }

        let mut io_err: Option<i32> = None;
        for cqe in ring.completion() {
            let idx = cqe.user_data() as usize;
            if idx >= slots.len() || !slots[idx].in_flight {
                continue;
            }
            let slot = &mut slots[idx];
            slot.in_flight = false;
            in_flight_count -= 1;
            let res = cqe.result();
            if res < 0 {
                io_err = Some(-res);
                break;
            }
            let n = res as u128;
            let lat_ns = slot.started.elapsed().as_nanos();
            stats.latency_ns_sum += lat_ns;
            if slot.is_write {
                stats.writes += 1;
                stats.write_bytes += n;
            } else {
                stats.reads += 1;
                stats.read_bytes += n;
            }
        }
        if let Some(os_err) = io_err {
            // Drain remaining in-flight operations before dropping buffers
            while in_flight_count > 0 {
                let _ = ring.submit_and_wait(1);
                for rem in ring.completion() {
                    let ri = rem.user_data() as usize;
                    if ri < slots.len() && slots[ri].in_flight {
                        slots[ri].in_flight = false;
                        in_flight_count -= 1;
                    }
                }
            }
            return Err(format!(
                "io_uring I/O hatası: {}",
                std::io::Error::from_raw_os_error(os_err)
            ));
        }
    }

    if !direct && direction != "read" {
        unsafe {
            libc::fdatasync(fd);
            libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED);
        }
    }
    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
fn run_worker_posix(
    worker_id: usize,
    fd: std::os::unix::io::RawFd,
    direct: bool,
    file_size: u64,
    block_bytes: usize,
    random: bool,
    direction: &str,
    deadline: Instant,
    zero_data: bool,
    cancel: &AtomicBool,
) -> Result<WorkerStats, String> {
    let b64 = block_bytes as u64;
    let max_slots = (file_size / b64).max(1);
    let mut rng = XorShift64::new((worker_id as u64) + 7);
    let mut seq_cursor = (worker_id as u64).wrapping_mul(max_slots / 16) % max_slots;
    let mut buf = AlignedBuf::new(block_bytes, !zero_data, (worker_id as u64) + 13)?;
    let mut stats = WorkerStats::default();
    let mut op_seq = 0u64;

    while Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return Err(process::CANCELLED.into());
        }
        let is_write = match direction {
            "write" => true,
            "mix" => op_seq.is_multiple_of(3),
            _ => false,
        };
        op_seq += 1;
        let offset = next_offset(&mut rng, &mut seq_cursor, random, b64, max_slots);
        let t0 = Instant::now();
        let rc = if is_write {
            unsafe {
                libc::pwrite(
                    fd,
                    buf.as_ptr().cast::<libc::c_void>(),
                    block_bytes,
                    offset as libc::off_t,
                )
            }
        } else {
            unsafe {
                libc::pread(
                    fd,
                    buf.as_mut_ptr().cast::<libc::c_void>(),
                    block_bytes,
                    offset as libc::off_t,
                )
            }
        };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(format!("Doğrudan I/O hatası: {err}"));
        }
        let lat_ns = t0.elapsed().as_nanos();
        stats.latency_ns_sum += lat_ns;
        if is_write {
            stats.writes += 1;
            stats.write_bytes += rc as u128;
        } else {
            stats.reads += 1;
            stats.read_bytes += rc as u128;
        }
        if !direct && op_seq.is_multiple_of(64) {
            if is_write {
                unsafe { libc::fdatasync(fd) };
            }
            unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED) };
        }
    }

    if !direct && direction != "read" {
        unsafe {
            libc::fdatasync(fd);
            libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED);
        }
    }
    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    worker_id: usize,
    path: &Path,
    file_size: u64,
    block_bytes: usize,
    queue: usize,
    random: bool,
    direction: &str,
    deadline: Instant,
    zero_data: bool,
    cancel: &AtomicBool,
) -> Result<WorkerStats, String> {
    let writable = direction != "read";
    let (file, direct) = open_bench_file(path, writable)?;
    let fd = file.as_raw_fd();

    if io_uring_supported() {
        match run_worker_uring(
            worker_id,
            fd,
            direct,
            file_size,
            block_bytes,
            queue,
            random,
            direction,
            deadline,
            zero_data,
            cancel,
        ) {
            Ok(stats) => return Ok(stats),
            Err(e) if e == process::CANCELLED => return Err(e),
            Err(_) => {
                // Fall back to POSIX pread/pwrite when seccomp/kernel restricts io_uring ops
            }
        }
    }

    run_worker_posix(
        worker_id,
        fd,
        direct,
        file_size,
        block_bytes,
        random,
        direction,
        deadline,
        zero_data,
        cancel,
    )
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
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds.max(1));

    let all_stats = std::thread::scope(|scope| -> Result<Vec<WorkerStats>, String> {
        let mut handles = Vec::with_capacity(workers);
        for id in 0..workers {
            handles.push(scope.spawn(move || {
                run_worker(
                    id,
                    path,
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
