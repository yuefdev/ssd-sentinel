//! Pure-Rust Linux hardware inventory, `/proc/diskstats` performance telemetry,
//! `/proc/self/mountinfo` volume mapping, PCIe link detection, and FITRIM helpers.
//!
//! Zero subprocess forks for inventory & performance monitoring: reads directly from
//! `/sys/block`, `/sys/class/nvme`, `/proc/self/mountinfo`, `/proc/diskstats`, and `statvfs`.

use crate::linux_priv::{self, HelperOp};
use crate::process;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::CString,
    fs,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug)]
pub struct MountEntry {
    pub major_minor: String,
    pub fs_root: String,
    pub mount_point: String,
    pub fstype: String,
    pub source: String,
    pub options: String,
}

#[derive(Clone, Debug)]
pub struct DiskStatRow {
    pub major: u64,
    pub minor: u64,
    pub name: String,
    pub reads_completed: u64,
    pub sectors_read: u64,
    pub ms_reading: u64,
    pub writes_completed: u64,
    pub sectors_written: u64,
    pub ms_writing: u64,
    pub ios_in_progress: u64,
    pub ms_doing_io: u64,
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse::<u64>().ok()
}

pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    if rc != 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64)
}

pub fn statvfs_bytes(path: &Path) -> Option<(u64, u64)> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut vfs) };
    if rc != 0 {
        return None;
    }
    let block = if vfs.f_frsize > 0 {
        vfs.f_frsize as u64
    } else {
        vfs.f_bsize as u64
    };
    let total = (vfs.f_blocks as u64).saturating_mul(block);
    let free = (vfs.f_bavail as u64).saturating_mul(block);
    Some((total, free))
}

fn unescape_mount_field(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Ok(oct) = std::str::from_utf8(&bytes[i + 1..i + 4]) {
                if let Ok(val) = u8::from_str_radix(oct, 8) {
                    out.push(val as char);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

pub fn parse_mountinfo_str(content: &str) -> Vec<MountEntry> {
    let mut out = Vec::new();
    for line in content.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let lparts: Vec<&str> = left.split(' ').collect();
        let rparts: Vec<&str> = right.split(' ').collect();
        if lparts.len() < 5 || rparts.len() < 3 {
            continue;
        }
        out.push(MountEntry {
            major_minor: lparts[2].to_owned(),
            fs_root: unescape_mount_field(lparts[3]),
            mount_point: unescape_mount_field(lparts[4]),
            fstype: rparts[0].to_owned(),
            source: unescape_mount_field(rparts[1]),
            options: rparts[2].to_owned(),
        });
    }
    out
}

pub fn mounted_volumes() -> Vec<MountEntry> {
    let Ok(raw) = fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    parse_mountinfo_str(&raw)
        .into_iter()
        .filter(|e| {
            e.mount_point.starts_with('/')
                && !matches!(
                    e.fstype.as_str(),
                    "proc"
                        | "sysfs"
                        | "devtmpfs"
                        | "tmpfs"
                        | "overlay"
                        | "devpts"
                        | "cgroup"
                        | "cgroup2"
                        | "pstore"
                        | "bpf"
                        | "tracefs"
                        | "securityfs"
                        | "debugfs"
                        | "hugetlbfs"
                        | "mqueue"
                        | "configfs"
                        | "fusectl"
                        | "autofs"
                        | "efivarfs"
                        | "binfmt_misc"
                )
        })
        .collect()
}

pub fn parse_diskstats_str(content: &str) -> Vec<DiskStatRow> {
    let mut out = Vec::new();
    for line in content.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 14 {
            continue;
        }
        let Ok(major) = fields[0].parse::<u64>() else {
            continue;
        };
        let Ok(minor) = fields[1].parse::<u64>() else {
            continue;
        };
        let Ok(reads_completed) = fields[3].parse::<u64>() else {
            continue;
        };
        let Ok(sectors_read) = fields[5].parse::<u64>() else {
            continue;
        };
        let Ok(ms_reading) = fields[6].parse::<u64>() else {
            continue;
        };
        let Ok(writes_completed) = fields[7].parse::<u64>() else {
            continue;
        };
        let Ok(sectors_written) = fields[9].parse::<u64>() else {
            continue;
        };
        let Ok(ms_writing) = fields[10].parse::<u64>() else {
            continue;
        };
        let Ok(ios_in_progress) = fields[11].parse::<u64>() else {
            continue;
        };
        let Ok(ms_doing_io) = fields[12].parse::<u64>() else {
            continue;
        };
        out.push(DiskStatRow {
            major,
            minor,
            name: fields[2].to_owned(),
            reads_completed,
            sectors_read,
            ms_reading,
            writes_completed,
            sectors_written,
            ms_writing,
            ios_in_progress,
            ms_doing_io,
        });
    }
    out
}

pub fn parse_pcie_gen(raw: &str) -> Option<u64> {
    let trimmed = raw.trim();
    if trimmed.starts_with("2.5") {
        Some(1)
    } else if trimmed.starts_with("5.0") || trimmed.starts_with("5 ") {
        Some(2)
    } else if trimmed.starts_with("8.0") || trimmed.starts_with("8 ") {
        Some(3)
    } else if trimmed.starts_with("16.0") || trimmed.starts_with("16 ") {
        Some(4)
    } else if trimmed.starts_with("32.0") || trimmed.starts_with("32 ") {
        Some(5)
    } else if trimmed.starts_with("64.0") || trimmed.starts_with("64 ") {
        Some(6)
    } else {
        None
    }
}

pub fn parse_scheduler_str(raw: &str) -> (Option<String>, Vec<String>) {
    let mut current = None;
    let mut available = Vec::new();
    for tok in raw.split_whitespace() {
        if tok.starts_with('[') && tok.ends_with(']') && tok.len() > 2 {
            let name = tok[1..tok.len() - 1].to_owned();
            current = Some(name.clone());
            available.push(name);
        } else if !tok.is_empty() {
            available.push(tok.to_owned());
        }
    }
    (current, available)
}

fn is_physical_block_name(name: &str) -> bool {
    let ignored = [
        "loop", "ram", "zram", "sr", "fd", "dm-", "md", "nbd", "mtd", "pktcdvd",
    ];
    if ignored.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    // Skip NVMe controller multipath sub-nodes like nvme0c0n1 if nvme0n1 exists
    if name.starts_with("nvme") && name.contains('c') && name.contains('n') {
        let after_nvme = &name[4..];
        if let Some((ctrl, _)) = after_nvme.split_once('c') {
            if !ctrl.is_empty() && ctrl.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
        }
    }
    true
}

fn is_local_real_fs(fstype: &str) -> bool {
    matches!(
        fstype.to_ascii_lowercase().as_str(),
        "ext4"
            | "ext3"
            | "ext2"
            | "btrfs"
            | "xfs"
            | "f2fs"
            | "zfs"
            | "bcachefs"
            | "ntfs"
            | "ntfs3"
            | "fuseblk"
            | "vfat"
            | "exfat"
            | "fat32"
            | "jfs"
            | "reiserfs"
    )
}

fn read_symlink_map(dir: &Path) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return map;
    };
    for entry in entries.filter_map(Result::ok) {
        let link_name = entry.file_name().to_string_lossy().into_owned();
        if let Ok(resolved) = fs::canonicalize(entry.path()) {
            let target = resolved.to_string_lossy().into_owned();
            map.entry(target).or_default().push(link_name);
        }
    }
    map
}

fn resolve_dm_parent_disks(
    sys_block_name: &str,
    phys_set: &HashSet<String>,
    part_to_disk: &HashMap<String, String>,
    visited: &mut HashSet<String>,
) -> Vec<String> {
    if !visited.insert(sys_block_name.to_owned()) {
        return Vec::new();
    }
    if phys_set.contains(sys_block_name) {
        return vec![sys_block_name.to_owned()];
    }
    if let Some(parent) = part_to_disk.get(sys_block_name) {
        return vec![parent.clone()];
    }
    let slaves_dir = Path::new("/sys/block").join(sys_block_name).join("slaves");
    let Ok(entries) = fs::read_dir(slaves_dir) else {
        return Vec::new();
    };
    let mut parents = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let slave_name = entry.file_name().to_string_lossy().into_owned();
        for p in resolve_dm_parent_disks(&slave_name, phys_set, part_to_disk, visited) {
            if !parents.contains(&p) {
                parents.push(p);
            }
        }
    }
    parents
}

fn find_pcie_info(block_dir: &Path) -> Value {
    // Walk /sys/block/<dev>/device and parent dirs (up to 3 levels) looking for PCIe link attributes.
    let start = block_dir.join("device");
    let mut cur = fs::canonicalize(start).ok();
    for _ in 0..4 {
        let Some(dir) = cur else {
            break;
        };
        let cur_spd = read_trimmed(&dir.join("current_link_speed"));
        let max_spd = read_trimmed(&dir.join("max_link_speed"));
        let cur_w = read_u64(&dir.join("current_link_width"));
        let max_w = read_u64(&dir.join("max_link_width"));
        if let (Some(cs), Some(ms), Some(cw), Some(mw)) = (cur_spd, max_spd, cur_w, max_w) {
            if let (Some(cg), Some(mg)) = (parse_pcie_gen(&cs), parse_pcie_gen(&ms)) {
                return json!({
                    "current_gen": cg,
                    "max_gen": mg,
                    "current_width": cw,
                    "max_width": mw
                });
            }
        }
        cur = dir.parent().map(Path::to_path_buf);
    }
    Value::Null
}

fn read_nvme_serial_and_model(
    block_name: &str,
    block_dir: &Path,
) -> (Option<String>, Option<String>) {
    let mut serial = read_trimmed(&block_dir.join("device/serial"));
    let mut model = read_trimmed(&block_dir.join("device/model"));
    if serial.is_some() && model.is_some() {
        return (serial, model);
    }
    // Check /sys/class/nvme/nvmeX for nvmeXnY
    if let Some(rest) = block_name.strip_prefix("nvme") {
        if let Some(ctrl_idx) = rest.split('n').next() {
            let ctrl_dir = PathBuf::from(format!("/sys/class/nvme/nvme{ctrl_idx}"));
            if serial.is_none() {
                serial = read_trimmed(&ctrl_dir.join("serial"));
            }
            if model.is_none() {
                model = read_trimmed(&ctrl_dir.join("model"));
            }
        }
    }
    (serial, model)
}

fn detect_bus_type(block_name: &str, block_dir: &Path) -> u64 {
    if block_name.starts_with("nvme") {
        return 17; // NVMe
    }
    if let Ok(resolved) = fs::canonicalize(block_dir.join("device")) {
        let s = resolved.to_string_lossy();
        if s.contains("/usb") {
            return 7; // USB
        }
        if s.contains("/nvme") {
            return 17;
        }
        if s.contains("/ata") {
            return 11; // SATA/ATA
        }
        if s.contains("/mmc") || s.contains("/virtio") {
            return 8; // RAID/MMC/Virtio
        }
    }
    let transport = read_trimmed(&block_dir.join("device/transport"));
    if transport.as_deref() == Some("nvme") {
        17
    } else if block_name.starts_with("sd") {
        11
    } else {
        1
    }
}

fn read_os_release_pretty() -> Option<String> {
    let txt = fs::read_to_string("/etc/os-release").ok()?;
    for line in txt.lines() {
        if let Some(val) = line.strip_prefix("PRETTY_NAME=") {
            return Some(val.trim_matches('"').trim().to_owned());
        }
    }
    None
}

fn detect_display_server() -> &'static str {
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
    {
        "wayland"
    } else if std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("x11"))
    {
        "x11"
    } else {
        "headless"
    }
}

fn autostart_desktop_path() -> Option<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(config_home.join("autostart").join("ssd-sentinel.desktop"))
}

pub fn check_fstrim_timer_enabled() -> Option<bool> {
    let candidates = [
        "/etc/systemd/system/timers.target.wants/fstrim.timer",
        "/usr/lib/systemd/system/timers.target.wants/fstrim.timer",
        "/lib/systemd/system/timers.target.wants/fstrim.timer",
    ];
    for c in candidates {
        if Path::new(c).exists() {
            return Some(true);
        }
    }
    if Path::new("/run/systemd/system").is_dir() {
        return Some(false);
    }
    None
}

pub fn read_pcie_aspm_policy() -> (Option<u64>, Option<String>) {
    let Some(raw) = read_trimmed(Path::new("/sys/module/pcie_aspm/parameters/policy")) else {
        return (None, None);
    };
    let (active, _) = parse_scheduler_str(&raw);
    let code = active.as_deref().map(|a| match a {
        "performance" => 0,
        "default" => 1,
        "powersave" | "powersupersave" => 2,
        _ => 1,
    });
    (code, active)
}

pub fn read_swappiness() -> Option<u64> {
    read_u64(Path::new("/proc/sys/vm/swappiness"))
}

pub fn read_total_swap_bytes() -> Option<u64> {
    let txt = fs::read_to_string("/proc/swaps").ok()?;
    let mut total_kib = 0u64;
    for line in txt.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 3 {
            if let Ok(kb) = cols[2].parse::<u64>() {
                total_kib = total_kib.saturating_add(kb);
            }
        }
    }
    (total_kib > 0).then_some(total_kib.saturating_mul(1024))
}

pub fn inventory(cancel: &AtomicBool) -> Result<Value, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(process::CANCELLED.into());
    }

    let mut block_names: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/block") {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_physical_block_name(&name) {
                let sz = read_u64(&entry.path().join("size")).unwrap_or(0);
                if sz > 0 {
                    block_names.push(name);
                }
            }
        }
    }
    block_names.sort();

    let phys_set: HashSet<String> = block_names.iter().cloned().collect();
    let mut majmin_to_disk: HashMap<String, String> = HashMap::new();
    let mut devpath_to_disk: HashMap<String, String> = HashMap::new();
    let mut part_to_disk: HashMap<String, String> = HashMap::new();

    for name in &block_names {
        let bdir = Path::new("/sys/block").join(name);
        if let Some(mm) = read_trimmed(&bdir.join("dev")) {
            majmin_to_disk.insert(mm, name.clone());
        }
        devpath_to_disk.insert(format!("/dev/{name}"), name.clone());
        if let Ok(subs) = fs::read_dir(&bdir) {
            for sub in subs.filter_map(Result::ok) {
                let sub_path = sub.path();
                if sub_path.join("partition").is_file() {
                    let pname = sub.file_name().to_string_lossy().into_owned();
                    part_to_disk.insert(pname.clone(), name.clone());
                    devpath_to_disk.insert(format!("/dev/{pname}"), name.clone());
                    if let Some(mm) = read_trimmed(&sub_path.join("dev")) {
                        majmin_to_disk.insert(mm, name.clone());
                    }
                }
            }
        }
    }

    // Resolve device-mapper / mdraid nodes (LUKS, LVM, MD) back to physical disks
    if let Ok(entries) = fs::read_dir("/sys/block") {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if phys_set.contains(&name) || (!name.starts_with("dm-") && !name.starts_with("md")) {
                continue;
            }
            let mut visited = HashSet::new();
            let parents = resolve_dm_parent_disks(&name, &phys_set, &part_to_disk, &mut visited);
            if parents.len() == 1 {
                let parent = &parents[0];
                if let Some(mm) = read_trimmed(&entry.path().join("dev")) {
                    majmin_to_disk.insert(mm, parent.clone());
                }
                devpath_to_disk.insert(format!("/dev/{name}"), parent.clone());
            }
        }
    }

    let by_id_map = read_symlink_map(Path::new("/dev/disk/by-id"));
    let by_label_map = read_symlink_map(Path::new("/dev/disk/by-label"));

    let mountinfo_txt = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mounts = parse_mountinfo_str(&mountinfo_txt);
    let mut has_discard_mount = false;
    let mut volumes_by_disk: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut seen_mount_keys: HashSet<(String, String)> = HashSet::new();

    for m in &mounts {
        if m.options
            .split(',')
            .any(|o| o == "discard" || o.starts_with("discard="))
        {
            has_discard_mount = true;
        }
        if !is_local_real_fs(&m.fstype) {
            continue;
        }
        // Skip internal container/snap runtime mounts
        if m.mount_point.starts_with("/var/lib/docker/")
            || m.mount_point.starts_with("/var/lib/containers/")
            || m.mount_point.starts_with("/run/docker/")
            || m.mount_point.starts_with("/snap/")
        {
            continue;
        }
        let disk_name = majmin_to_disk
            .get(&m.major_minor)
            .cloned()
            .or_else(|| devpath_to_disk.get(&m.source).cloned())
            .or_else(|| {
                fs::canonicalize(&m.source).ok().and_then(|c| {
                    let s = c.to_string_lossy().into_owned();
                    devpath_to_disk.get(&s).cloned()
                })
            });
        let Some(disk_name) = disk_name else {
            continue;
        };
        let dedup_key = (m.major_minor.clone(), m.fs_root.clone());
        if !seen_mount_keys.insert(dedup_key) {
            continue;
        }
        let Some((size_bytes, free_bytes)) = statvfs_bytes(Path::new(&m.mount_point)) else {
            continue;
        };
        let canonical_src = fs::canonicalize(&m.source)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| m.source.clone());
        let label = by_label_map
            .get(&canonical_src)
            .and_then(|v| v.first())
            .map(|s| unescape_mount_field(s))
            .or_else(|| {
                if m.mount_point == "/" {
                    Some("Root".to_owned())
                } else {
                    Path::new(&m.mount_point)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                }
            });
        volumes_by_disk.entry(disk_name).or_default().push(json!({
            "letter": m.mount_point,
            "mount_point": m.mount_point,
            "device": canonical_src,
            "label": label,
            "filesystem": m.fstype.to_ascii_uppercase(),
            "size_bytes": size_bytes,
            "free_bytes": free_bytes,
        }));
    }

    let mut out_disks = Vec::with_capacity(block_names.len());
    for (idx, name) in block_names.into_iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Err(process::CANCELLED.into());
        }
        let bdir = Path::new("/sys/block").join(&name);
        let dev_path = format!("/dev/{name}");
        let disk_bytes = read_u64(&bdir.join("size")).map(|s| s.saturating_mul(512));
        let rotational = read_u64(&bdir.join("queue/rotational"));
        let vendor = read_trimmed(&bdir.join("device/vendor")).unwrap_or_default();
        let raw_model = read_trimmed(&bdir.join("device/model")).unwrap_or_default();
        let (nvme_serial, nvme_model) = read_nvme_serial_and_model(&name, &bdir);
        let model = if !raw_model.is_empty() {
            if !vendor.is_empty()
                && !raw_model
                    .to_ascii_lowercase()
                    .starts_with(&vendor.to_ascii_lowercase())
                && !vendor.eq_ignore_ascii_case("ATA")
            {
                format!("{vendor} {raw_model}")
            } else {
                raw_model
            }
        } else {
            nvme_model.clone().unwrap_or_else(|| name.clone())
        };

        let mut serial = read_trimmed(&bdir.join("device/serial")).or_else(|| nvme_serial.clone());
        let mut unique_ids = Vec::new();
        if let Some(wwid) = read_trimmed(&bdir.join("wwid")) {
            unique_ids.push(wwid);
        }
        if let Some(ids) = by_id_map.get(&dev_path) {
            for link in ids {
                if !link.starts_with("wwn-") && serial.is_none() {
                    if let Some((_, sn)) = link.rsplit_once('_') {
                        if sn.len() >= 4 && !sn.contains(':') {
                            serial = Some(sn.to_owned());
                        }
                    }
                }
                if let Some(wwn) = link.strip_prefix("wwn-") {
                    if let Some(hex) = wwn.strip_prefix("0x") {
                        unique_ids.push(hex.to_owned());
                    } else {
                        unique_ids.push(wwn.to_owned());
                    }
                } else {
                    unique_ids.push(link.clone());
                }
            }
        }

        let bus_type = detect_bus_type(&name, &bdir);
        let pcie = if bus_type == 17 {
            find_pcie_info(&bdir)
        } else {
            Value::Null
        };

        let write_cache = read_trimmed(&bdir.join("queue/write_cache")).map(|wc| {
            let enabled = wc.eq_ignore_ascii_case("write back");
            json!({
                "type": wc,
                "enabled": enabled,
                "changeable": true,
                "flush_supported": true
            })
        });
        let (sched_cur, sched_avail) = read_trimmed(&bdir.join("queue/scheduler"))
            .map(|s| parse_scheduler_str(&s))
            .unwrap_or((None, Vec::new()));

        let mut allocated_bytes = 0u64;
        let mut misaligned = 0u64;
        if let Ok(subs) = fs::read_dir(&bdir) {
            for sub in subs.filter_map(Result::ok) {
                let sp = sub.path();
                if sp.join("partition").is_file() {
                    let p_bytes = read_u64(&sp.join("size")).unwrap_or(0).saturating_mul(512);
                    allocated_bytes = allocated_bytes.saturating_add(p_bytes);
                    let start_bytes = read_u64(&sp.join("start")).unwrap_or(0).saturating_mul(512);
                    let align_off = read_u64(&sp.join("alignment_offset")).unwrap_or(0);
                    if !(start_bytes + align_off).is_multiple_of(4096) {
                        misaligned += 1;
                    }
                }
            }
        }
        let mut vols = volumes_by_disk.remove(&name).unwrap_or_default();
        vols.sort_by_key(|v| v["letter"].as_str().unwrap_or("").len());

        out_disks.push(json!({
            "number": idx as u64,
            "name": name,
            "device_path": dev_path,
            "serial": serial,
            "model": model,
            "nvme_serial": nvme_serial,
            "nvme_model": nvme_model,
            "unique_ids": unique_ids,
            "bus_type": bus_type,
            "rotational": rotational,
            "disk_bytes": disk_bytes,
            "volumes": vols,
            "pcie": pcie,
            "write_cache": write_cache,
            "io_scheduler": {
                "current": sched_cur,
                "available": sched_avail
            },
            "layout": {
                "allocated_bytes": allocated_bytes,
                "misaligned_partitions": misaligned
            }
        }));
    }

    let trim_timer = check_fstrim_timer_enabled();
    let trim_enabled = match (trim_timer, has_discard_mount) {
        (Some(true), _) | (_, true) => Some(true),
        (Some(false), false) => Some(false),
        _ => None,
    };
    let (pcie_aspm, pcie_aspm_policy) = read_pcie_aspm_policy();
    let swap_bytes = read_total_swap_bytes();
    let swappiness = read_swappiness();
    let zswap_enabled = read_trimmed(Path::new("/sys/module/zswap/parameters/enabled"))
        .map(|v| matches!(v.as_str(), "Y" | "y" | "1" | "true"));
    let autostart_enabled = autostart_desktop_path().is_some_and(|p| p.is_file());

    Ok(json!({
        "disks": out_disks,
        "trim_enabled": trim_enabled,
        "system_tuning": {
            "trim_enabled": trim_enabled,
            "fstrim_timer_enabled": trim_timer,
            "hibernate_enabled": swap_bytes.is_some(),
            "hiberfil_bytes": swap_bytes,
            "swappiness": swappiness,
            "zswap_enabled": zswap_enabled,
            "pcie_aspm": pcie_aspm,
            "pcie_aspm_policy": pcie_aspm_policy,
            "autostart_enabled": autostart_enabled,
            "long_paths_enabled": true,
            "os_name": read_os_release_pretty(),
            "desktop_env": std::env::var("XDG_CURRENT_DESKTOP").ok(),
            "display_server": detect_display_server(),
        },
        "error": null
    }))
}

pub fn performance(previous: &mut Value, cancel: &AtomicBool) -> Result<Value, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(process::CANCELLED.into());
    }
    let stats_txt = fs::read_to_string("/proc/diskstats")
        .map_err(|e| format!("/proc/diskstats okunamadı: {e}"))?;
    let rows = parse_diskstats_str(&stats_txt);
    let now_ns = monotonic_ns();
    let prev_ns = previous["timestamp_ns"].as_u64();

    let mut phys_names: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/block") {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_physical_block_name(&name)
                && read_u64(&entry.path().join("size")).unwrap_or(0) > 0
            {
                phys_names.push(name);
            }
        }
    }
    phys_names.sort();
    let phys_index: HashMap<String, usize> = phys_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i))
        .collect();

    let mut raw_disks = Vec::new();
    let mut out_disks = Vec::new();
    for row in rows {
        let Some(&disk_num) = phys_index.get(&row.name) else {
            continue;
        };
        let curr_raw = json!({
            "number": disk_num as u64,
            "name": row.name,
            "reads_completed": row.reads_completed,
            "sectors_read": row.sectors_read,
            "ms_reading": row.ms_reading,
            "writes_completed": row.writes_completed,
            "sectors_written": row.sectors_written,
            "ms_writing": row.ms_writing,
            "ms_doing_io": row.ms_doing_io,
            "queue_length": row.ios_in_progress,
        });
        let old = previous["disks"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| r["name"] == row.name);

        let (read_bps, write_bps, read_iops, write_iops, busy_percent, read_lat, write_lat) =
            if let (Some(old), Some(p_ns)) = (old, prev_ns) {
                let dt_ns = now_ns.saturating_sub(p_ns);
                if dt_ns > 0 {
                    let dt = dt_ns as f64 / 1_000_000_000.0;
                    let dt_ms = dt_ns as f64 / 1_000_000.0;
                    let d_reads = row
                        .reads_completed
                        .saturating_sub(old["reads_completed"].as_u64().unwrap_or(0));
                    let d_writes = row
                        .writes_completed
                        .saturating_sub(old["writes_completed"].as_u64().unwrap_or(0));
                    let d_sec_r = row
                        .sectors_read
                        .saturating_sub(old["sectors_read"].as_u64().unwrap_or(0));
                    let d_sec_w = row
                        .sectors_written
                        .saturating_sub(old["sectors_written"].as_u64().unwrap_or(0));
                    let d_ms_r = row
                        .ms_reading
                        .saturating_sub(old["ms_reading"].as_u64().unwrap_or(0));
                    let d_ms_w = row
                        .ms_writing
                        .saturating_sub(old["ms_writing"].as_u64().unwrap_or(0));
                    let d_busy_ms = row
                        .ms_doing_io
                        .saturating_sub(old["ms_doing_io"].as_u64().unwrap_or(0));
                    let r_lat = (d_reads > 0).then(|| d_ms_r as f64 / d_reads as f64);
                    let w_lat = (d_writes > 0).then(|| d_ms_w as f64 / d_writes as f64);
                    let busy = ((d_busy_ms as f64 / dt_ms) * 100.0).clamp(0.0, 100.0);
                    (
                        (d_sec_r * 512) as f64 / dt,
                        (d_sec_w * 512) as f64 / dt,
                        d_reads as f64 / dt,
                        d_writes as f64 / dt,
                        busy,
                        r_lat,
                        w_lat,
                    )
                } else {
                    (0.0, 0.0, 0.0, 0.0, 0.0, None, None)
                }
            } else {
                (0.0, 0.0, 0.0, 0.0, 0.0, None, None)
            };

        raw_disks.push(curr_raw);
        out_disks.push(json!({
            "number": disk_num as u64,
            "name": row.name,
            "read_bps": read_bps,
            "write_bps": write_bps,
            "read_iops": read_iops,
            "write_iops": write_iops,
            "read_latency_ms": read_lat,
            "write_latency_ms": write_lat,
            "busy_percent": busy_percent,
            "queue_length": row.ios_in_progress,
        }));
    }

    *previous = json!({
        "timestamp_ns": now_ns,
        "disks": raw_disks
    });
    Ok(json!({ "disks": out_disks, "error": null }))
}

pub fn fstrim_volume(mount_point: &str) -> Result<Value, String> {
    if linux_priv::is_root() {
        return linux_priv::trim_mount_point_local(mount_point);
    }
    if linux_priv::is_helper_active() {
        return linux_priv::call(HelperOp::FsTrim {
            mount_point: mount_point.to_owned(),
        });
    }
    linux_priv::trim_mount_point_local(mount_point)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FiemapExtentRaw {
    fe_logical: u64,
    fe_physical: u64,
    fe_length: u64,
    fe_reserved64: [u64; 2],
    fe_flags: u32,
    fe_reserved: [u32; 3],
}

#[repr(C)]
struct FiemapHeaderRaw {
    fm_start: u64,
    fm_length: u64,
    fm_flags: u32,
    fm_mapped_extents: u32,
    fm_extent_count: u32,
    fm_reserved: u32,
}

const FS_IOC_FIEMAP: libc::c_ulong = 0xC020_660B;
const FIEMAP_FLAG_SYNC: u32 = 0x0000_0001;
const FIEMAP_EXTENT_LAST: u32 = 0x0000_0001;

/// Queries physical extents `(physical_byte_offset, length_bytes)` for a regular file
/// via Linux `FS_IOC_FIEMAP` (read-only, zero mutation).
pub fn fiemap_extents(path: &Path) -> Option<Vec<(u64, u64)>> {
    use std::os::unix::io::AsRawFd;
    let file = fs::File::open(path).ok()?;
    let mut all = Vec::new();
    let mut start = 0u64;
    const BATCH: usize = 64;
    let hdr_size = std::mem::size_of::<FiemapHeaderRaw>();
    let ext_size = std::mem::size_of::<FiemapExtentRaw>();
    let mut buf = vec![0u8; hdr_size + BATCH * ext_size];

    for _ in 0..256 {
        buf.fill(0);
        let hdr = FiemapHeaderRaw {
            fm_start: start,
            fm_length: u64::MAX - start,
            fm_flags: FIEMAP_FLAG_SYNC,
            fm_mapped_extents: 0,
            fm_extent_count: BATCH as u32,
            fm_reserved: 0,
        };
        unsafe {
            std::ptr::write_unaligned(buf.as_mut_ptr().cast::<FiemapHeaderRaw>(), hdr);
        }
        let rc = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_FIEMAP as _, buf.as_mut_ptr()) };
        if rc != 0 {
            return (!all.is_empty()).then_some(all);
        }
        let out_hdr = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<FiemapHeaderRaw>()) };
        let count = (out_hdr.fm_mapped_extents as usize).min(BATCH);
        if count == 0 {
            break;
        }
        let mut last = false;
        let mut next_start = start;
        for i in 0..count {
            let off = hdr_size + i * ext_size;
            let ext = unsafe {
                std::ptr::read_unaligned(
                    buf[off..off + ext_size].as_ptr().cast::<FiemapExtentRaw>(),
                )
            };
            all.push((ext.fe_physical, ext.fe_length));
            next_start = ext.fe_logical.saturating_add(ext.fe_length);
            if ext.fe_flags & FIEMAP_EXTENT_LAST != 0 {
                last = true;
            }
        }
        if last || next_start <= start {
            break;
        }
        start = next_start;
    }
    Some(all)
}

/// Returns `(partition_offset_bytes, partition_size_bytes)` for a Linux volume mount point.
pub fn volume_disk_span(device_path: &str) -> Option<(u64, u64)> {
    let canonical = fs::canonicalize(device_path).ok()?;
    let name = canonical.file_name()?.to_string_lossy().into_owned();
    let class_dir = Path::new("/sys/class/block").join(&name);
    let start_sectors = read_u64(&class_dir.join("start")).unwrap_or(0);
    let size_sectors = read_u64(&class_dir.join("size"))?;
    Some((
        start_sectors.saturating_mul(512),
        size_sectors.saturating_mul(512),
    ))
}

pub fn discover_smartctl() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("smartmontools").join("smartctl");
            if bundled.is_file() {
                return bundled;
            }
            let sibling = dir.join("smartctl");
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    linux_priv::find_trusted_binary(&["smartctl"]).unwrap_or_else(|| PathBuf::from("smartctl"))
}

pub fn resolve_linux_disk_targets(inventory: &Value) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for disk in inventory["disks"].as_array().into_iter().flatten() {
        if let Some(dev_path) = disk["device_path"].as_str() {
            let kind = match disk["bus_type"].as_u64() {
                Some(17) => Some("nvme".to_owned()),
                Some(7) => Some("sat".to_owned()),
                _ => None,
            };
            out.push((dev_path.to_owned(), kind));
        }
    }
    out
}

/// Builds honest `smartctl -j` fallback snapshots when neither `smartctl` nor UDisks2
/// can read SMART counters (e.g., inside restricted containers or unprivileged sandboxes).
/// Sets `exit_status = 2` so `model::normalise` marks `read_incomplete: true` & `health.percent: null`.
pub fn synthesize_sysfs_fallback_snapshots(inventory: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for disk in inventory["disks"].as_array().into_iter().flatten() {
        let Some(dev_path) = disk["device_path"].as_str() else {
            continue;
        };
        let model = disk["model"].as_str().unwrap_or("Linux Block Device");
        let serial = disk["serial"]
            .as_str()
            .or_else(|| disk["nvme_serial"].as_str());
        let cap = disk["disk_bytes"].as_u64();
        let bus_type = disk["bus_type"].as_u64().unwrap_or(0);
        let rot = disk["rotational"].as_u64();
        let (dev_type, proto) = match bus_type {
            17 => ("nvme", "NVMe"),
            7 => ("sat", "USB"),
            _ => ("sat", "ATA"),
        };
        out.push(json!({
            "model_name": model,
            "serial_number": serial,
            "rotation_rate": if bus_type == 17 { json!(0) } else { json!(rot) },
            "user_capacity": { "bytes": cap },
            "device": { "name": dev_path, "type": dev_type, "protocol": proto },
            "smartctl": { "exit_status": 2, "source": "sysfs_fallback" }
        }));
    }
    out
}
