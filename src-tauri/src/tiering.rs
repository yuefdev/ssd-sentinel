use crate::{
    core::{lock, Core, JobRequest},
    jobs::{autopilot_hdd_defrag_guard_for_lang, cancelled, phase_bi},
    model::Disk,
    platform, process,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;
const SSD_COMPANION_MIN_FREE_BYTES: u64 = 20 * GIB;
const RELOCATE_FREE_RESERVE_BYTES: u64 = 10 * GIB;
#[cfg(windows)]
const SSD_EXTENT_THRESHOLD: usize = 50;
#[cfg(windows)]
const SSD_WRITE_BUDGET_BYTES: u64 = 2 * GIB;
#[cfg(windows)]
const HDD_OUTER_MOVE_BUDGET_BYTES: u64 = 4 * GIB;
const ANALYZE_MAX_ENTRIES: usize = 12_000;
const ANALYZE_TIME_BUDGET: Duration = Duration::from_millis(3500);
const ANALYZE_TOP_CANDIDATES: usize = 25;
#[cfg(windows)]
const ANALYZE_EXTENT_SAMPLE_LIMIT: usize = 400;
const ARCHIVE_ROOT_NAME: &str = "SSD-Sentinel-Arsiv";

const FORBIDDEN_COMPONENTS: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "appdata",
    "$recycle.bin",
    "system volume information",
    "recovery",
    "perflogs",
    "msocache",
    "config.msi",
    "ssd-sentinel-arsiv",
    ".ssd-sentinel-quarantine",
    "$windows.~bt",
    "$windows.~ws",
    "$sysreset",
    "pagefile.sys",
    "hiberfil.sys",
    "swapfile.sys",
    "dumpstack.log",
    "dumpstack.log.tmp",
    "bootmgr",
    "bootnxt",
    "ntuser.dat",
    "usrclass.dat",
];

const USER_SUBLOCATIONS: &[&str] = &[
    "Downloads",
    "Videos",
    "Documents",
    "Desktop",
    "Music",
    "Pictures",
];

const CONTAINER_DIR_NAMES: &[&str] = &[
    "games",
    "oyunlar",
    "steamlibrary",
    "downloads",
    "videos",
    "documents",
    "desktop",
    "music",
    "pictures",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierScore {
    pub cold_score: u32,
    pub hot_score: u32,
    pub category: String,
    pub reason_tr: String,
    pub recommended_direction: String,
}

pub fn is_ssd_disk(disk: &Disk) -> bool {
    disk.kind != "HDD" && disk.kind.to_ascii_uppercase().contains("SSD")
}

/// Returns `true` only when `path` exists on disk and is a reparse point / symlink.
/// Unlike `platform::reparse`, nonexistent synthetic paths return `false` so pure
/// scoring functions can evaluate paths without requiring them on disk.
fn is_existing_reparse(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn has_forbidden_components(path: &Path, is_dir_hint: Option<bool>) -> bool {
    let raw = path.to_string_lossy();
    if raw.trim().is_empty() {
        return true;
    }
    let mut normals: Vec<String> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => return true,
            Component::Prefix(p) => {
                use std::path::Prefix;
                match p.kind() {
                    Prefix::Disk(_) | Prefix::VerbatimDisk(_) => {}
                    _ => return true,
                }
            }
            Component::RootDir | Component::CurDir => {}
            Component::Normal(os) => {
                let s = os.to_string_lossy();
                let lower = s.to_ascii_lowercase();
                if lower.is_empty()
                    || lower == ".."
                    || FORBIDDEN_COMPONENTS.contains(&lower.as_str())
                    || lower.starts_with("ssd-sentinel-")
                    || lower.ends_with(".sentinel-moving")
                {
                    return true;
                }
                normals.push(lower);
            }
        }
    }
    if normals.is_empty() {
        return true;
    }
    if matches!(normals[0].as_str(), "boot" | "efi" | "drivers") {
        return true;
    }
    if normals.len() == 1 && normals[0] == "users" {
        return true;
    }
    if is_dir_hint == Some(true) && normals.len() == 2 && normals[0] == "users" {
        return true;
    }
    false
}

/// Strictly rejects forbidden Windows system/OS/dangerous paths, traversal (`..`),
/// bare drive roots, and existing reparse points / symlinks.
pub fn is_forbidden_path(path: &Path) -> bool {
    has_forbidden_components(path, None) || is_existing_reparse(path)
}

fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("\\\\?\\") {
        PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

pub(crate) fn is_forbidden_in_test_sandbox(path: &Path, is_dir_hint: Option<bool>) -> bool {
    if path.components().any(|c| matches!(c, Component::ParentDir)) || is_existing_reparse(path) {
        return true;
    }
    let temp_raw = strip_verbatim(&std::env::temp_dir());
    let temp_canon = std::env::temp_dir()
        .canonicalize()
        .map(|p| strip_verbatim(&p))
        .unwrap_or_else(|_| temp_raw.clone());
    let norm = strip_verbatim(path);
    let canon = path
        .canonicalize()
        .map(|p| strip_verbatim(&p))
        .unwrap_or_else(|_| norm.clone());

    for candidate_temp in [&temp_canon, &temp_raw] {
        for candidate_path in [&canon, &norm] {
            if let Ok(rel) = candidate_path.strip_prefix(candidate_temp) {
                // Skip the single random `.tmpXXXX` directory created by `tempfile::tempdir()`
                // if present, and strictly validate all components beneath it.
                let mut comps = rel.components();
                if let Some(first) = comps.next() {
                    let first_str = first.as_os_str().to_string_lossy().to_ascii_lowercase();
                    if first_str.starts_with(".tmp") || first_str.starts_with("tmp") {
                        let sub: PathBuf = comps.collect();
                        if !sub.as_os_str().is_empty() {
                            return has_forbidden_components(&sub, is_dir_hint);
                        }
                    }
                }
                return has_forbidden_components(rel, is_dir_hint);
            }
        }
    }
    has_forbidden_components(path, is_dir_hint)
}

fn classify_category(path: &Path, is_dir: bool) -> &'static str {
    let full_lower = path.to_string_lossy().to_ascii_lowercase();
    let name_lower = path
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if is_dir {
        if [
            "games",
            "oyun",
            "steam",
            "epic games",
            "gog",
            "riot games",
            "ubisoft",
            "xboxgames",
            "battle.net",
            "blizzard",
        ]
        .iter()
        .any(|k| name_lower.contains(k) || full_lower.contains(&format!("\\{k}\\")))
        {
            return "game";
        }
        if [
            "src",
            "repo",
            "repos",
            "project",
            "projects",
            "proje",
            "projeler",
            "code",
            "workspace",
            "node_modules",
            "target",
            ".git",
        ]
        .iter()
        .any(|k| name_lower == *k || name_lower.contains(k))
        {
            return "code";
        }
        if ["video", "film", "movie", "record", "kayit", "capture"]
            .iter()
            .any(|k| name_lower.contains(k))
        {
            return "video";
        }
        if ["arsiv", "archive", "backup", "yedek"]
            .iter()
            .any(|k| name_lower.contains(k))
        {
            return "archive";
        }
        if ["photo", "picture", "foto", "resim"]
            .iter()
            .any(|k| name_lower.contains(k))
        {
            return "photo";
        }
        if ["music", "muzik", "audio", "ses"]
            .iter()
            .any(|k| name_lower.contains(k))
        {
            return "audio";
        }
        return "folder";
    }

    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "mp4" | "mkv" | "mov" | "avi" | "wmv" | "flv" | "webm" | "m4v" | "mts" | "m2ts" => "video",
        "iso" | "img" | "vhd" | "vhdx" | "vmdk" | "vdi" | "wim" | "esd" => "iso",
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst" | "cab" | "bak" => "archive",
        "exe" | "msi" | "msix" | "appx" | "dmg" | "pkg" => "installer",
        "jpg" | "jpeg" | "png" | "raw" | "cr2" | "nef" | "arw" | "dng" | "psd" | "tif" | "tiff"
        | "bmp" | "webp" | "heic" => "photo",
        "flac" | "wav" | "mp3" | "aac" | "ogg" | "m4a" | "aiff" => "audio",
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "cpp" | "cc" | "c" | "h" | "hpp"
        | "cs" | "java" | "kt" | "swift" | "json" | "toml" | "yaml" | "yml" | "lock" | "sln"
        | "csproj" => "code",
        "db" | "sqlite" | "sqlite3" | "mdf" | "ldf" | "ndf" | "ibd" | "ini" | "cfg" | "conf"
        | "reg" | "sys" | "dll" => "database",
        _ => "other",
    }
}

fn category_label_tr(category: &str) -> &'static str {
    match category {
        "video" => "video dosyası",
        "iso" => "disk imajı (ISO/VHD)",
        "archive" => "arşiv dosyası",
        "installer" => "kurulum paketi",
        "photo" => "fotoğraf arşivi",
        "audio" => "ses / müzik arşivi",
        "game" => "oyun / uygulama klasörü",
        "code" => "kod / proje verisi",
        "database" => "veritabanı / yapılandırma",
        "folder" => "kullanıcı klasörü",
        _ => "dosya",
    }
}

/// Cold/Hot heuristic scoring for SSD→HDD offloading (`cold_score`) and HDD→SSD promotion (`hot_score`).
/// Returns `None` if `path` is forbidden/system/reparse or zero-sized.
pub fn score_tier_candidate(
    path: &Path,
    is_dir: bool,
    size_bytes: u64,
    days_untouched: u64,
    source_is_ssd: bool,
) -> Option<TierScore> {
    if size_bytes == 0 || has_forbidden_components(path, Some(is_dir)) || is_existing_reparse(path)
    {
        return None;
    }
    score_unchecked(path, is_dir, size_bytes, days_untouched, source_is_ssd)
}

fn score_unchecked(
    path: &Path,
    is_dir: bool,
    size_bytes: u64,
    days_untouched: u64,
    source_is_ssd: bool,
) -> Option<TierScore> {
    if size_bytes == 0 {
        return None;
    }
    let category = classify_category(path, is_dir);
    let full_lower = path.to_string_lossy().to_ascii_lowercase();
    let in_downloads = full_lower.contains("downloads") || full_lower.contains("indirilenler");

    // --- Cold Score (0..=100) for SSD -> HDD offloading ---
    let age_cold: i32 = if days_untouched >= 180 {
        45
    } else if days_untouched >= 90 {
        40
    } else if days_untouched >= 60 {
        34
    } else if days_untouched >= 30 {
        26
    } else if days_untouched >= 14 {
        16
    } else if days_untouched >= 7 {
        8
    } else {
        0
    };

    let profile_cold: i32 = match category {
        "video" | "iso" => 35,
        "archive" => 32,
        "installer" => {
            if in_downloads {
                30
            } else {
                22
            }
        }
        "audio" | "photo" => 22,
        "folder" => 18,
        "game" => {
            if days_untouched >= 30 {
                16
            } else {
                0
            }
        }
        "other" => 12,
        "code" | "database" => -35,
        _ => 10,
    };

    let size_cold: i32 = if is_dir {
        if size_bytes >= 10 * GIB {
            20
        } else if size_bytes >= 2 * GIB {
            16
        } else if size_bytes >= 500 * MIB {
            12
        } else if size_bytes >= 100 * MIB {
            8
        } else {
            0
        }
    } else if size_bytes >= 2 * GIB {
        20
    } else if size_bytes >= 500 * MIB {
        16
    } else if size_bytes >= 100 * MIB {
        12
    } else if size_bytes >= 50 * MIB {
        8
    } else {
        0
    };

    let cold_score = (age_cold + profile_cold + size_cold).clamp(0, 100) as u32;

    // --- Hot Score (0..=100) for HDD -> SSD promotion ---
    let recency_hot: i32 = if days_untouched <= 1 {
        50
    } else if days_untouched <= 3 {
        44
    } else if days_untouched <= 7 {
        36
    } else if days_untouched <= 14 {
        26
    } else if days_untouched <= 30 {
        10
    } else {
        0
    };

    let profile_hot: i32 = match category {
        "game" => 30,
        "code" | "database" => 28,
        "folder" => 20,
        "photo" | "audio" | "other" => 10,
        "video" | "iso" | "archive" | "installer" => -15,
        _ => 5,
    };

    let min_hot = 50 * MIB;
    let max_hot = 40 * GIB;
    let size_hot: i32 = if (min_hot..=max_hot).contains(&size_bytes) {
        if is_dir {
            20
        } else {
            12
        }
    } else if size_bytes > max_hot {
        -40
    } else if size_bytes < 10 * MIB {
        -10
    } else {
        0
    };

    let mut hot_raw = (recency_hot + profile_hot + size_hot).clamp(0, 100);
    if days_untouched > 14 {
        hot_raw = hot_raw.min(35);
    }
    let hot_score = hot_raw as u32;

    let label = category_label_tr(category);
    let (recommended_direction, reason_tr) = if source_is_ssd {
        let reason = if is_dir {
            format!(
                "{days_untouched} gündür dokunulmayan {label}; NTFS Junction ile kısayollar bozulmadan HDD'ye aktarılabilir."
            )
        } else {
            format!(
                "{days_untouched} gündür kullanılmayan {label}; HDD'ye taşınarak SSD SLC önbelleği rahatlatılabilir."
            )
        };
        ("to_hdd".to_owned(), reason)
    } else {
        let when = if days_untouched == 0 {
            "Bugün aktif kullanılan".to_owned()
        } else {
            format!("Son {days_untouched} gün içinde kullanılan")
        };
        let reason = if is_dir {
            format!(
                "{when} {label}; SSD'ye terfi ettirilerek yükleme ve rastgele erişim hızlandırılabilir."
            )
        } else {
            format!("{when} {label}; hızlı erişim için SSD'ye veya dış izlere alınabilir.")
        };
        ("to_ssd".to_owned(), reason)
    };

    Some(TierScore {
        cold_score,
        hot_score,
        category: category.to_owned(),
        reason_tr,
        recommended_direction,
    })
}

fn find_companion(disks: &[Disk], selected: &Disk, letter: &str) -> (Option<&'static str>, Value) {
    let selected_is_ssd = is_ssd_disk(selected);
    let mut best: Option<(u64, Value)> = None;

    if selected_is_ssd {
        // SSD -> best NTFS HDD volume with maximum free space.
        for disk in disks {
            if disk.id == selected.id || disk.kind != "HDD" {
                continue;
            }
            for vol in &disk.volumes {
                let Some(v_letter) = vol["letter"].as_str() else {
                    continue;
                };
                if v_letter.len() != 1
                    || !v_letter.as_bytes()[0].is_ascii_uppercase()
                    || v_letter.eq_ignore_ascii_case(letter)
                {
                    continue;
                }
                if vol["filesystem"].as_str() != Some("NTFS") {
                    continue;
                }
                let free = vol["free_bytes"].as_u64().unwrap_or(0);
                if free == 0 {
                    continue;
                }
                if best.as_ref().is_none_or(|(b, _)| free > *b) {
                    best = Some((
                        free,
                        json!({
                            "disk_id": disk.id,
                            "model": disk.model,
                            "letter": v_letter,
                            "type": disk.kind,
                            "free_bytes": free,
                            "role": "hdd_archive"
                        }),
                    ));
                }
            }
        }
        match best {
            Some((_, val)) => (Some("hdd_archive"), val),
            None => (None, Value::Null),
        }
    } else {
        // HDD -> best NTFS SSD volume with >= 20 GiB free space.
        for disk in disks {
            if disk.id == selected.id || !is_ssd_disk(disk) {
                continue;
            }
            for vol in &disk.volumes {
                let Some(v_letter) = vol["letter"].as_str() else {
                    continue;
                };
                if v_letter.len() != 1
                    || !v_letter.as_bytes()[0].is_ascii_uppercase()
                    || v_letter.eq_ignore_ascii_case(letter)
                {
                    continue;
                }
                if vol["filesystem"].as_str() != Some("NTFS") {
                    continue;
                }
                let free = vol["free_bytes"].as_u64().unwrap_or(0);
                if free < SSD_COMPANION_MIN_FREE_BYTES {
                    continue;
                }
                if best.as_ref().is_none_or(|(b, _)| free > *b) {
                    best = Some((
                        free,
                        json!({
                            "disk_id": disk.id,
                            "model": disk.model,
                            "letter": v_letter,
                            "type": disk.kind,
                            "free_bytes": free,
                            "role": "ssd_boost"
                        }),
                    ));
                }
            }
        }
        match best {
            Some((_, val)) => (Some("ssd_boost"), val),
            None => (None, Value::Null),
        }
    }
}

fn volume_space_info(selected: &Disk, letter: &str, allow_live_fallback: bool) -> (u64, u64) {
    if let Some(vol) = selected.volumes.iter().find(|v| {
        v["letter"]
            .as_str()
            .is_some_and(|l| l.eq_ignore_ascii_case(letter))
    }) {
        let size = vol["size_bytes"].as_u64().unwrap_or(0);
        let free = vol["free_bytes"].as_u64().unwrap_or(0);
        if size > 0 {
            return (size, free);
        }
    }
    #[cfg(windows)]
    if allow_live_fallback {
        if let Some((size, free)) = win::volume_free_space(&format!("{letter}:\\")) {
            return (size, free);
        }
    }
    #[cfg(not(windows))]
    let _ = allow_live_fallback;
    (0, 0)
}

fn compute_slc_health(is_ssd: bool, fill_percent: f64) -> (&'static str, String) {
    if is_ssd {
        if fill_percent > 75.0 {
            (
                "high",
                format!(
                    "SSD doluluğu %75'in üzerinde (%{fill_percent:.1}); dinamik SLC yazma önbelleği daralıyor ve yazma hızı düşebilir. Soğuk verileri HDD'ye aktarmanız önerilir."
                ),
            )
        } else if fill_percent >= 55.0 {
            (
                "moderate",
                format!(
                    "SSD doluluğu orta düzeyde (%{fill_percent:.1}); SLC önbelleği sağlıklı ancak büyük soğuk arşivleri taşımak dayanımı korur."
                ),
            )
        } else {
            (
                "low",
                format!(
                    "SSD doluluğu düşük (%{fill_percent:.1}); dinamik SLC yazma önbelleği tam kapasiteyle çalışıyor."
                ),
            )
        }
    } else if fill_percent > 85.0 {
        (
            "high",
            format!(
                "HDD doluluğu %{fill_percent:.1}; veriler yavaş iç izlere taşmış olabilir. HDD Dış-İz Hızlı Yerleşim önerilir."
            ),
        )
    } else if fill_percent >= 65.0 {
        (
            "moderate",
            format!(
                "HDD doluluğu %{fill_percent:.1}; sıcak klasörleri dış izlere toplamak veya SSD'ye terfi ettirmek erişimi hızlandırır."
            ),
        )
    } else {
        (
            "low",
            format!(
                "HDD doluluğu %{fill_percent:.1}; dış izlerde hızlı yerleşim için geniş boş alan mevcut."
            ),
        )
    }
}

fn days_since_touched(meta: &std::fs::Metadata, now: SystemTime) -> u64 {
    let mtime = meta.modified().ok();
    let atime = meta.accessed().ok().filter(|t| *t <= now);
    let latest = match (mtime, atime) {
        (Some(m), Some(a)) => Some(if a > m { a } else { m }),
        (Some(m), None) => Some(m),
        (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    latest
        .and_then(|t| now.duration_since(t).ok())
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

fn candidate_id(path: &Path) -> String {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    format!("{:x}", Sha256::digest(lower.as_bytes()))[..16].to_owned()
}

struct ScanState {
    started: Instant,
    now: SystemTime,
    entries_visited: usize,
    extent_samples: usize,
    extreme_fragmented_count: u64,
    worst_extents: u64,
}

impl ScanState {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            now: SystemTime::now(),
            entries_visited: 0,
            extent_samples: 0,
            extreme_fragmented_count: 0,
            worst_extents: 0,
        }
    }

    fn budget_exhausted(&self) -> bool {
        self.entries_visited >= ANALYZE_MAX_ENTRIES || self.started.elapsed() >= ANALYZE_TIME_BUDGET
    }

    fn sample_extents(&mut self, path: &Path, size_bytes: u64) {
        #[cfg(windows)]
        {
            if self.extent_samples >= ANALYZE_EXTENT_SAMPLE_LIMIT
                || size_bytes < 64 * 1024
                || self.budget_exhausted()
            {
                return;
            }
            self.extent_samples += 1;
            if let Some(count) = win::query_path_extent_count(path) {
                let c = count as u64;
                if c > self.worst_extents {
                    self.worst_extents = c;
                }
                if count >= SSD_EXTENT_THRESHOLD {
                    self.extreme_fragmented_count += 1;
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (path, size_bytes);
        }
    }
}

/// Rolls up total size, file count, and most recent touch age (`min days_untouched`)
/// inside `dir`, strictly respecting scan entry/time budgets and skipping reparse points.
fn rollup_directory(dir: &Path, root: &Path, state: &mut ScanState) -> (u64, u64, u64) {
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut total_bytes = 0u64;
    let mut file_count = 0u64;
    let mut min_days = u64::MAX;

    while let Some((cur, depth)) = stack.pop() {
        if state.budget_exhausted() || depth > 6 {
            break;
        }
        if is_existing_reparse(&cur) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&cur) else {
            continue;
        };
        for entry in rd.filter_map(Result::ok) {
            if state.budget_exhausted() {
                break;
            }
            state.entries_visited += 1;
            let p = entry.path();
            if is_existing_reparse(&p) {
                continue;
            }
            if let Ok(rel) = p.strip_prefix(root) {
                if has_forbidden_components(rel, None) {
                    continue;
                }
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                if depth < 6 && stack.len() < 1024 {
                    stack.push((p, depth + 1));
                }
            } else if meta.is_file() {
                let sz = meta.len();
                total_bytes = total_bytes.saturating_add(sz);
                file_count = file_count.saturating_add(1);
                let d = days_since_touched(&meta, state.now);
                if d < min_days {
                    min_days = d;
                }
                state.sample_extents(&p, sz);
            }
        }
    }

    if min_days == u64::MAX {
        min_days = std::fs::metadata(dir)
            .map(|m| days_since_touched(&m, state.now))
            .unwrap_or(0);
    }
    (total_bytes, file_count, min_days)
}

/// Read-only replay-safe builder: never touches the live filesystem, computes companion
/// pairing and volume metrics honestly from the snapshot metadata and stored moves.
pub fn analyze_volume_replay(
    disks: &[Disk],
    selected: &Disk,
    letter: &str,
    active_moves: &[Value],
) -> Value {
    let is_ssd = is_ssd_disk(selected);
    let (size_bytes, free_bytes) = volume_space_info(selected, letter, false);
    let fill_percent = if size_bytes > 0 {
        let raw = (size_bytes.saturating_sub(free_bytes)) as f64 / size_bytes as f64 * 100.0;
        (raw.clamp(0.0, 100.0) * 10.0).round() / 10.0
    } else {
        0.0
    };
    let (slc_pressure, slc_note_tr) = compute_slc_health(is_ssd, fill_percent);
    let (companion_role, companion) = find_companion(disks, selected, letter);
    json!({
        "disk_id": selected.id,
        "letter": letter,
        "disk_type": selected.kind,
        "is_ssd": is_ssd,
        "replay": true,
        "fill_percent": fill_percent,
        "free_bytes": free_bytes,
        "size_bytes": size_bytes,
        "slc_pressure": slc_pressure,
        "slc_note_tr": slc_note_tr,
        "extreme_fragmented_count": 0,
        "worst_extents": 0,
        "companion_role": companion_role,
        "companion": companion,
        "total_reclaimable_bytes": 0,
        "candidates": [],
        "active_moves": active_moves,
        "summary_tr": "Kayıt modunda canlı klasör taraması kapalıdır; yalnızca disk eşleştirme ve kayıtlı taşımalar gösterilir."
    })
}

/// Read-only volume analyzer on a live Windows volume letter.
pub fn analyze_volume(
    disks: &[Disk],
    selected: &Disk,
    letter: &str,
    active_moves: &[Value],
) -> Result<Value, String> {
    if letter.len() != 1 || !letter.as_bytes()[0].is_ascii_uppercase() {
        return Err("Geçersiz bölüm harfi.".into());
    }
    let root = platform::volume_root(letter)?;
    analyze_volume_root(&root, disks, selected, letter, active_moves, 10 * MIB)
}

/// Read-only analyzer operating on an explicit `root` directory (live drive root or
/// `tempfile::tempdir()` in tests). Checks forbidden paths strictly relative to `root`.
pub fn analyze_volume_root(
    root: &Path,
    disks: &[Disk],
    selected: &Disk,
    letter: &str,
    active_moves: &[Value],
    min_size_bytes: u64,
) -> Result<Value, String> {
    if !root.is_dir() {
        return Err(format!("Analiz kökü bulunamadı: {}", root.display()));
    }
    let is_ssd = is_ssd_disk(selected);
    let (size_bytes, free_bytes) = volume_space_info(selected, letter, true);
    let fill_percent = if size_bytes > 0 {
        let raw = (size_bytes.saturating_sub(free_bytes)) as f64 / size_bytes as f64 * 100.0;
        (raw.clamp(0.0, 100.0) * 10.0).round() / 10.0
    } else {
        0.0
    };
    let (slc_pressure, slc_note_tr) = compute_slc_health(is_ssd, fill_percent);
    let (companion_role, companion) = find_companion(disks, selected, letter);

    let active_sources: HashSet<String> = active_moves
        .iter()
        .filter_map(|m| m["source_path"].as_str())
        .map(|s| s.to_ascii_lowercase())
        .collect();

    let mut state = ScanState::new();
    let mut raw_candidates: Vec<(u32, u64, Value)> = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();

    let mut consider_item =
        |item_path: PathBuf,
         is_dir: bool,
         state: &mut ScanState,
         raw_candidates: &mut Vec<(u32, u64, Value)>| {
            if state.budget_exhausted() {
                return;
            }
            let lower_full = item_path.to_string_lossy().to_ascii_lowercase();
            if active_sources.contains(&lower_full) || !seen_paths.insert(lower_full) {
                return;
            }
            if is_existing_reparse(&item_path) {
                return;
            }
            let Ok(rel) = item_path.strip_prefix(root) else {
                return;
            };
            if has_forbidden_components(rel, Some(is_dir)) {
                return;
            }
            let (item_size, file_count, days_untouched) = if is_dir {
                rollup_directory(&item_path, root, state)
            } else {
                let Ok(meta) = std::fs::metadata(&item_path) else {
                    return;
                };
                let sz = meta.len();
                state.sample_extents(&item_path, sz);
                (sz, 1, days_since_touched(&meta, state.now))
            };
            let min_threshold = if is_dir {
                min_size_bytes.saturating_mul(2)
            } else {
                min_size_bytes
            };
            if item_size == 0 || item_size < min_threshold {
                return;
            }
            let Some(scored) = score_unchecked(rel, is_dir, item_size, days_untouched, is_ssd)
            else {
                return;
            };
            let score = if is_ssd {
                scored.cold_score
            } else {
                scored.hot_score
            };
            if score == 0 {
                return;
            }
            let name = item_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| item_path.to_string_lossy().into_owned());
            raw_candidates.push((
                score,
                item_size,
                json!({
                    "id": candidate_id(&item_path),
                    "name": name,
                    "path": item_path.to_string_lossy(),
                    "is_dir": is_dir,
                    "size_bytes": item_size,
                    "file_count": file_count,
                    "days_untouched": days_untouched,
                    "category": scored.category,
                    "score": score,
                    "reason_tr": scored.reason_tr,
                    "recommended_direction": scored.recommended_direction
                }),
            ));
        };

    // 1. Scan safe user profile locations: <root>\Users\<profile>\<Downloads|Videos|Documents|Desktop|Music|Pictures>
    let users_dir = root.join("Users");
    if users_dir.is_dir() && !is_existing_reparse(&users_dir) {
        if let Ok(profiles) = std::fs::read_dir(&users_dir) {
            for profile in profiles.filter_map(Result::ok) {
                if state.budget_exhausted() {
                    break;
                }
                let p_name = profile.file_name().to_string_lossy().to_ascii_lowercase();
                if matches!(
                    p_name.as_str(),
                    "default" | "default user" | "all users" | "defaultapppool"
                ) {
                    continue;
                }
                let p_path = profile.path();
                if !p_path.is_dir() || is_existing_reparse(&p_path) {
                    continue;
                }
                for sub in USER_SUBLOCATIONS {
                    if state.budget_exhausted() {
                        break;
                    }
                    let loc = p_path.join(sub);
                    if !loc.is_dir() || is_existing_reparse(&loc) {
                        continue;
                    }
                    if let Ok(children) = std::fs::read_dir(&loc) {
                        for child in children.filter_map(Result::ok) {
                            if state.budget_exhausted() {
                                break;
                            }
                            state.entries_visited += 1;
                            let cp = child.path();
                            let Ok(meta) = child.metadata() else {
                                continue;
                            };
                            if meta.is_dir() || meta.is_file() {
                                consider_item(cp, meta.is_dir(), &mut state, &mut raw_candidates);
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Scan non-system top-level folders (e.g. Games, Arsiv, Videos) and top-level large files.
    if let Ok(top_entries) = std::fs::read_dir(root) {
        for entry in top_entries.filter_map(Result::ok) {
            if state.budget_exhausted() {
                break;
            }
            state.entries_visited += 1;
            let p = entry.path();
            if is_existing_reparse(&p) {
                continue;
            }
            let name_lower = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name_lower == "users" {
                continue;
            }
            let Ok(rel) = p.strip_prefix(root) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if has_forbidden_components(rel, Some(meta.is_dir())) {
                continue;
            }
            if meta.is_dir() {
                if CONTAINER_DIR_NAMES.contains(&name_lower.as_str()) {
                    if let Ok(children) = std::fs::read_dir(&p) {
                        for child in children.filter_map(Result::ok) {
                            if state.budget_exhausted() {
                                break;
                            }
                            state.entries_visited += 1;
                            let cp = child.path();
                            let Ok(cmeta) = child.metadata() else {
                                continue;
                            };
                            if cmeta.is_dir() || cmeta.is_file() {
                                consider_item(cp, cmeta.is_dir(), &mut state, &mut raw_candidates);
                            }
                        }
                    }
                } else {
                    consider_item(p, true, &mut state, &mut raw_candidates);
                }
            } else if meta.is_file() {
                consider_item(p, false, &mut state, &mut raw_candidates);
            }
        }
    }

    raw_candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    raw_candidates.truncate(ANALYZE_TOP_CANDIDATES);

    let total_reclaimable_bytes: u64 = raw_candidates.iter().map(|(_, sz, _)| *sz).sum();
    let candidates: Vec<Value> = raw_candidates.into_iter().map(|(_, _, v)| v).collect();

    let summary_tr = if is_ssd {
        if candidates.is_empty() {
            if state.extreme_fragmented_count > 0 {
                format!(
                    "{letter}: üzerinde taşınacak büyük soğuk veri bulunmadı; ancak {} aşırı parçalanmış (>=50 parça) dosya tespit edildi.",
                    state.extreme_fragmented_count
                )
            } else {
                format!(
                    "{letter}: SSD bölümü dengeli görünüyor; öne çıkan büyük soğuk veri bulunmadı."
                )
            }
        } else {
            let gb = total_reclaimable_bytes as f64 / (GIB as f64);
            format!(
                "{letter}: üzerinde {} soğuk aday bulundu (toplam {gb:.2} GiB geri kazanılabilir); en yüksek parça sayısı: {}.",
                candidates.len(),
                state.worst_extents
            )
        }
    } else if candidates.is_empty() {
        format!("{letter}: HDD üzerinde terfi adayı sıcak klasör bulunmadı; HDD Dış-İz Hızlı Yerleşim çalıştırılabilir.")
    } else {
        let gb = total_reclaimable_bytes as f64 / (GIB as f64);
        format!(
            "{letter}: HDD üzerinde {} aktif sıcak aday bulundu (toplam {gb:.2} GiB).",
            candidates.len()
        )
    };

    Ok(json!({
        "disk_id": selected.id,
        "letter": letter,
        "disk_type": selected.kind,
        "is_ssd": is_ssd,
        "replay": false,
        "fill_percent": fill_percent,
        "free_bytes": free_bytes,
        "size_bytes": size_bytes,
        "slc_pressure": slc_pressure,
        "slc_note_tr": slc_note_tr,
        "extreme_fragmented_count": state.extreme_fragmented_count,
        "worst_extents": state.worst_extents,
        "companion_role": companion_role,
        "companion": companion,
        "total_reclaimable_bytes": total_reclaimable_bytes,
        "candidates": candidates,
        "active_moves": active_moves,
        "summary_tr": summary_tr
    }))
}

fn drive_letter_of(path: &Path) -> Option<String> {
    let norm = strip_verbatim(path);
    let s = norm.to_string_lossy();
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        Some((bytes[0].to_ascii_uppercase() as char).to_string())
    } else {
        None
    }
}

struct CopyProgress<'a> {
    total_work: u64,
    done_bytes: u64,
    read_bytes: u64,
    write_bytes: u64,
    last_reported: u64,
    on_progress: &'a mut dyn FnMut(u64, u64, u64, u64, &str),
}

impl<'a> CopyProgress<'a> {
    fn new(item_bytes: u64, on_progress: &'a mut dyn FnMut(u64, u64, u64, u64, &str)) -> Self {
        Self {
            total_work: item_bytes.saturating_mul(2).max(1),
            done_bytes: 0,
            read_bytes: 0,
            write_bytes: 0,
            last_reported: 0,
            on_progress,
        }
    }

    fn emit(&mut self, phase: &str, force: bool) {
        if force || self.done_bytes.saturating_sub(self.last_reported) >= 2 * MIB {
            self.last_reported = self.done_bytes;
            (self.on_progress)(
                self.done_bytes,
                self.total_work,
                self.read_bytes,
                self.write_bytes,
                phase,
            );
        }
    }
}

fn copy_file_verified_with(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    progress: &mut CopyProgress<'_>,
) -> Result<u64, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(process::CANCELLED.into());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Hedef klasör oluşturulamadı: {e}"))?;
    }
    let mtime = std::fs::metadata(src).ok().and_then(|m| m.modified().ok());
    let mut reader =
        File::open(src).map_err(|e| format!("Kaynak dosya okunamadı ({}): {e}", src.display()))?;
    let mut writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .map_err(|e| format!("Hedef dosya oluşturulamadı ({}): {e}", dst.display()))?;

    let file_name = src
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let copy_phase = format!("Kopyalanıyor · {file_name}");
    let verify_phase = format!("SHA-256 doğrulanıyor · {file_name}");

    let mut buf = vec![0u8; 256 * 1024];
    let mut src_hasher = Sha256::new();
    let mut src_total = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            drop(writer);
            drop(reader);
            let _ = std::fs::remove_file(dst);
            return Err(process::CANCELLED.into());
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                drop(writer);
                let _ = std::fs::remove_file(dst);
                return Err(format!("Kaynak okuma hatası: {e}"));
            }
        };
        src_hasher.update(&buf[..n]);
        if let Err(e) = writer.write_all(&buf[..n]) {
            drop(writer);
            let _ = std::fs::remove_file(dst);
            return Err(format!("Hedef yazma hatası: {e}"));
        }
        src_total = src_total.saturating_add(n as u64);
        progress.read_bytes = progress.read_bytes.saturating_add(n as u64);
        progress.write_bytes = progress.write_bytes.saturating_add(n as u64);
        progress.done_bytes = progress.done_bytes.saturating_add(n as u64);
        progress.emit(&copy_phase, false);
    }
    if let Err(e) = writer.sync_all() {
        drop(writer);
        let _ = std::fs::remove_file(dst);
        return Err(format!("Hedef dosya diske yazılamadı: {e}"));
    }
    if let Some(t) = mtime {
        let _ = writer.set_modified(t);
    }
    drop(writer);
    drop(reader);

    // Full read-back SHA-256 verification from destination storage.
    let mut verify_reader = File::open(dst).map_err(|e| {
        let _ = std::fs::remove_file(dst);
        format!("Doğrulama okuması başarısız: {e}")
    })?;
    let mut dst_hasher = Sha256::new();
    let mut dst_total = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            drop(verify_reader);
            let _ = std::fs::remove_file(dst);
            return Err(process::CANCELLED.into());
        }
        let n = match verify_reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                drop(verify_reader);
                let _ = std::fs::remove_file(dst);
                return Err(format!("Doğrulama okuma hatası: {e}"));
            }
        };
        dst_hasher.update(&buf[..n]);
        dst_total = dst_total.saturating_add(n as u64);
        progress.read_bytes = progress.read_bytes.saturating_add(n as u64);
        progress.done_bytes = progress.done_bytes.saturating_add(n as u64);
        progress.emit(&verify_phase, false);
    }
    drop(verify_reader);
    progress.emit(&verify_phase, true);

    if dst_total != src_total || src_hasher.finalize() != dst_hasher.finalize() {
        let _ = std::fs::remove_file(dst);
        return Err(format!(
            "SHA-256 doğrulama uyuşmazlığı ({}); kaynak değiştirilmeden işlem iptal edildi.",
            src.display()
        ));
    }
    Ok(src_total)
}

fn inspect_tree(src: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>, u64), String> {
    let mut subdirs = Vec::new();
    let mut files = Vec::new();
    let mut total_bytes = 0u64;
    let mut stack = vec![src.to_path_buf()];

    while let Some(cur) = stack.pop() {
        if platform::reparse(&cur) {
            return Err(
                "Klasör içinde sembolik bağ / junction bulunduğu için taşıma güvenlik nedeniyle reddedildi."
                    .into(),
            );
        }
        let entries = std::fs::read_dir(&cur)
            .map_err(|e| format!("Klasör okunamadı ({}): {e}", cur.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("Klasör girdisi okunamadı: {e}"))?;
            let p = entry.path();
            if platform::reparse(&p) {
                return Err(
                    "Klasör içinde sembolik bağ / junction bulunduğu için taşıma güvenlik nedeniyle reddedildi."
                        .into(),
                );
            }
            let rel = p
                .strip_prefix(src)
                .map_err(|e| e.to_string())?
                .to_path_buf();
            let meta = entry
                .metadata()
                .map_err(|e| format!("Öğe bilgisi okunamadı: {e}"))?;
            if meta.is_dir() {
                subdirs.push(rel);
                stack.push(p);
            } else if meta.is_file() {
                total_bytes = total_bytes.saturating_add(meta.len());
                files.push(rel);
            }
        }
    }
    Ok((subdirs, files, total_bytes))
}

fn copy_tree_verified_with(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, u64, u64, u64, &str),
) -> Result<(u64, u64), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(process::CANCELLED.into());
    }
    let (subdirs, files, total_bytes) = inspect_tree(src)?;
    std::fs::create_dir_all(dst).map_err(|e| format!("Hedef klasör oluşturulamadı: {e}"))?;
    for rel in &subdirs {
        if let Err(e) = std::fs::create_dir_all(dst.join(rel)) {
            let _ = std::fs::remove_dir_all(dst);
            return Err(format!("Alt klasör oluşturulamadı: {e}"));
        }
    }
    let mut progress = CopyProgress::new(total_bytes, on_progress);
    let mut copied_bytes = 0u64;
    for rel in &files {
        match copy_file_verified_with(&src.join(rel), &dst.join(rel), cancel, &mut progress) {
            Ok(n) => copied_bytes = copied_bytes.saturating_add(n),
            Err(e) => {
                let _ = std::fs::remove_dir_all(dst);
                return Err(e);
            }
        }
    }
    Ok((copied_bytes, files.len() as u64))
}

fn unique_target_path(desired: PathBuf) -> PathBuf {
    if !desired.exists() && !is_existing_reparse(&desired) {
        return desired;
    }
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let stem = desired
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".into());
    desired.with_file_name(format!("{stem}-{suffix}"))
}

fn staging_sibling_path(src: &Path) -> Result<PathBuf, String> {
    let parent = src.parent().ok_or("Kaynak üst klasörü belirlenemedi.")?;
    let name = src
        .file_name()
        .ok_or("Kaynak adı belirlenemedi.")?
        .to_string_lossy();
    let base = parent.join(format!("{name}.sentinel-moving"));
    if !base.exists() && !is_existing_reparse(&base) {
        return Ok(base);
    }
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
    Ok(parent.join(format!("{name}-{suffix}.sentinel-moving")))
}

/// Safe SHA-256 verified relocation across live drive letters (with 10 GiB free-space reserve
/// and pure Win32 `FSCTL_SET_REPARSE_POINT` NTFS Directory Junction for folders).
pub fn relocate_item(
    source_path: &Path,
    target_letter: &str,
    direction: &str,
) -> Result<Value, String> {
    let cancel = AtomicBool::new(false);
    relocate_item_with_progress(
        source_path,
        target_letter,
        direction,
        &cancel,
        &mut |_, _, _, _, _| {},
    )
}

pub fn relocate_item_with_progress(
    source_path: &Path,
    target_letter: &str,
    direction: &str,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, u64, u64, u64, &str),
) -> Result<Value, String> {
    if !matches!(direction, "to_hdd" | "to_ssd" | "to_archive") {
        return Err("Geçersiz katmanlama yönü (to_hdd, to_ssd veya to_archive olmalı).".into());
    }
    if target_letter.len() != 1 || !target_letter.as_bytes()[0].is_ascii_uppercase() {
        return Err("Geçersiz hedef bölüm harfi.".into());
    }
    if is_forbidden_path(source_path) {
        return Err("Sistem veya yasaklı yol katmanlama için reddedildi.".into());
    }
    let meta = std::fs::symlink_metadata(source_path)
        .map_err(|_| "Kaynak dosya veya klasör bulunamadı.".to_owned())?;
    let is_dir = meta.is_dir();
    if has_forbidden_components(source_path, Some(is_dir)) || platform::reparse(source_path) {
        return Err("Sistem klasörü veya reparse point taşınamaz.".into());
    }
    let source_letter =
        drive_letter_of(source_path).ok_or("Kaynak yol geçerli bir sürücü harfi içermiyor.")?;
    if source_letter.eq_ignore_ascii_case(target_letter) {
        return Err("Kaynak ve hedef sürücü harfi farklı olmalıdır.".into());
    }
    let target_root = platform::volume_root(target_letter)?;
    #[cfg(windows)]
    {
        if !win::is_volume_ntfs(&format!("{target_letter}:\\")) {
            return Err(format!(
                "{target_letter}: bölümü NTFS değil; katmanlama için NTFS gerekir."
            ));
        }
    }

    let rel_from_drive = source_path
        .strip_prefix(format!("{source_letter}:\\"))
        .or_else(|_| source_path.strip_prefix(format!("{}:\\", source_letter.to_ascii_lowercase())))
        .map_err(|_| "Kaynak yol sürücü kökünden ayrıştırılamadı.".to_owned())?;
    let archive_target = target_root
        .join(ARCHIVE_ROOT_NAME)
        .join(&source_letter)
        .join(rel_from_drive);
    let target_base = archive_target
        .parent()
        .ok_or("Hedef arşiv klasörü belirlenemedi.")?;

    relocate_core(
        source_path,
        target_base,
        Some(archive_target.clone()),
        &source_letter,
        target_letter,
        direction,
        is_dir,
        RELOCATE_FREE_RESERVE_BYTES,
        true,
        cancel,
        on_progress,
    )
}

/// Testable relocation helper that moves `source_path` into `target_base` with configurable
/// junction creation and free-space reserve (used by unit tests inside `tempfile::tempdir()`).
pub fn relocate_item_into_dir(
    source_path: &Path,
    target_base: &Path,
    direction: &str,
    create_junction: bool,
    min_reserve_bytes: u64,
) -> Result<Value, String> {
    let cancel = AtomicBool::new(false);
    relocate_item_into_dir_with_cancel(
        source_path,
        target_base,
        direction,
        create_junction,
        min_reserve_bytes,
        &cancel,
        &mut |_, _, _, _, _| {},
    )
}

pub fn relocate_item_into_dir_with_cancel(
    source_path: &Path,
    target_base: &Path,
    direction: &str,
    create_junction: bool,
    min_reserve_bytes: u64,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, u64, u64, u64, &str),
) -> Result<Value, String> {
    if !matches!(direction, "to_hdd" | "to_ssd" | "to_archive") {
        return Err("Geçersiz katmanlama yönü (to_hdd, to_ssd veya to_archive olmalı).".into());
    }
    let meta = std::fs::symlink_metadata(source_path)
        .map_err(|_| "Kaynak dosya veya klasör bulunamadı.".to_owned())?;
    let is_dir = meta.is_dir();
    if is_forbidden_in_test_sandbox(source_path, Some(is_dir)) || platform::reparse(source_path) {
        return Err("Sistem veya yasaklı yol katmanlama için reddedildi.".into());
    }
    let src_letter = drive_letter_of(source_path).unwrap_or_else(|| "C".into());
    let dst_letter = drive_letter_of(target_base).unwrap_or_else(|| "D".into());
    relocate_core(
        source_path,
        target_base,
        None,
        &src_letter,
        &dst_letter,
        direction,
        is_dir && create_junction,
        min_reserve_bytes,
        false,
        cancel,
        on_progress,
    )
}

#[allow(clippy::too_many_arguments)]
fn relocate_core(
    source_path: &Path,
    target_base: &Path,
    explicit_target: Option<PathBuf>,
    source_letter: &str,
    target_letter: &str,
    direction: &str,
    create_junction: bool,
    min_reserve_bytes: u64,
    strict_drive_check: bool,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, u64, u64, u64, &str),
) -> Result<Value, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(process::CANCELLED.into());
    }
    let meta = std::fs::symlink_metadata(source_path)
        .map_err(|_| "Kaynak dosya veya klasör bulunamadı.".to_owned())?;
    let is_dir = meta.is_dir();
    if !is_dir && !meta.is_file() {
        return Err("Yalnız normal dosya ve klasörler taşınabilir.".into());
    }

    let item_size = if is_dir {
        let (_, _, total) = inspect_tree(source_path)?;
        total
    } else {
        meta.len()
    };

    if min_reserve_bytes > 0 || strict_drive_check {
        #[cfg(windows)]
        {
            let check_path = if strict_drive_check {
                format!("{target_letter}:\\")
            } else {
                let mut probe = target_base;
                while !probe.exists() {
                    match probe.parent() {
                        Some(parent) => probe = parent,
                        None => break,
                    }
                }
                probe.to_string_lossy().into_owned()
            };
            if let Some((_, free_avail)) = win::volume_free_space(&check_path) {
                let required = item_size.saturating_add(min_reserve_bytes);
                if free_avail < required {
                    return Err(format!(
                        "Hedef sürücüde yeterli boş alan yok (gerekli: {:.2} GiB, mevcut: {:.2} GiB).",
                        required as f64 / (GIB as f64),
                        free_avail as f64 / (GIB as f64)
                    ));
                }
            } else if strict_drive_check {
                return Err("Hedef sürücünün boş alanı okunamadı.".into());
            }
        }
    }

    std::fs::create_dir_all(target_base)
        .map_err(|e| format!("Hedef temel klasör oluşturulamadı: {e}"))?;

    let desired_target = match explicit_target {
        Some(t) => t,
        None => {
            let name = source_path
                .file_name()
                .ok_or("Kaynak öğe adı belirlenemedi.")?;
            target_base.join(name)
        }
    };
    let target_path = unique_target_path(desired_target);

    if let (Ok(sc), Ok(tb)) = (source_path.canonicalize(), target_base.canonicalize()) {
        if tb.starts_with(&sc) {
            return Err("Klasör kendi alt klasörüne taşınamaz.".into());
        }
    }

    let mut cleanup_warning: Option<String> = None;
    let (copied_bytes, file_count, junction_created) = if is_dir {
        let (bytes, files) =
            copy_tree_verified_with(source_path, &target_path, cancel, on_progress)?;
        if cancel.load(Ordering::SeqCst) {
            let _ = std::fs::remove_dir_all(&target_path);
            return Err(process::CANCELLED.into());
        }
        let staging = staging_sibling_path(source_path)?;
        if let Err(e) = std::fs::rename(source_path, &staging) {
            let _ = std::fs::remove_dir_all(&target_path);
            return Err(format!(
                "Klasör şu anda başka bir program tarafından kullanılıyor; kaynak değiştirilmedi ({e})."
            ));
        }
        let junction_ok = if create_junction {
            match create_directory_junction(source_path, &target_path) {
                Ok(()) => true,
                Err(j_err) => {
                    // Roll back atomically: remove any partial dir at source_path, restore staging, delete target.
                    let _ = remove_junction_or_empty_dir(source_path);
                    let _ = std::fs::rename(&staging, source_path);
                    let _ = std::fs::remove_dir_all(&target_path);
                    return Err(format!(
                        "NTFS Junction oluşturulamadı; kaynak özgün yerine geri alındı ({j_err})."
                    ));
                }
            }
        } else {
            false
        };
        if let Err(e) = std::fs::remove_dir_all(&staging) {
            cleanup_warning = Some(format!(
                "Taşıma tamamlandı ancak geçici klasör ({}) kilitli olduğu için temizlenemedi; manuel silebilirsiniz: {e}",
                staging.display()
            ));
        }
        (bytes, files, junction_ok)
    } else {
        let mut progress = CopyProgress::new(item_size, on_progress);
        let bytes = copy_file_verified_with(source_path, &target_path, cancel, &mut progress)?;
        if cancel.load(Ordering::SeqCst) {
            let _ = std::fs::remove_file(&target_path);
            return Err(process::CANCELLED.into());
        }
        if let Err(e) = std::fs::remove_file(source_path) {
            let _ = std::fs::remove_file(&target_path);
            return Err(format!(
                "Kaynak dosya kilitli olduğu için silinemedi; hedef kopya temizlendi ({e})."
            ));
        }
        (bytes, 1, false)
    };

    Ok(json!({
        "id": uuid::Uuid::new_v4().simple().to_string(),
        "source_path": source_path.to_string_lossy(),
        "target_path": target_path.to_string_lossy(),
        "source_letter": source_letter,
        "target_letter": target_letter,
        "is_dir": is_dir,
        "junction_created": junction_created,
        "size_bytes": copied_bytes,
        "file_count": file_count,
        "direction": direction,
        "moved_at": Utc::now().to_rfc3339(),
        "cleanup_warning": cleanup_warning
    }))
}

fn create_directory_junction(junction_path: &Path, target_path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::create_ntfs_junction(junction_path, target_path)
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target_path, junction_path)
            .map_err(|e| format!("Sembolik bağ oluşturulamadı: {e}"))?;
        if !is_existing_reparse(junction_path) {
            let _ = std::fs::remove_file(junction_path);
            return Err("Sembolik bağ doğrulanamadı.".into());
        }
        Ok(())
    }
}

fn remove_junction_only(path: &Path) -> Result<(), String> {
    if !is_existing_reparse(path) {
        return Err("Hedef yol bir NTFS Junction / reparse point değil; silme reddedildi.".into());
    }
    #[cfg(windows)]
    {
        win::remove_directory_reparse(path)
    }
    #[cfg(not(windows))]
    {
        std::fs::remove_file(path).map_err(|e| format!("Sembolik bağ kaldırılamadı: {e}"))
    }
}

fn remove_junction_or_empty_dir(path: &Path) -> Result<(), String> {
    if is_existing_reparse(path) {
        remove_junction_only(path)
    } else if path.exists() {
        std::fs::remove_dir(path).map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

/// 1-click SHA-256 verified rollback of a previously relocated folder (junction or plain) or file.
pub fn rollback_move(rec: &Value) -> Result<Value, String> {
    let cancel = AtomicBool::new(false);
    rollback_move_with_progress(rec, &cancel, &mut |_, _, _, _, _| {})
}

pub fn rollback_move_with_progress(
    rec: &Value,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, u64, u64, u64, &str),
) -> Result<Value, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(process::CANCELLED.into());
    }
    let id = rec["id"].as_str().unwrap_or("").to_owned();
    let source_str = rec["source_path"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Taşıma kaydında kaynak yol eksik.")?;
    let target_str = rec["target_path"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Taşıma kaydında hedef yol eksik.")?;
    let source_path = Path::new(source_str);
    let target_path = Path::new(target_str);

    if source_path
        .components()
        .any(|c| matches!(c, Component::ParentDir))
        || target_path
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("Geçersiz geri alma yolu.".into());
    }
    if !target_path.exists() || platform::reparse(target_path) {
        return Err("Taşınmış hedef öğe diskte bulunamadı veya reparse point.".into());
    }

    let is_dir = rec["is_dir"].as_bool().unwrap_or(target_path.is_dir());
    let junction_created = rec["junction_created"].as_bool().unwrap_or(false);
    let mut cleanup_warning: Option<String> = None;

    let (restored_bytes, restored_files) = if is_dir {
        let removed_junction = if junction_created {
            if std::fs::symlink_metadata(source_path).is_ok() {
                if !is_existing_reparse(source_path) {
                    return Err(
                        "Özgün konumda gerçek bir klasör bulunuyor; veri kaybını önlemek için geri alma durduruldu."
                            .into(),
                    );
                }
                remove_junction_only(source_path)?;
                true
            } else {
                false
            }
        } else {
            if std::fs::symlink_metadata(source_path).is_ok() {
                return Err("Özgün konumda zaten bir öğe var; geri alma reddedildi.".into());
            }
            false
        };

        match copy_tree_verified_with(target_path, source_path, cancel, on_progress) {
            Ok((bytes, files)) => {
                if let Err(e) = std::fs::remove_dir_all(target_path) {
                    cleanup_warning = Some(format!(
                        "Öğe geri alındı ancak arşiv kopyası ({}) temizlenemedi: {e}",
                        target_path.display()
                    ));
                }
                (bytes, files)
            }
            Err(err) => {
                let _ = std::fs::remove_dir_all(source_path);
                if removed_junction || junction_created {
                    let _ = create_directory_junction(source_path, target_path);
                }
                if err == process::CANCELLED {
                    return Err(err);
                }
                return Err(format!(
                    "Geri kopyalama başarısız; hedef veri ve junction korundu ({err})."
                ));
            }
        }
    } else {
        if std::fs::symlink_metadata(source_path).is_ok() {
            return Err("Özgün konumda zaten bir dosya var; geri alma reddedildi.".into());
        }
        let item_size = std::fs::metadata(target_path).map(|m| m.len()).unwrap_or(0);
        let mut progress = CopyProgress::new(item_size, on_progress);
        let bytes = copy_file_verified_with(target_path, source_path, cancel, &mut progress)?;
        if let Err(e) = std::fs::remove_file(target_path) {
            cleanup_warning = Some(format!(
                "Dosya geri alındı ancak arşiv kopyası ({}) temizlenemedi: {e}",
                target_path.display()
            ));
        }
        (bytes, 1)
    };

    Ok(json!({
        "rolled_back": true,
        "id": id,
        "source_path": source_str,
        "target_path": target_str,
        "size_bytes": restored_bytes,
        "file_count": restored_files,
        "cleanup_warning": cleanup_warning,
        "message_tr": "Öğe SHA-256 doğrulamasıyla özgün konumuna geri alındı.",
        "message": "Item restored to its original path with SHA-256 verification."
    }))
}

/// Executes `kind == "smart_tier"` background optimization job:
/// - `ssd_extent_optimize`: Consolidates ONLY extreme-fragmented (`>= 50` extents) NTFS files
///   up to a strict 2 GiB write budget cap, then runs SSD ReTrim (`defrag /L /V`). NEVER runs classic defrag!
/// - `hdd_outer_optimize`: Enforces `autopilot_hdd_defrag_guard(disk)` first, then packs
///   fragmented (`>= 2` extents) & hot user/game/app files contiguously starting from LCN 0
///   (fastest outer ZBR tracks, up to 4 GiB per session).
pub fn run_smart_tier_job(
    core: &Core,
    request: &JobRequest,
    disk: &Disk,
    root: &Path,
) -> Result<(), String> {
    let mode = request.options["mode"].as_str().unwrap_or("");
    match mode {
        "ssd_extent_optimize" => {
            if disk.kind == "HDD" {
                return Err(
                    core.t("SSD Akıllı Parça Temizliği yalnız SSD sürücülerde çalışır; HDD için Dış-İz Hızlı Yerleşim kullanın.", "Smart Extent Consolidation works on SSD drives only; use Outer-Track Placement for HDDs.")
                        .into(),
                );
            }
            if !platform::is_admin() {
                return Err(core
                    .t(
                        "Akıllı depolama optimizasyonu için Yönetici erişimi gereklidir.",
                        "Administrator access is required for smart storage optimization.",
                    )
                    .into());
            }
            #[cfg(windows)]
            {
                win::run_ssd_extent_optimize(core, request, root)
            }
            #[cfg(not(windows))]
            {
                let _ = (core, request, root);
                Err(core
                    .t(
                        "SSD Akıllı Parça Temizliği yalnız Windows NTFS'te desteklenir.",
                        "SSD Smart Extent Consolidation is supported on Windows NTFS only.",
                    )
                    .into())
            }
        }
        "hdd_outer_optimize" => {
            if disk.kind != "HDD" {
                return Err(
                    core.t("HDD Dış-İz Hızlı Yerleşim yalnız mekanik HDD'lerde çalışır; SSD için Akıllı Parça Temizliği kullanın.", "HDD Outer-Track Placement works on mechanical HDDs only; use Smart Extent Consolidation for SSDs.")
                        .into(),
                );
            }
            if let Some(warning) = autopilot_hdd_defrag_guard_for_lang(disk, core.is_en()) {
                return Err(warning);
            }
            if !platform::is_admin() {
                return Err(core
                    .t(
                        "Akıllı depolama optimizasyonu için Yönetici erişimi gereklidir.",
                        "Administrator access is required for smart storage optimization.",
                    )
                    .into());
            }
            #[cfg(windows)]
            {
                win::run_hdd_outer_optimize(core, request, root)
            }
            #[cfg(not(windows))]
            {
                let _ = (core, request, root);
                Err(core
                    .t(
                        "HDD Dış-İz Hızlı Yerleşim yalnız Windows NTFS'te desteklenir.",
                        "HDD Outer-Track Placement is supported on Windows NTFS only.",
                    )
                    .into())
            }
        }
        _ => Err(core
            .t(
                "Geçersiz akıllı katmanlama optimizasyon modu.",
                "Invalid smart tiering optimization mode.",
            )
            .into()),
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_MORE_DATA, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetVolumeInformationW,
        RemoveDirectoryW, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const FSCTL_GET_VOLUME_BITMAP: u32 = 0x0009_006F;
    const FSCTL_GET_RETRIEVAL_POINTERS: u32 = 0x0009_0073;
    const FSCTL_MOVE_FILE: u32 = 0x0009_0074;
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00A4;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const SYNCHRONIZE: u32 = 0x0010_0000;

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
    struct VolumeBitmapHeader {
        starting_lcn: i64,
        bitmap_size: i64,
    }

    #[repr(C)]
    struct MoveFileData {
        file_handle: HANDLE,
        starting_vcn: i64,
        starting_lcn: i64,
        cluster_count: u32,
        _pad: u32,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn volume_free_space(path: &str) -> Option<(u64, u64)> {
        let w = wide(path);
        let mut free_avail = 0u64;
        let mut total = 0u64;
        let mut total_free = 0u64;
        let ok = unsafe {
            GetDiskFreeSpaceExW(w.as_ptr(), &mut free_avail, &mut total, &mut total_free)
        };
        (ok != 0).then_some((total, free_avail))
    }

    pub fn is_volume_ntfs(root: &str) -> bool {
        let w = wide(root);
        let mut fs_buf = [0u16; 64];
        let ok = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                null_mut(),
                fs_buf.as_mut_ptr(),
                fs_buf.len() as u32,
            )
        };
        if ok == 0 {
            return false;
        }
        let len = fs_buf.iter().position(|&c| c == 0).unwrap_or(fs_buf.len());
        String::from_utf16_lossy(&fs_buf[..len]).eq_ignore_ascii_case("NTFS")
    }

    fn cluster_bytes(letter: &str) -> u64 {
        let root_w = wide(&format!("{letter}:\\"));
        let mut spc = 0u32;
        let mut bps = 0u32;
        let mut fc = 0u32;
        let mut tc = 0u32;
        let ok =
            unsafe { GetDiskFreeSpaceW(root_w.as_ptr(), &mut spc, &mut bps, &mut fc, &mut tc) };
        if ok != 0 && spc > 0 && bps > 0 {
            u64::from(spc) * u64::from(bps)
        } else {
            4096
        }
    }

    fn open_file_for_extents(path: &Path) -> Option<WinHandle> {
        let w = wide(&path.to_string_lossy());
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                null_mut(),
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(WinHandle(h))
        }
    }

    fn open_volume_rw(letter: &str) -> Result<WinHandle, String> {
        let w = wide(&format!("\\\\.\\{letter}:"));
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Err(format!(
                "{letter}: bölümü açılamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(WinHandle(h))
    }

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
            let more = ok == 0
                && std::io::Error::last_os_error().raw_os_error() == Some(ERROR_MORE_DATA as i32);
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

    pub fn query_path_extent_count(path: &Path) -> Option<usize> {
        let fh = open_file_for_extents(path)?;
        let extents = file_extents_all(fh.0);
        (!extents.is_empty()).then_some(extents.len())
    }

    fn find_free_run_from(vol: HANDLE, need: u64, from: u64, limit_lcn: u64) -> Option<u64> {
        if need == 0 {
            return None;
        }
        let hdr_sz = std::mem::size_of::<VolumeBitmapHeader>();
        let mut out = vec![0u8; hdr_sz + 1024 * 1024];
        let mut pos = (from / 8 * 8) as i64;
        let mut run_start = 0u64;
        let mut run_len = 0u64;
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
            let more = ok == 0
                && std::io::Error::last_os_error().raw_os_error() == Some(ERROR_MORE_DATA as i32);
            if (ok == 0 && !more) || (ret as usize) <= hdr_sz {
                return None;
            }
            let hdr: VolumeBitmapHeader = unsafe { std::ptr::read_unaligned(out.as_ptr().cast()) };
            let bm = &out[hdr_sz..ret as usize];
            let here = (hdr.bitmap_size.max(0) as u64).min(bm.len() as u64 * 8);
            let mut b = 0u64;
            while b < here {
                let lcn = hdr.starting_lcn as u64 + b;
                if lcn >= limit_lcn {
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
                        return Some(run_start);
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

    /// Creates a transparent NTFS Directory Junction (`IO_REPARSE_TAG_MOUNT_POINT = 0xA0000003`)
    /// using pure Win32 `DeviceIoControl(FSCTL_SET_REPARSE_POINT)` with zero shell execution.
    pub fn create_ntfs_junction(junction_path: &Path, target_path: &Path) -> Result<(), String> {
        let canon_target = target_path
            .canonicalize()
            .map_err(|e| format!("Hedef yol çözümlenemedi: {e}"))?;
        let canon_str = canon_target.to_string_lossy();
        let print_name = canon_str
            .strip_prefix("\\\\?\\")
            .unwrap_or(&canon_str)
            .trim_end_matches('\\')
            .to_owned();
        if print_name.len() < 3 || print_name.starts_with("UNC\\") {
            return Err("NTFS Junction yalnız yerel sabit disk hedeflerini destekler.".into());
        }
        let substitute_name = format!("\\??\\{print_name}");
        let sub_u16: Vec<u16> = substitute_name.encode_utf16().collect();
        let print_u16: Vec<u16> = print_name.encode_utf16().collect();
        let sub_byte_len = u16::try_from(sub_u16.len() * 2)
            .map_err(|_| "NTFS Junction yol uzunluğu sınırı aşıldı.")?;
        let print_byte_len = u16::try_from(print_u16.len() * 2)
            .map_err(|_| "NTFS Junction yol uzunluğu sınırı aşıldı.")?;
        let sub_offset: u16 = 0;
        let print_offset: u16 = sub_byte_len
            .checked_add(2)
            .ok_or("NTFS Junction yol uzunluğu taşması.")?;
        let path_buf_len: u16 = print_offset
            .checked_add(print_byte_len)
            .and_then(|v| v.checked_add(2))
            .ok_or("NTFS Junction arabellek taşması.")?;
        let reparse_data_length: u16 = path_buf_len
            .checked_add(8)
            .ok_or("NTFS Junction başlık taşması.")?;
        if usize::from(reparse_data_length) + 8 > 16 * 1024 {
            return Err("NTFS Junction verisi 16 KiB sınırını aşıyor.".into());
        }

        let mut buf = Vec::with_capacity(8 + usize::from(reparse_data_length));
        buf.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        buf.extend_from_slice(&reparse_data_length.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&sub_offset.to_le_bytes());
        buf.extend_from_slice(&sub_byte_len.to_le_bytes());
        buf.extend_from_slice(&print_offset.to_le_bytes());
        buf.extend_from_slice(&print_byte_len.to_le_bytes());
        for unit in &sub_u16 {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
        buf.extend_from_slice(&0u16.to_le_bytes());
        for unit in &print_u16 {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
        buf.extend_from_slice(&0u16.to_le_bytes());

        std::fs::create_dir(junction_path)
            .map_err(|e| format!("Junction klasör girişi oluşturulamadı: {e}"))?;

        let j_wide = wide(&junction_path.to_string_lossy());
        let raw_h = unsafe {
            CreateFileW(
                j_wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if raw_h.is_null() || raw_h == INVALID_HANDLE_VALUE {
            let err = std::io::Error::last_os_error();
            let _ = std::fs::remove_dir(junction_path);
            return Err(format!("Junction tutamacı açılamadı: {err}"));
        }
        let handle = WinHandle(raw_h);
        let mut bytes_ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                handle.0,
                FSCTL_SET_REPARSE_POINT,
                buf.as_ptr().cast(),
                buf.len() as u32,
                null_mut(),
                0,
                &mut bytes_ret,
                null_mut(),
            )
        };
        drop(handle);
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            let _ = std::fs::remove_dir(junction_path);
            return Err(format!("FSCTL_SET_REPARSE_POINT başarısız: {err}"));
        }

        // Verify reparse attribute AND target resolution.
        if !is_existing_reparse(junction_path)
            || !same_file::is_same_file(junction_path, target_path).unwrap_or(false)
            || std::fs::read_dir(junction_path).is_err()
        {
            let _ = remove_directory_reparse(junction_path);
            return Err("Oluşturulan NTFS Junction hedefe çözümlenemedi.".into());
        }
        Ok(())
    }

    pub fn remove_directory_reparse(path: &Path) -> Result<(), String> {
        let w = wide(&path.to_string_lossy());
        let ok = unsafe { RemoveDirectoryW(w.as_ptr()) };
        if ok == 0 {
            return Err(format!(
                "NTFS Junction kaldırılamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    fn move_file_contiguously(
        core: &Core,
        vol: HANDLE,
        fh: HANDLE,
        extents: &[(i64, i64, i64)],
        target_lcn: u64,
    ) -> Result<u64, String> {
        let mut moved_clusters = 0u64;
        let mut dst_lcn = target_lcn;
        'outer: for &(ext_vcn, _ext_lcn, ext_len) in extents {
            let mut off = 0i64;
            while off < ext_len {
                cancelled(core)?;
                let chunk = (ext_len - off).min(4096) as u32;
                let mv = MoveFileData {
                    file_handle: fh,
                    starting_vcn: ext_vcn + off,
                    starting_lcn: dst_lcn as i64,
                    cluster_count: chunk,
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
                    break 'outer;
                }
                moved_clusters = moved_clusters.saturating_add(u64::from(chunk));
                dst_lcn = dst_lcn.saturating_add(u64::from(chunk));
                off += i64::from(chunk);
            }
        }
        Ok(moved_clusters)
    }

    fn collect_safe_user_files(
        core: &Core,
        root: &Path,
        min_bytes: u64,
        max_bytes: u64,
        max_entries: usize,
    ) -> Result<Vec<(PathBuf, u64, u64)>, String> {
        let now = SystemTime::now();
        let mut out = Vec::new();
        let mut dirs = vec![root.to_path_buf()];
        let mut visited = 0usize;

        while let Some(dir) = dirs.pop() {
            cancelled(core)?;
            if visited >= max_entries {
                break;
            }
            if is_existing_reparse(&dir) {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in rd.filter_map(Result::ok) {
                cancelled(core)?;
                visited += 1;
                if visited >= max_entries {
                    break;
                }
                let p = entry.path();
                if is_existing_reparse(&p) {
                    continue;
                }
                let Ok(rel) = p.strip_prefix(root) else {
                    continue;
                };
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if has_forbidden_components(rel, Some(meta.is_dir())) {
                    continue;
                }
                if meta.is_dir() {
                    if dirs.len() < 2048 {
                        dirs.push(p);
                    }
                } else if meta.is_file() {
                    let sz = meta.len();
                    if sz >= min_bytes && sz <= max_bytes {
                        let days = days_since_touched(&meta, now);
                        out.push((p, sz, days));
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn run_ssd_extent_optimize(
        core: &Core,
        request: &JobRequest,
        root: &Path,
    ) -> Result<(), String> {
        let vol = open_volume_rw(&request.letter)?;
        let cluster = cluster_bytes(&request.letter);
        lock(&core.job)["total"] = json!(100);
        phase_bi(
            core,
            "1/3 · Aşırı parçalanmış (>=50 parça) SSD dosyaları taranıyor",
            "1/3 · Scanning heavily fragmented (>=50 extents) SSD files",
        );

        let files = collect_safe_user_files(core, root, 64 * 1024, 512 * MIB, 25_000)?;
        let mut targets: Vec<(PathBuf, usize, u64)> = Vec::new();
        for (idx, (path, _, _)) in files.iter().enumerate() {
            if idx.is_multiple_of(64) {
                cancelled(core)?;
                let pct = ((idx as u64) * 30 / files.len().max(1) as u64).min(30);
                let mut job = lock(&core.job);
                job["completed"] = json!(pct);
                job["progress"] = json!(pct);
            }
            let Some(fh) = open_file_for_extents(path) else {
                continue;
            };
            let extents = file_extents_all(fh.0);
            if extents.len() >= SSD_EXTENT_THRESHOLD {
                let clusters: u64 = extents.iter().map(|&(_, _, n)| n as u64).sum();
                if clusters > 0 {
                    targets.push((path.clone(), extents.len(), clusters));
                }
            }
        }
        targets.sort_by_key(|a| std::cmp::Reverse(a.1));

        phase_bi(
            core,
            "2/3 · Aşırı parçalanmış SSD dosyaları birleştiriliyor (en fazla 2 GiB yazma sınırı)",
            "2/3 · Consolidating heavily fragmented SSD files (2 GiB write cap)",
        );
        let mut files_consolidated = 0u64;
        let mut extents_before = 0u64;
        let mut extents_after = 0u64;
        let mut bytes_written = 0u64;
        let mut free_hint = 0u64;
        let target_total = targets.len().max(1) as u64;

        for (i, (path, _, _)) in targets.iter().enumerate() {
            cancelled(core)?;
            if bytes_written >= SSD_WRITE_BUDGET_BYTES {
                break;
            }
            let pct = 30 + ((i as u64) * 45 / target_total).min(45);
            {
                let mut job = lock(&core.job);
                job["completed"] = json!(pct);
                job["progress"] = json!(pct);
                job["write_bytes"] = json!(bytes_written);
            }
            let Some(fh) = open_file_for_extents(path) else {
                continue;
            };
            let extents = file_extents_all(fh.0);
            if extents.len() < SSD_EXTENT_THRESHOLD {
                continue;
            }
            let need_clusters: u64 = extents.iter().map(|&(_, _, n)| n as u64).sum();
            let file_bytes = need_clusters.saturating_mul(cluster);
            if need_clusters == 0
                || bytes_written.saturating_add(file_bytes) > SSD_WRITE_BUDGET_BYTES
            {
                continue;
            }
            let Some(dst_lcn) = find_free_run_from(vol.0, need_clusters, free_hint, u64::MAX)
                .or_else(|| find_free_run_from(vol.0, need_clusters, 0, free_hint))
            else {
                continue;
            };
            let before = extents.len() as u64;
            let moved_clusters = move_file_contiguously(core, vol.0, fh.0, &extents, dst_lcn)?;
            if moved_clusters > 0 {
                let after = file_extents_all(fh.0).len() as u64;
                files_consolidated += 1;
                extents_before += before;
                extents_after += after;
                bytes_written =
                    bytes_written.saturating_add(moved_clusters.saturating_mul(cluster));
                free_hint = dst_lcn.saturating_add(need_clusters);
            }
        }
        drop(vol);

        cancelled(core)?;
        phase_bi(
            core,
            "3/3 · SSD ReTrim (defrag /L) uygulanıyor",
            "3/3 · Applying SSD ReTrim (defrag /L)",
        );
        {
            let mut job = lock(&core.job);
            job["completed"] = json!(80);
            job["progress"] = json!(80);
            job["write_bytes"] = json!(bytes_written);
        }
        let retrim_res = process::run(
            &platform::system_executable("defrag.exe"),
            &[format!("{}:", request.letter), "/L".into(), "/V".into()],
            Duration::from_secs(600),
            &core.job_cancel,
        );
        let retrim_ok = match retrim_res {
            Ok(out) => out.code == 0,
            Err(e) if e == process::CANCELLED => return Err(e),
            Err(_) => false,
        };

        let mut job = lock(&core.job);
        job["completed"] = json!(100);
        job["progress"] = json!(100);
        job["write_bytes"] = json!(bytes_written);
        job["repair_summary"] = json!({
            "mode": "ssd_extent_optimize",
            "files_consolidated": files_consolidated,
            "extents_before": extents_before,
            "extents_after": extents_after,
            "bytes_written": bytes_written,
            "retrim_ok": retrim_ok,
            "scope": if core.is_en() { "SSD Smart Extent Consolidation: only user files with >=50 extents were consolidated (2 GiB write cap) and ReTrim (/L) was applied; no classic defrag was run." } else { "SSD Akıllı Parça Temizliği: Yalnız >=50 parçalı kullanıcı dosyaları (en fazla 2 GiB yazma sınırı) birleştirildi ve ReTrim (/L) uygulandı; klasik defrag yapılmadı." }
        });
        Ok(())
    }

    pub fn run_hdd_outer_optimize(
        core: &Core,
        request: &JobRequest,
        root: &Path,
    ) -> Result<(), String> {
        let vol = open_volume_rw(&request.letter)?;
        let cluster = cluster_bytes(&request.letter);
        lock(&core.job)["total"] = json!(100);
        phase_bi(
            core,
            "1/3 · Sıcak ve parçalanmış HDD dosyaları taranıyor",
            "1/3 · Scanning hot and fragmented HDD files",
        );

        let files = collect_safe_user_files(core, root, 64 * 1024, GIB, 25_000)?;
        // (priority, path, before_extents, first_lcn)
        let mut candidates: Vec<(u32, PathBuf, usize, u64)> = Vec::new();
        for (idx, (path, _, days)) in files.iter().enumerate() {
            if idx.is_multiple_of(64) {
                cancelled(core)?;
                let pct = ((idx as u64) * 30 / files.len().max(1) as u64).min(30);
                let mut job = lock(&core.job);
                job["completed"] = json!(pct);
                job["progress"] = json!(pct);
            }
            let Some(fh) = open_file_for_extents(path) else {
                continue;
            };
            let extents = file_extents_all(fh.0);
            if extents.is_empty() {
                continue;
            }
            let first_lcn = extents[0].1.max(0) as u64;
            let is_fragmented = extents.len() >= 2;
            let is_hot = *days <= 14;
            if !is_fragmented && !is_hot {
                continue;
            }
            let mut priority = 0u32;
            if is_hot {
                priority += 60u32.saturating_sub((*days as u32).saturating_mul(3));
            }
            if is_fragmented {
                priority += (extents.len() as u32).min(40);
            }
            candidates.push((priority, path.clone(), extents.len(), first_lcn));
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.2.cmp(&a.2)));

        phase_bi(
            core,
            "2/3 · Sıcak ve parçalanmış dosyalar en hızlı dış izlere (LCN 0+) yerleştiriliyor",
            "2/3 · Placing hot and fragmented files on the fastest outer tracks (LCN 0+)",
        );
        let mut files_placed_outer = 0u64;
        let mut extents_before = 0u64;
        let mut extents_after = 0u64;
        let mut bytes_moved = 0u64;
        let mut outer_cursor = 0u64;
        let total = candidates.len().max(1) as u64;

        for (i, (_, path, _, _)) in candidates.iter().enumerate() {
            cancelled(core)?;
            if bytes_moved >= HDD_OUTER_MOVE_BUDGET_BYTES {
                break;
            }
            let pct = 30 + ((i as u64) * 65 / total).min(65);
            {
                let mut job = lock(&core.job);
                job["completed"] = json!(pct);
                job["progress"] = json!(pct);
                job["write_bytes"] = json!(bytes_moved);
            }
            let Some(fh) = open_file_for_extents(path) else {
                continue;
            };
            let extents = file_extents_all(fh.0);
            if extents.is_empty() {
                continue;
            }
            let before = extents.len() as u64;
            let current_first_lcn = extents[0].1.max(0) as u64;
            let need_clusters: u64 = extents.iter().map(|&(_, _, n)| n as u64).sum();
            let file_bytes = need_clusters.saturating_mul(cluster);
            if need_clusters == 0
                || bytes_moved.saturating_add(file_bytes) > HDD_OUTER_MOVE_BUDGET_BYTES
            {
                continue;
            }
            // If already contiguous, only move strictly outward (target_lcn < current_first_lcn).
            let limit = if before == 1 {
                current_first_lcn
            } else {
                u64::MAX
            };
            let Some(target_lcn) = find_free_run_from(vol.0, need_clusters, outer_cursor, limit)
            else {
                continue;
            };
            let moved_clusters = move_file_contiguously(core, vol.0, fh.0, &extents, target_lcn)?;
            if moved_clusters > 0 {
                let after = file_extents_all(fh.0).len() as u64;
                files_placed_outer += 1;
                extents_before += before;
                extents_after += after;
                bytes_moved = bytes_moved.saturating_add(moved_clusters.saturating_mul(cluster));
                outer_cursor = target_lcn.saturating_add(need_clusters);
            }
        }
        drop(vol);

        phase_bi(
            core,
            "3/3 · HDD dış-iz yerleşim özeti kaydediliyor",
            "3/3 · Saving the HDD outer-track placement summary",
        );
        let mut job = lock(&core.job);
        job["completed"] = json!(100);
        job["progress"] = json!(100);
        job["write_bytes"] = json!(bytes_moved);
        job["repair_summary"] = json!({
            "mode": "hdd_outer_optimize",
            "files_placed_outer": files_placed_outer,
            "extents_before": extents_before,
            "extents_after": extents_after,
            "bytes_moved": bytes_moved,
            "scope": if core.is_en() { "HDD Outer-Track Placement: SMART safety guard verified; hot and fragmented files were placed contiguously on the fastest outer ZBR tracks (LCN 0+, up to 4 GiB)." } else { "HDD Dış-İz Hızlı Yerleşim: SMART güvenlik koruması doğrulandı; sıcak ve parçalanmış dosyalar en hızlı dış ZBR izlerine (LCN 0+, en fazla 4 GiB) kesintisiz yerleştirildi." }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Health;

    fn mock_disk(id: &str, kind: &str, model: &str, volumes: Vec<Value>) -> Disk {
        Disk {
            id: id.into(),
            stable_identity: true,
            device: "/dev/sda".into(),
            device_type: "sat".into(),
            model: model.into(),
            serial: Some("SER123".into()),
            firmware: None,
            kind: kind.into(),
            protocol: Some("ATA".into()),
            metrics: json!({}),
            health: Health {
                percent: Some(98),
                status: "ok".into(),
                method: None,
                explanation: String::new(),
            },
            smart: Vec::new(),
            alerts: Vec::new(),
            capabilities: json!({}),
            interface: None,
            logical_block_size: Some(512),
            physical_block_size: Some(4096),
            rotation_rate: Some(if kind == "HDD" { 7200 } else { 0 }),
            nvme_eui64: Vec::new(),
            nvme_nguid: Vec::new(),
            wwn: None,
            source_time: None,
            os_disk_number: Some(0),
            os_serial: Some("SER123".into()),
            os_model: Some(model.into()),
            volumes,
            bus_link: Value::Null,
            optimization: Value::Null,
            read_incomplete: false,
            read_warning: None,
        }
    }

    #[test]
    fn forbidden_paths_are_strictly_rejected() {
        let forbidden = [
            "C:\\Windows\\System32\\cmd.exe",
            "C:\\Program Files\\App\\app.exe",
            "C:\\Program Files (x86)\\Tool\\tool.bin",
            "C:\\ProgramData\\Cache\\big.dat",
            "C:\\Users\\ali\\AppData\\Local\\cache.bin",
            "C:\\$Recycle.Bin\\S-1-5\\file.mkv",
            "C:\\System Volume Information\\snap.bin",
            "C:\\Recovery\\WindowsRE\\winre.wim",
            "C:\\PerfLogs\\trace.etl",
            "C:\\MSOCache\\pkg.cab",
            "C:\\Config.Msi\\a.rbf",
            "D:\\SSD-Sentinel-Arsiv\\C\\Videos\\movie.mkv",
            "C:\\.ssd-sentinel-quarantine\\q.bin",
            "C:\\pagefile.sys",
            "C:\\hiberfil.sys",
            "C:\\swapfile.sys",
            "C:\\Users\\ali\\..\\Windows\\foo.bin",
            "C:\\",
            "C:\\Users",
        ];
        for p in forbidden {
            assert!(
                is_forbidden_path(Path::new(p)),
                "Expected forbidden path to be rejected: {p}"
            );
            assert!(
                score_tier_candidate(Path::new(p), false, 200 * MIB, 120, true).is_none(),
                "score_tier_candidate must return None for forbidden path: {p}"
            );
        }
        // Moving an entire user profile directory is also rejected.
        assert!(
            score_tier_candidate(Path::new("C:\\Users\\ali"), true, 2 * GIB, 120, true).is_none()
        );
    }

    #[test]
    fn cold_and_hot_scoring_ranks_media_code_and_games_accurately() {
        let cold_movie = score_tier_candidate(
            Path::new("C:\\Users\\ali\\Videos\\tatil-4k.mkv"),
            false,
            4 * GIB,
            100,
            true,
        )
        .unwrap();
        assert!(
            cold_movie.cold_score >= 90,
            "cold_movie={}",
            cold_movie.cold_score
        );
        assert_eq!(cold_movie.category, "video");
        assert_eq!(cold_movie.recommended_direction, "to_hdd");

        let cold_iso = score_tier_candidate(
            Path::new("C:\\Users\\ali\\Downloads\\ubuntu.iso"),
            false,
            2 * GIB,
            60,
            true,
        )
        .unwrap();
        assert!(cold_iso.cold_score >= 85);
        assert_eq!(cold_iso.category, "iso");

        let cold_installer = score_tier_candidate(
            Path::new("C:\\Users\\ali\\Downloads\\setup.exe"),
            false,
            300 * MIB,
            45,
            true,
        )
        .unwrap();
        assert!(cold_installer.cold_score >= 65);
        assert_eq!(cold_installer.category, "installer");

        let active_code = score_tier_candidate(
            Path::new("C:\\Projeler\\app\\src\\main.rs"),
            false,
            80 * MIB,
            20,
            true,
        )
        .unwrap();
        assert!(
            active_code.cold_score <= 15,
            "active_code cold_score should be low, got {}",
            active_code.cold_score
        );

        let active_db = score_tier_candidate(
            Path::new("C:\\Data\\app.sqlite3"),
            false,
            200 * MIB,
            10,
            true,
        )
        .unwrap();
        assert!(
            active_db.cold_score <= 15,
            "active_db cold_score should be low, got {}",
            active_db.cold_score
        );

        let hot_game = score_tier_candidate(
            Path::new("D:\\Games\\Cyberpunk2077"),
            true,
            12 * GIB,
            2,
            false,
        )
        .unwrap();
        assert!(hot_game.hot_score >= 85, "hot_game={}", hot_game.hot_score);
        assert_eq!(hot_game.category, "game");
        assert_eq!(hot_game.recommended_direction, "to_ssd");

        let stale_game =
            score_tier_candidate(Path::new("D:\\Games\\OldGame"), true, 12 * GIB, 90, false)
                .unwrap();
        assert!(stale_game.hot_score <= 35);
    }

    #[test]
    fn analyze_volume_root_detects_companion_slc_pressure_and_ranks_candidates() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("Users\\demo\\Videos")).unwrap();
        std::fs::create_dir_all(root.path().join("Users\\demo\\Downloads")).unwrap();
        std::fs::create_dir_all(root.path().join("Windows\\System32")).unwrap();
        std::fs::create_dir_all(root.path().join("Users\\demo\\AppData\\Local")).unwrap();

        std::fs::write(
            root.path().join("Users\\demo\\Videos\\old_film.mkv"),
            vec![0xABu8; 256 * 1024],
        )
        .unwrap();
        std::fs::write(
            root.path().join("Users\\demo\\Downloads\\archive.zip"),
            vec![0xCDu8; 128 * 1024],
        )
        .unwrap();
        // Forbidden files must never appear in candidates!
        std::fs::write(
            root.path().join("Windows\\System32\\kernel.bin"),
            vec![0xEEu8; 512 * 1024],
        )
        .unwrap();
        std::fs::write(
            root.path().join("Users\\demo\\AppData\\Local\\secret.mkv"),
            vec![0xEEu8; 512 * 1024],
        )
        .unwrap();

        let ssd = mock_disk(
            "ssd-1",
            "NVMe SSD",
            "Samsung 980 PRO",
            vec![json!({
                "letter": "C",
                "filesystem": "NTFS",
                "size_bytes": 500 * GIB,
                "free_bytes": 80 * GIB // 84% full -> high SLC pressure
            })],
        );
        let hdd = mock_disk(
            "hdd-1",
            "HDD",
            "Seagate Barracuda 2TB",
            vec![json!({
                "letter": "D",
                "filesystem": "NTFS",
                "size_bytes": 2000 * GIB,
                "free_bytes": 1200 * GIB
            })],
        );
        let disks = vec![ssd.clone(), hdd.clone()];

        let analysis = analyze_volume_root(root.path(), &disks, &ssd, "C", &[], 64 * 1024).unwrap();
        assert_eq!(analysis["is_ssd"], true);
        assert_eq!(analysis["slc_pressure"], "high");
        assert!(analysis["fill_percent"].as_f64().unwrap() > 75.0);
        assert_eq!(analysis["companion_role"], "hdd_archive");
        assert_eq!(analysis["companion"]["letter"], "D");

        let candidates = analysis["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 2);
        for c in candidates {
            let rel = Path::new(c["path"].as_str().unwrap())
                .strip_prefix(root.path())
                .unwrap()
                .to_string_lossy();
            assert!(
                !rel.contains("Windows"),
                "Forbidden Windows path leaked: {rel}"
            );
            assert!(
                !rel.contains("AppData"),
                "Forbidden AppData path leaked: {rel}"
            );
            assert!(!rel.contains("kernel.bin"));
            assert!(!rel.contains("secret.mkv"));
        }
        assert_eq!(candidates[0]["name"], "old_film.mkv");
        assert_eq!(candidates[0]["recommended_direction"], "to_hdd");
        assert_eq!(
            analysis["total_reclaimable_bytes"].as_u64().unwrap(),
            (256 + 128) * 1024
        );

        // When no HDD is attached, companion must be null honestly.
        let solo = analyze_volume_root(
            root.path(),
            std::slice::from_ref(&ssd),
            &ssd,
            "C",
            &[],
            64 * 1024,
        )
        .unwrap();
        assert!(solo["companion"].is_null());
        assert!(solo["companion_role"].is_null());

        // For HDD with >= 20 GiB free SSD, companion is ssd_boost.
        let hdd_replay = analyze_volume_replay(&disks, &hdd, "D", &[]);
        assert_eq!(hdd_replay["replay"], true);
        assert_eq!(hdd_replay["companion_role"], "ssd_boost");
        assert_eq!(hdd_replay["companion"]["letter"], "C");
    }

    #[test]
    fn standalone_file_relocate_and_rollback_sha256_roundtrip() {
        let src_dir = tempfile::tempdir().unwrap();
        let dst_dir = tempfile::tempdir().unwrap();
        let file_path = src_dir.path().join("movie_sample.mkv");
        let payload: Vec<u8> = (0..131_072u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&file_path, &payload).unwrap();

        let rec = relocate_item_into_dir(&file_path, dst_dir.path(), "to_hdd", false, 0).unwrap();
        assert_eq!(rec["is_dir"], false);
        assert_eq!(rec["junction_created"], false);
        assert_eq!(rec["size_bytes"], payload.len() as u64);
        assert_eq!(rec["file_count"], 1);
        assert!(!file_path.exists());
        let moved_path = PathBuf::from(rec["target_path"].as_str().unwrap());
        assert_eq!(std::fs::read(&moved_path).unwrap(), payload);

        let rb = rollback_move(&rec).unwrap();
        assert_eq!(rb["rolled_back"], true);
        assert!(!moved_path.exists());
        assert_eq!(std::fs::read(&file_path).unwrap(), payload);
    }

    #[test]
    fn directory_relocate_with_ntfs_junction_and_safe_rollback_roundtrip() {
        let src_parent = tempfile::tempdir().unwrap();
        let dst_parent = tempfile::tempdir().unwrap();
        let folder = src_parent.path().join("MyVideos");
        let sub = folder.join("2025");
        std::fs::create_dir_all(&sub).unwrap();
        let f1 = folder.join("clip1.mp4");
        let f2 = sub.join("clip2.mkv");
        let data1 = b"SSD Sentinel v1.8.0 verified video payload alpha".to_vec();
        let data2 = vec![0x42u8; 65_536];
        std::fs::write(&f1, &data1).unwrap();
        std::fs::write(&f2, &data2).unwrap();

        let rec = relocate_item_into_dir(&folder, dst_parent.path(), "to_hdd", true, 0).unwrap();
        assert_eq!(rec["is_dir"], true);
        assert_eq!(rec["junction_created"], true);
        assert_eq!(rec["file_count"], 2);
        assert_eq!(rec["size_bytes"], (data1.len() + data2.len()) as u64);

        // Source path is now a transparent NTFS Junction (or symlink) resolving to target!
        assert!(is_existing_reparse(&folder));
        assert_eq!(std::fs::read(&f1).unwrap(), data1);
        assert_eq!(std::fs::read(&f2).unwrap(), data2);

        let target_folder = PathBuf::from(rec["target_path"].as_str().unwrap());
        assert!(target_folder.is_dir());
        assert!(!is_existing_reparse(&target_folder));
        assert_eq!(
            std::fs::read(target_folder.join("clip1.mp4")).unwrap(),
            data1
        );

        // Reject relocating an already-junctioned folder a second time.
        assert!(relocate_item_into_dir(&folder, dst_parent.path(), "to_hdd", true, 0).is_err());

        // 1-Click Rollback removes ONLY the junction, restores real folder + files with SHA-256!
        let rb = rollback_move(&rec).unwrap();
        assert_eq!(rb["rolled_back"], true);
        assert!(!target_folder.exists());
        assert!(folder.is_dir());
        assert!(!is_existing_reparse(&folder));
        assert_eq!(std::fs::read(&f1).unwrap(), data1);
        assert_eq!(std::fs::read(&f2).unwrap(), data2);
    }

    #[test]
    fn relocate_rejects_forbidden_folder_inside_sandbox() {
        let src_parent = tempfile::tempdir().unwrap();
        let dst_parent = tempfile::tempdir().unwrap();
        let win_dir = src_parent.path().join("Windows");
        std::fs::create_dir_all(&win_dir).unwrap();
        std::fs::write(win_dir.join("test.bin"), b"nope").unwrap();

        assert!(relocate_item_into_dir(&win_dir, dst_parent.path(), "to_hdd", true, 0).is_err());
        assert!(win_dir.join("test.bin").exists());
    }

    #[test]
    fn relocate_rejects_insufficient_reserve_and_invalid_inputs_without_touching_source() {
        let src_parent = tempfile::tempdir().unwrap();
        let dst_parent = tempfile::tempdir().unwrap();
        let sample = src_parent.path().join("keep_me.mkv");
        std::fs::write(&sample, b"precious-data").unwrap();

        #[cfg(windows)]
        {
            assert!(relocate_item_into_dir(
                &sample,
                dst_parent.path(),
                "to_hdd",
                false,
                u64::MAX / 2,
            )
            .is_err());
            assert_eq!(std::fs::read(&sample).unwrap(), b"precious-data");
        }

        assert!(relocate_item(Path::new("C:\\Windows\\notepad.exe"), "D", "to_hdd").is_err());
        assert!(relocate_item(&sample, "c", "to_hdd").is_err());
        assert!(relocate_item(&sample, "C", "invalid").is_err());
        assert_eq!(std::fs::read(&sample).unwrap(), b"precious-data");
    }

    #[test]
    fn smart_tier_job_guards_disk_type_and_failing_hdd_smart() {
        use crate::core::Config;
        use crate::model::SmartRow;

        let tmp = tempfile::tempdir().unwrap();
        let core = Core::new(Config {
            smartctl: "missing.exe".into(),
            data_dir: tmp.path().into(),
            snapshot_dir: None,
        })
        .unwrap();

        let ssd = mock_disk("ssd-1", "NVMe SSD", "Samsung", vec![]);
        let mut failing_hdd = mock_disk("hdd-bad", "HDD", "OldHDD", vec![]);
        failing_hdd.smart.push(SmartRow {
            id: "197".into(),
            name: "Bekleyen sektör".into(),
            original: "Current_Pending_Sector".into(),
            raw: "2".into(),
            value: Some(100.0),
            worst: Some(100.0),
            threshold: Some(0.0),
            description: String::new(),
            level: "warning".into(),
        });

        let ssd_on_hdd = JobRequest {
            kind: "smart_tier".into(),
            disk_id: failing_hdd.id.clone(),
            letter: "D".into(),
            confirmation: String::new(),
            options: json!({"mode": "ssd_extent_optimize"}),
        };
        assert!(run_smart_tier_job(&core, &ssd_on_hdd, &failing_hdd, tmp.path()).is_err());

        let hdd_on_ssd = JobRequest {
            kind: "smart_tier".into(),
            disk_id: ssd.id.clone(),
            letter: "C".into(),
            confirmation: String::new(),
            options: json!({"mode": "hdd_outer_optimize"}),
        };
        assert!(run_smart_tier_job(&core, &hdd_on_ssd, &ssd, tmp.path()).is_err());

        let bad_hdd_req = JobRequest {
            kind: "smart_tier".into(),
            disk_id: failing_hdd.id.clone(),
            letter: "D".into(),
            confirmation: String::new(),
            options: json!({"mode": "hdd_outer_optimize"}),
        };
        let err = run_smart_tier_job(&core, &bad_hdd_req, &failing_hdd, tmp.path()).unwrap_err();
        assert!(
            err.contains("#197"),
            "Expected SMART #197 guard, got: {err}"
        );
    }

    #[test]
    fn relocate_with_progress_streams_updates_and_honors_cancellation_cleanly() {
        let src_parent = tempfile::tempdir().unwrap();
        let dst_parent = tempfile::tempdir().unwrap();
        let folder = src_parent.path().join("BigFolder");
        std::fs::create_dir_all(&folder).unwrap();
        let buf_a = vec![0x11u8; 2 * 1024 * 1024];
        let buf_b = vec![0x22u8; 2 * 1024 * 1024];
        let buf_c = vec![0x33u8; 2 * 1024 * 1024];
        std::fs::write(folder.join("a.bin"), &buf_a).unwrap();
        std::fs::write(folder.join("b.bin"), &buf_b).unwrap();
        std::fs::write(folder.join("c.bin"), &buf_c).unwrap();

        let cancel = AtomicBool::new(false);
        let mut updates: Vec<(u64, u64)> = Vec::new();
        let rec = relocate_item_into_dir_with_cancel(
            &folder,
            dst_parent.path(),
            "to_hdd",
            true,
            0,
            &cancel,
            &mut |done, total, _, _, _| {
                updates.push((done, total));
            },
        )
        .unwrap();
        assert!(updates.len() >= 3, "Expected streamed progress updates");
        assert!(updates.windows(2).all(|w| w[0].0 <= w[1].0));
        assert_eq!(updates.last().unwrap().0, updates.last().unwrap().1);

        rollback_move(&rec).unwrap();
        assert!(folder.is_dir());
        assert!(!is_existing_reparse(&folder));

        let mut saw_progress = false;
        let err = relocate_item_into_dir_with_cancel(
            &folder,
            dst_parent.path(),
            "to_hdd",
            true,
            0,
            &cancel,
            &mut |_, _, _, _, _| {
                saw_progress = true;
                cancel.store(true, Ordering::SeqCst);
            },
        )
        .unwrap_err();
        assert_eq!(err, process::CANCELLED);
        assert!(saw_progress);
        assert!(!dst_parent.path().join("BigFolder").exists());
        assert_eq!(std::fs::read(folder.join("a.bin")).unwrap(), buf_a);
        assert_eq!(std::fs::read(folder.join("b.bin")).unwrap(), buf_b);
        assert_eq!(std::fs::read(folder.join("c.bin")).unwrap(), buf_c);
    }
}
