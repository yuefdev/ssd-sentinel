//! Privileged Polkit helper (`--privileged-helper`) and elevation client for Linux.
//!
//! Security model:
//! - The GUI never runs as `root` (keeping Wayland/X11/XDG Portals safe).
//! - When privileged hardware/storage actions are requested by a non-root user without direct
//!   `/dev/*` permissions, `authenticate()` spawns `pkexec <self> --privileged-helper` once per
//!   session and exchanges strictly typed, allowlisted `HelperOp` JSON-RPC commands over stdin/stdout.
//! - Zero shell interpolation: every binary is resolved from trusted system prefixes and invoked
//!   strictly with fixed `Command::arg` vectors.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
    os::unix::fs::{FileTypeExt, OpenOptionsExt},
    os::unix::io::AsRawFd,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Mutex,
    time::Instant,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HelperOp {
    Ping,
    SmartScan {
        open: bool,
    },
    SmartQuery {
        device: String,
        device_type: Option<String>,
        mode: String, // "info" | "health" | "all"
    },
    SmartSelfTest {
        device: String,
        device_type: Option<String>,
        mode: String, // "short" | "long" | "abort"
    },
    SmartApm {
        device: String,
        device_type: Option<String>,
        level: u8,
    },
    FsTrim {
        mount_point: String,
    },
    WriteSysfs {
        path: String,
        value: String,
    },
    HdparmWriteCache {
        device: String,
        enable: bool,
    },
    HdparmApm {
        device: String,
        level: u8,
    },
    SystemctlFstrimTimer {
        enable: bool,
    },
    RawDeviceSize {
        device: String,
    },
    RawDeviceRead {
        device: String,
        offset: u64,
        len: usize,
        direct: bool,
        include_data: bool,
    },
    FsCheckReadOnly {
        device: String,
        fstype: String,
    },
    FsDefrag {
        path: String,
        fstype: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct HelperRequest {
    id: u64,
    #[serde(flatten)]
    op: HelperOp,
}

#[derive(Debug, Serialize, Deserialize)]
struct HelperResponse {
    id: u64,
    ok: bool,
    #[serde(default)]
    data: Value,
    #[serde(default)]
    error: Option<String>,
}

struct HelperSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Drop for HelperSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static HELPER: Mutex<Option<HelperSession>> = Mutex::new(None);

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn is_helper_active() -> bool {
    let mut guard = HELPER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(sess) = guard.as_mut() {
        match sess.child.try_wait() {
            Ok(None) => return true,
            _ => {
                *guard = None;
            }
        }
    }
    false
}

pub fn is_elevated() -> bool {
    is_root() || is_helper_active()
}

pub fn has_direct_device_access(device: &str) -> bool {
    if is_root() {
        return true;
    }
    if !is_valid_block_dev_path(device) {
        return false;
    }
    OpenOptions::new().read(true).open(device).is_ok()
}

pub fn shutdown_helper() {
    let mut guard = HELPER.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

pub fn find_trusted_binary(names: &[&str]) -> Option<PathBuf> {
    let prefixes = [
        "/usr/sbin",
        "/sbin",
        "/usr/bin",
        "/bin",
        "/usr/local/sbin",
        "/usr/local/bin",
        "/run/current-system/sw/bin",
    ];
    for name in names {
        for dir in prefixes {
            let candidate = Path::new(dir).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

pub fn authenticate() -> Result<(), String> {
    if is_root() {
        return Ok(());
    }
    if is_helper_active() {
        return call(HelperOp::Ping).map(|_| ());
    }
    let exe = std::env::current_exe().map_err(|e| format!("Yürütülebilir yol bulunamadı: {e}"))?;
    let exe_str = exe
        .to_str()
        .ok_or_else(|| "Geçersiz UTF-8 uygulama yolu.".to_string())?;
    let pkexec = find_trusted_binary(&["pkexec"]).ok_or_else(|| {
        "Polkit (pkexec) bulunamadı. Yetki yükseltme için pkexec veya sudo kullanın.".to_string()
    })?;

    let mut child = Command::new(pkexec)
        .arg(exe_str)
        .arg("--privileged-helper")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("pkexec başlatılamadı: {e}"))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Yardımcı girdi akışı açılamadı.".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Yardımcı çıktı akışı açılamadı.".to_string())?;

    let mut sess = HelperSession {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 1,
    };

    match send_on_session(&mut sess, HelperOp::Ping) {
        Ok(_) => {
            let mut guard = HELPER.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(sess);
            Ok(())
        }
        Err(e) => {
            use std::io::Read as _;
            let _ = sess.child.kill();
            let mut stderr_str = String::new();
            if let Some(mut se) = sess.child.stderr.take() {
                let _ = se.read_to_string(&mut stderr_str);
            }
            let _ = sess.child.wait();
            let stderr = stderr_str.trim();
            if stderr.is_empty() {
                Err(format!(
                    "Polkit yetkilendirmesi reddedildi veya başarısız oldu: {e}"
                ))
            } else {
                Err(format!("Polkit yetkilendirmesi başarısız: {stderr}"))
            }
        }
    }
}

pub fn call(op: HelperOp) -> Result<Value, String> {
    if is_root() {
        return execute_op_local(op);
    }
    let mut guard = HELPER.lock().unwrap_or_else(|e| e.into_inner());
    let sess = guard.as_mut().ok_or_else(|| {
        "Yönetici yetki oturumu aktif değil; önce Polkit ile yetki verin.".to_string()
    })?;
    match sess.child.try_wait() {
        Ok(None) => {}
        _ => {
            *guard = None;
            return Err("Yetkili yardımcı süreç sonlandı; yeniden doğrulayın.".into());
        }
    }
    send_on_session(sess, op)
}

fn send_on_session(sess: &mut HelperSession, op: HelperOp) -> Result<Value, String> {
    let id = sess.next_id;
    sess.next_id = sess.next_id.wrapping_add(1);
    let req = HelperRequest { id, op };
    let line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    writeln!(sess.stdin, "{line}").map_err(|e| format!("Yardımcıya yazılamadı: {e}"))?;
    sess.stdin
        .flush()
        .map_err(|e| format!("Yardımcı akışı gönderilemedi: {e}"))?;

    let mut resp_line = String::new();
    let n = sess
        .stdout
        .read_line(&mut resp_line)
        .map_err(|e| format!("Yardımcı yanıtı okunamadı: {e}"))?;
    if n == 0 {
        return Err("Yetkili yardımcı bağlantısı kapandı.".into());
    }
    let resp: HelperResponse = serde_json::from_str(resp_line.trim())
        .map_err(|e| format!("Yardımcı JSON yanıtı bozuk: {e}"))?;
    if resp.id != id {
        return Err("Yardımcı istek/yanıt kimliği uyuşmuyor.".into());
    }
    if resp.ok {
        Ok(resp.data)
    } else {
        Err(resp
            .error
            .unwrap_or_else(|| "Yardımcı işlem hatası.".into()))
    }
}

/// Validates a Linux block device node path (`/dev/nvme0n1`, `/dev/sda`, etc.).
pub fn is_valid_block_dev_path(dev: &str) -> bool {
    if !dev.starts_with("/dev/") || dev.len() > 128 || dev.contains('\0') {
        return false;
    }
    let rest = &dev["/dev/".len()..];
    if rest.is_empty() || rest.contains("..") {
        return false;
    }
    rest.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'/' | b'.'))
}

/// Strictly validates allowlisted writable sysfs/procfs paths.
pub fn validate_writable_sysfs_path(path: &str) -> Result<PathBuf, String> {
    if path.contains("..") || path.contains('\0') || path.len() > 160 {
        return Err("Geçersiz sysfs yolu.".into());
    }
    if path == "/sys/module/pcie_aspm/parameters/policy" || path == "/proc/sys/vm/swappiness" {
        return Ok(PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("/sys/block/") {
        if let Some((dev, tail)) = rest.split_once('/') {
            if !dev.is_empty()
                && dev.len() <= 64
                && dev
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                && matches!(tail, "queue/write_cache" | "queue/scheduler")
            {
                return Ok(PathBuf::from(path));
            }
        }
    }
    Err(format!("İzin verilen sysfs listesi dışında yol: {path}"))
}

fn validate_safe_sysfs_value(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 48
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b' '))
    {
        return Err("Geçersiz sysfs değeri.".into());
    }
    Ok(trimmed.to_owned())
}

fn validate_device_type(dt: &Option<String>) -> Result<(), String> {
    if let Some(t) = dt {
        if t.is_empty()
            || t.starts_with('-')
            || t.len() > 64
            || !t
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b',' | b'.' | b'_' | b'+' | b'-'))
        {
            return Err("Geçersiz smartctl aygıt türü.".into());
        }
    }
    Ok(())
}

fn validate_local_mount_or_path(p: &str) -> Result<PathBuf, String> {
    if !p.starts_with('/') || p.contains('\0') || p.split('/').any(|c| c == "..") {
        return Err("Geçersiz yerel Linux yolu.".into());
    }
    let pb = PathBuf::from(p);
    if !pb.exists() {
        return Err(format!("Yol bulunamadı: {p}"));
    }
    Ok(pb)
}

fn resolve_smartctl_trusted() -> Result<PathBuf, String> {
    let candidates = [
        "/usr/sbin/smartctl",
        "/sbin/smartctl",
        "/usr/bin/smartctl",
        "/bin/smartctl",
        "/usr/local/sbin/smartctl",
        "/usr/local/bin/smartctl",
        "/run/current-system/sw/bin/smartctl",
    ];
    for c in candidates {
        let p = Path::new(c);
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
    }
    Err("smartctl sistem dizinlerinde bulunamadı (smartmontools kurulu mu?).".into())
}

fn run_smartctl_json(args: &[String]) -> Result<Value, String> {
    let exe = resolve_smartctl_trusted()?;
    let mut cmd_args = vec!["-j".to_string()];
    cmd_args.extend_from_slice(args);
    let output = Command::new(&exe)
        .args(&cmd_args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("smartctl çalıştırılamadı: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut val: Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        format!(
            "smartctl JSON çıktısı geçersiz (kod {:?}).",
            output.status.code()
        )
    })?;
    if !val.is_object() {
        return Err("SMART çıktısı JSON nesnesi değil.".into());
    }
    if !val["smartctl"].is_object() {
        val["smartctl"] = json!({});
    }
    val["smartctl"]["exit_status"] = json!(output.status.code().unwrap_or(-1) as u32);
    Ok(val)
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err("Hex veri uzunluğu çift olmalı.".into());
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let (chunks, _) = bytes.as_chunks::<2>();
    for pair in chunks {
        let hi = (pair[0] as char)
            .to_digit(16)
            .ok_or_else(|| "Geçersiz hex karakteri.".to_string())?;
        let lo = (pair[1] as char)
            .to_digit(16)
            .ok_or_else(|| "Geçersiz hex karakteri.".to_string())?;
        out.push(((hi << 4) | lo) as u8);
    }
    Ok(out)
}

#[repr(C)]
struct FitrimRange {
    start: u64,
    len: u64,
    minlen: u64,
}

#[cfg(target_pointer_width = "64")]
const BLKGETSIZE64: libc::c_ulong = 0x8008_1272;
#[cfg(target_pointer_width = "32")]
const BLKGETSIZE64: libc::c_ulong = 0x8004_1272;
// FITRIM = _IOWR('X', 121, struct fstrim_range) = 0xC0185879
const FITRIM_IOCTL: libc::c_ulong = 0xC018_5879;

pub fn query_block_device_size(device: &str) -> Result<u64, String> {
    if !is_valid_block_dev_path(device) {
        return Err("Geçersiz blok aygıt yolu.".into());
    }
    let meta = std::fs::metadata(device).map_err(|e| format!("{device} incelenemedi: {e}"))?;
    if !meta.file_type().is_block_device() {
        return Err(format!("{device} bir blok aygıtı değil."));
    }
    let file = OpenOptions::new()
        .read(true)
        .open(device)
        .map_err(|e| format!("{device} açılamadı: {e}"))?;
    let mut bytes: u64 = 0;
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), BLKGETSIZE64 as _, &mut bytes) };
    if rc != 0 {
        return Err(format!(
            "BLKGETSIZE64 başarısız ({device}): {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(bytes)
}

pub fn read_raw_device_chunk(
    device: &str,
    offset: u64,
    len: usize,
    direct: bool,
    include_data: bool,
) -> Result<Value, String> {
    if !is_valid_block_dev_path(device) {
        return Err("Geçersiz blok aygıt yolu.".into());
    }
    let clamped_len = len.clamp(512, 8 * 1024 * 1024);
    let aligned_len = clamped_len.div_ceil(4096) * 4096;

    let open_res = if direct {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECT)
            .open(device)
            .or_else(|_| OpenOptions::new().read(true).open(device))
    } else {
        OpenOptions::new().read(true).open(device)
    };
    let file = open_res.map_err(|e| format!("{device} açılamadı: {e}"))?;
    let fd = file.as_raw_fd();

    // Allocate 4K-aligned buffer so O_DIRECT works cleanly across all block drivers.
    let layout = std::alloc::Layout::from_size_align(aligned_len, 4096)
        .map_err(|e| format!("Bellek hizalaması başarısız: {e}"))?;
    let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        return Err("4K hizalı okuma arabelleği ayrılamadı.".into());
    }
    let buf = unsafe { std::slice::from_raw_parts_mut(ptr, aligned_len) };

    let started = Instant::now();
    let read_res = unsafe {
        libc::pread(
            fd,
            buf.as_mut_ptr().cast::<libc::c_void>(),
            clamped_len,
            offset as libc::off_t,
        )
    };
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    let out = if read_res < 0 {
        let err = std::io::Error::last_os_error();
        Err(format!("Ham okuma hatası (LBA offset {offset}): {err}"))
    } else {
        let n = (read_res as usize).min(clamped_len);
        let hex = if include_data {
            Value::String(encode_hex(&buf[..n]))
        } else {
            Value::Null
        };
        Ok(json!({
            "bytes_read": n,
            "latency_ms": elapsed_ms,
            "data_hex": hex
        }))
    };

    unsafe {
        std::alloc::dealloc(ptr, layout);
    }
    out
}

pub fn trim_mount_point_local(mount_point: &str) -> Result<Value, String> {
    let mp = validate_local_mount_or_path(mount_point)?;
    if let Ok(dir) = OpenOptions::new().read(true).open(&mp) {
        let mut range = FitrimRange {
            start: 0,
            len: u64::MAX,
            minlen: 0,
        };
        let rc = unsafe { libc::ioctl(dir.as_raw_fd(), FITRIM_IOCTL as _, &mut range) };
        if rc == 0 {
            return Ok(json!({
                "status": "ok",
                "method": "FITRIM ioctl",
                "trimmed_bytes": range.len,
                "mount_point": mount_point
            }));
        }
    }
    let fstrim = find_trusted_binary(&["fstrim"])
        .ok_or_else(|| "FITRIM izni yok ve fstrim komutu bulunamadı.".to_string())?;
    let out = Command::new(fstrim)
        .arg("-v")
        .arg(mount_point)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("fstrim çalıştırılamadı: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    if !out.status.success() {
        return Err(if stderr.is_empty() { stdout } else { stderr });
    }
    Ok(json!({
        "status": "ok",
        "method": "fstrim",
        "output": stdout,
        "mount_point": mount_point
    }))
}

/// Executes an allowlisted privileged operation locally (when root or inside helper daemon).
pub fn execute_op_local(op: HelperOp) -> Result<Value, String> {
    match op {
        HelperOp::Ping => Ok(json!({"pong": true, "euid": unsafe { libc::geteuid() }})),
        HelperOp::SmartScan { open } => {
            let flag = if open { "--scan-open" } else { "--scan" };
            run_smartctl_json(&[flag.to_string()])
        }
        HelperOp::SmartQuery {
            device,
            device_type,
            mode,
        } => {
            if !is_valid_block_dev_path(&device) {
                return Err("Geçersiz aygıt yolu.".into());
            }
            validate_device_type(&device_type)?;
            let mut args: Vec<String> = match mode.as_str() {
                "info" => vec!["-i".into()],
                "all" => vec!["-a".into()],
                _ => vec!["-i".into(), "-H".into(), "-A".into()],
            };
            if let Some(dt) = device_type.filter(|s| s != "auto") {
                args.push("-d".into());
                args.push(dt);
            }
            args.push(device);
            run_smartctl_json(&args)
        }
        HelperOp::SmartSelfTest {
            device,
            device_type,
            mode,
        } => {
            if !is_valid_block_dev_path(&device) {
                return Err("Geçersiz aygıt yolu.".into());
            }
            validate_device_type(&device_type)?;
            let mut args: Vec<String> = match mode.as_str() {
                "short" => vec!["-t".into(), "short".into()],
                "long" => vec!["-t".into(), "long".into()],
                "abort" => vec!["-X".into()],
                _ => return Err("Geçersiz self-test modu.".into()),
            };
            if let Some(dt) = device_type.filter(|s| s != "auto") {
                args.push("-d".into());
                args.push(dt);
            }
            args.push(device);
            run_smartctl_json(&args)
        }
        HelperOp::SmartApm {
            device,
            device_type,
            level,
        } => {
            if !is_valid_block_dev_path(&device) {
                return Err("Geçersiz aygıt yolu.".into());
            }
            validate_device_type(&device_type)?;
            let mut args = vec!["-s".into(), format!("apm,{level}")];
            if let Some(dt) = device_type.filter(|s| s != "auto") {
                args.push("-d".into());
                args.push(dt);
            }
            args.push(device);
            run_smartctl_json(&args)
        }
        HelperOp::FsTrim { mount_point } => trim_mount_point_local(&mount_point),
        HelperOp::WriteSysfs { path, value } => {
            let valid_path = validate_writable_sysfs_path(&path)?;
            let safe_val = validate_safe_sysfs_value(&value)?;
            std::fs::write(&valid_path, format!("{safe_val}\n"))
                .map_err(|e| format!("{} yazılamadı: {e}", valid_path.display()))?;
            let readback = std::fs::read_to_string(&valid_path)
                .unwrap_or_default()
                .trim()
                .to_owned();
            Ok(json!({ "path": path, "value": readback }))
        }
        HelperOp::HdparmWriteCache { device, enable } => {
            if !is_valid_block_dev_path(&device) {
                return Err("Geçersiz blok aygıtı.".into());
            }
            let hdparm = find_trusted_binary(&["hdparm"])
                .ok_or_else(|| "hdparm sistemde bulunamadı.".to_string())?;
            let flag = if enable { "-W1" } else { "-W0" };
            let out = Command::new(hdparm)
                .arg(flag)
                .arg(&device)
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("hdparm çalıştırılamadı: {e}"))?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
            }
            Ok(json!({ "device": device, "write_cache": enable }))
        }
        HelperOp::HdparmApm { device, level } => {
            if !is_valid_block_dev_path(&device) {
                return Err("Geçersiz blok aygıtı.".into());
            }
            let hdparm = find_trusted_binary(&["hdparm"])
                .ok_or_else(|| "hdparm sistemde bulunamadı.".to_string())?;
            let out = Command::new(hdparm)
                .arg("-B")
                .arg(level.to_string())
                .arg(&device)
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("hdparm çalıştırılamadı: {e}"))?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
            }
            Ok(json!({ "device": device, "apm": level }))
        }
        HelperOp::SystemctlFstrimTimer { enable } => {
            let systemctl = find_trusted_binary(&["systemctl"]).ok_or_else(|| {
                "systemctl bulunamadı (OpenRC/runit dağıtımlarında cron/fstrim kullanın)."
                    .to_string()
            })?;
            let action = if enable { "enable" } else { "disable" };
            let out = Command::new(systemctl)
                .args([action, "--now", "fstrim.timer"])
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("systemctl çalıştırılamadı: {e}"))?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
            }
            Ok(json!({ "fstrim_timer": enable }))
        }
        HelperOp::RawDeviceSize { device } => {
            let bytes = query_block_device_size(&device)?;
            Ok(json!({ "device": device, "bytes": bytes }))
        }
        HelperOp::RawDeviceRead {
            device,
            offset,
            len,
            direct,
            include_data,
        } => read_raw_device_chunk(&device, offset, len, direct, include_data),
        HelperOp::FsCheckReadOnly { device, fstype } => {
            if !is_valid_block_dev_path(&device) {
                return Err("Geçersiz blok aygıtı.".into());
            }
            let fs = fstype.to_ascii_lowercase();
            let (bin, args): (&str, Vec<&str>) = match fs.as_str() {
                "ext4" | "ext3" | "ext2" => ("e2fsck", vec!["-n", "-f", &device]),
                "xfs" => ("xfs_repair", vec!["-n", &device]),
                "btrfs" => ("btrfs", vec!["check", "--readonly", &device]),
                "vfat" | "fat32" => ("fsck.fat", vec!["-n", &device]),
                _ => {
                    return Ok(json!({
                        "status": "skipped",
                        "message": format!("{fstype} dosya sistemi için salt-okunur çevrimiçi kontrol atlandı.")
                    }));
                }
            };
            let exe = find_trusted_binary(&[bin])
                .ok_or_else(|| format!("{bin} aracı sistemde kurulu değil."))?;
            let out = Command::new(exe)
                .args(&args)
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("{bin} çalıştırılamadı: {e}"))?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            Ok(json!({
                "status": if out.status.success() { "ok" } else { "warning" },
                "exit_code": out.status.code().unwrap_or(-1),
                "output": format!("{stdout}\n{stderr}").trim().chars().take(1800).collect::<String>()
            }))
        }
        HelperOp::FsDefrag { path, fstype } => {
            let _ = validate_local_mount_or_path(&path)?;
            let fs = fstype.to_ascii_lowercase();
            let (bin, args): (&str, Vec<&str>) = match fs.as_str() {
                "ext4" => ("e4defrag", vec![&path]),
                "btrfs" => ("btrfs", vec!["filesystem", "defragment", "-r", &path]),
                "xfs" => ("xfs_fsr", vec![&path]),
                _ => {
                    return Err(format!(
                        "{fstype} dosya sistemi Linux birleştirme desteklemiyor."
                    ));
                }
            };
            let exe =
                find_trusted_binary(&[bin]).ok_or_else(|| format!("{bin} sistemde bulunamadı."))?;
            let out = Command::new(exe)
                .args(&args)
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("{bin} çalıştırılamadı: {e}"))?;
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            if !out.status.success() {
                return Err(if stderr.is_empty() { stdout } else { stderr });
            }
            Ok(json!({ "status": "ok", "output": stdout }))
        }
    }
}

/// Main loop for `ssd-sentinel --privileged-helper` invoked via `pkexec`.
pub fn run_helper_stdio() -> i32 {
    if !is_root() {
        eprintln!("ssd-sentinel --privileged-helper requires root privileges.");
        return 1;
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut reader = BufReader::new(stdin.lock());
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let resp = match serde_json::from_str::<HelperRequest>(trimmed) {
                    Ok(req) => match execute_op_local(req.op) {
                        Ok(data) => HelperResponse {
                            id: req.id,
                            ok: true,
                            data,
                            error: None,
                        },
                        Err(err) => HelperResponse {
                            id: req.id,
                            ok: false,
                            data: Value::Null,
                            error: Some(err),
                        },
                    },
                    Err(e) => HelperResponse {
                        id: 0,
                        ok: false,
                        data: Value::Null,
                        error: Some(format!("Geçersiz yardımcı isteği: {e}")),
                    },
                };
                if let Ok(json_line) = serde_json::to_string(&resp) {
                    let _ = writeln!(out, "{json_line}");
                    let _ = out.flush();
                }
            }
            Err(_) => break,
        }
    }
    0
}
