use chrono::{DateTime, TimeZone, Utc};
#[cfg(windows)]
use std::io::Read;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::{Path, PathBuf},
    time::SystemTime,
};

const WINDOWS_EPOCH_OFFSET_100NS: i64 = 116_444_736_000_000_000;
const MIN_VALID_UNIX_SEC: i64 = 1_104_537_600; // 2005-01-01
const MAX_VALID_UNIX_SEC: i64 = 2_208_988_800; // 2040-01-01
const MAX_FORENSIC_APPS: usize = 160;

#[derive(Clone, Debug)]
pub struct ForensicAppTrace {
    pub exe_path: String,
    pub name: String,
    pub publisher: Option<String>,
    pub sources: Vec<String>,
    pub run_count: Option<u32>,
    pub last_run_at: Option<String>,
    pub vss_path: Option<String>,
    pub drive_letter: String,
    pub is_game: bool,
    pub install_dir: Option<String>,
    pub appdata_paths: Vec<String>,
    pub leftover_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct VssSnapshotFile {
    pub original_path: String,
    pub vss_path: String,
    pub size_bytes: u64,
    pub modified_at: Option<String>,
    pub drive_letter: String,
}

#[derive(Clone, Debug)]
pub struct CachedMsiFile {
    pub msi_path: String,
    pub product_name: String,
    pub publisher: Option<String>,
    pub size_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct AppDataEntry {
    pub folder_name: String,
    pub path: String,
    pub size_bytes: u64,
    pub scope: String,
    pub is_installed: bool,
    pub has_registry_key: bool,
    pub modified_at: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DownloadsInstaller {
    pub path: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub modified_at: Option<String>,
    pub drive_letter: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedPrefetch {
    pub exe_name: String,
    pub full_path: Option<String>,
    pub run_count: u32,
    pub last_run_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedGameConfig {
    pub exe_path: String,
    pub title: Option<String>,
    pub is_game: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsnDeletedRecord {
    pub file_ref: u64,
    pub parent_ref: u64,
    pub file_name: String,
    pub deleted_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndxGhostEntry {
    pub parent_ref: u64,
    pub file_name: String,
    pub real_size: u64,
    pub modified_at: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct VolumeMap {
    /// Lowercased `\device\harddiskvolume3` -> `"C:"` (sorted longest-first).
    pub device_to_drive: Vec<(String, String)>,
    /// Lowercased volume GUID `"{xxxxxxxx-xxxx-...}"` -> `"C:"`.
    pub guid_to_drive: HashMap<String, String>,
    /// Volume serial number (32-bit NTFS serial) -> `"C:"`.
    pub serial_to_drive: HashMap<u32, String>,
    /// System drive prefix, e.g. `"C:"`.
    pub system_drive: String,
}

#[derive(Clone, Debug, Default)]
pub struct ForensicSweep {
    pub traces: Vec<ForensicAppTrace>,
    pub vss_shadows_found: usize,
    pub vss_files: Vec<VssSnapshotFile>,
    pub cached_msis: Vec<CachedMsiFile>,
    pub source_counts: HashMap<String, usize>,
    pub appdata_entries: Vec<AppDataEntry>,
    pub downloads_installers: Vec<DownloadsInstaller>,
}

fn filetime_to_rfc3339(ft: i64) -> Option<String> {
    if ft <= WINDOWS_EPOCH_OFFSET_100NS {
        return None;
    }
    let secs = (ft - WINDOWS_EPOCH_OFFSET_100NS) / 10_000_000;
    if !(MIN_VALID_UNIX_SEC..=MAX_VALID_UNIX_SEC).contains(&secs) {
        return None;
    }
    Utc.timestamp_opt(secs, 0)
        .single()
        .map(|dt| dt.to_rfc3339())
}

#[cfg(windows)]
fn system_time_to_rfc3339(st: SystemTime) -> Option<String> {
    let dt: DateTime<Utc> = st.into();
    let secs = dt.timestamp();
    if !(MIN_VALID_UNIX_SEC..=MAX_VALID_UNIX_SEC).contains(&secs) {
        return None;
    }
    Some(dt.to_rfc3339())
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn system_time_to_rfc3339(st: SystemTime) -> Option<String> {
    let dt: DateTime<Utc> = st.into();
    Some(dt.to_rfc3339())
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let words: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&w| w != 0)
        .collect();
    String::from_utf16_lossy(&words)
}

fn decode_utf16le_exact(bytes: &[u8]) -> String {
    let words: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&words)
        .trim_end_matches('\0')
        .to_owned()
}

pub fn rot13_decode(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            'a'..='m' | 'A'..='M' => ((c as u8) + 13) as char,
            'n'..='z' | 'N'..='Z' => ((c as u8) - 13) as char,
            _ => c,
        })
        .collect()
}

pub fn resolve_known_folder_guid(path: &str) -> String {
    let up = path.to_ascii_uppercase();
    let user_profile = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\User".into());
    let appdata =
        std::env::var("APPDATA").unwrap_or_else(|_| format!("{user_profile}\\AppData\\Roaming"));
    let local_appdata =
        std::env::var("LOCALAPPDATA").unwrap_or_else(|_| format!("{user_profile}\\AppData\\Local"));
    let program_data = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
    let sys_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());

    let table: [(&str, String); 11] = [
        (
            "{6D809377-6AF0-444B-8957-A3773F02200E}",
            format!("{sys_drive}\\Program Files"),
        ),
        (
            "{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}",
            format!("{sys_drive}\\Program Files (x86)"),
        ),
        (
            "{374DE290-123F-4565-9164-39C4925E467B}",
            format!("{user_profile}\\Downloads"),
        ),
        (
            "{B4BFCC3A-DB2C-424C-B029-7FE99A87C641}",
            format!("{user_profile}\\Desktop"),
        ),
        (
            "{FDD39AD0-238F-46AF-ADB4-6C85480369C7}",
            format!("{user_profile}\\Documents"),
        ),
        ("{F1B32785-6FBA-4FCF-9D55-7B8E7F157091}", local_appdata),
        ("{3EB685DB-65F9-4CF6-A03A-E3EF65729F3D}", appdata),
        ("{62AB5D82-FDC1-4DC3-A9DD-070D1D495D97}", program_data),
        (
            "{905E63B6-C1BF-494E-B29C-65B732D3D21A}",
            format!("{sys_drive}\\Program Files"),
        ),
        (
            "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}",
            format!("{sys_drive}\\Windows\\System32"),
        ),
        (
            "{F38BF404-1D43-42F2-9305-67DE0B28FC23}",
            format!("{sys_drive}\\Windows"),
        ),
    ];
    for (guid, replacement) in &table {
        if up.starts_with(guid) {
            return format!("{replacement}{}", &path[guid.len()..]);
        }
    }
    path.to_owned()
}

pub fn expand_windows_env_vars(raw: &str, system_drive: &str) -> String {
    if !raw.contains('%') {
        return raw.to_owned();
    }
    let sys_drive = if system_drive.is_empty() {
        std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
    } else {
        system_drive.to_owned()
    };
    let sys_root = std::env::var("SystemRoot").unwrap_or_else(|_| format!("{sys_drive}\\Windows"));
    let pf =
        std::env::var("ProgramFiles").unwrap_or_else(|_| format!("{sys_drive}\\Program Files"));
    let pf86 = std::env::var("ProgramFiles(x86)")
        .unwrap_or_else(|_| format!("{sys_drive}\\Program Files (x86)"));
    let up = std::env::var("USERPROFILE").unwrap_or_else(|_| format!("{sys_drive}\\Users\\User"));
    let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| format!("{up}\\AppData\\Local"));
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| format!("{up}\\AppData\\Roaming"));
    let pd = std::env::var("ProgramData").unwrap_or_else(|_| format!("{sys_drive}\\ProgramData"));
    let public = std::env::var("PUBLIC").unwrap_or_else(|_| format!("{sys_drive}\\Users\\Public"));

    let vars: [(&str, &str); 10] = [
        ("%systemdrive%", &sys_drive),
        ("%systemroot%", &sys_root),
        ("%windir%", &sys_root),
        ("%programfiles%", &pf),
        ("%programfiles(x86)%", &pf86),
        ("%userprofile%", &up),
        ("%localappdata%", &local),
        ("%appdata%", &appdata),
        ("%programdata%", &pd),
        ("%public%", &public),
    ];

    let mut out = raw.to_owned();
    for (token, value) in vars {
        let lower = out.to_ascii_lowercase();
        if let Some(idx) = lower.find(token) {
            let mut next = String::with_capacity(out.len() + value.len());
            next.push_str(&out[..idx]);
            next.push_str(value);
            next.push_str(&out[idx + token.len()..]);
            out = next;
        }
    }
    out
}

/// Extracts uppercase single drive letter (e.g. `"C"`) from a Windows path.
pub fn extract_drive_letter(path: &str) -> Option<String> {
    let trimmed = path
        .trim()
        .trim_matches('"')
        .strip_prefix("\\\\?\\")
        .or_else(|| path.trim().trim_matches('"').strip_prefix("\\??\\"))
        .unwrap_or_else(|| path.trim().trim_matches('"'));
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        Some((bytes[0] as char).to_ascii_uppercase().to_string())
    } else {
        None
    }
}

/// Normalizes Windows kernel/Prefetch/Firewall/Registry paths into canonical `X:\...` DOS paths.
/// Supports:
/// - `\Device\HarddiskVolumeN\...` (longest-first + boundary-checked)
/// - `\VOLUME{guid}\...` and `\\?\Volume{guid}\...`
/// - `\VOLUME{<filetime_hex>-<serial_hex>}\...` (Windows 10/11 Prefetch format)
/// - `\??\C:\...`, `\\?\C:\...`, `SYSVOL\...`, `\SystemRoot\...`
/// - Known folder GUIDs and `%ProgramFiles%` / `%USERPROFILE%` / `%LOCALAPPDATA%` env vars
pub fn normalize_windows_path(
    raw: &str,
    vol_map: &VolumeMap,
    fallback_letter: Option<&str>,
) -> Option<String> {
    let mut s = raw
        .trim()
        .trim_matches('"')
        .trim_end_matches('\0')
        .replace('/', "\\");
    if s.is_empty() {
        return None;
    }

    let sys_drive = if vol_map.system_drive.is_empty() {
        "C:".to_owned()
    } else {
        vol_map.system_drive.clone()
    };

    s = resolve_known_folder_guid(&s);
    s = expand_windows_env_vars(&s, &sys_drive);

    if let Some(rest) = s.strip_prefix("\\??\\") {
        s = rest.to_owned();
    } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
        let lower_rest = rest.to_ascii_lowercase();
        if lower_rest.starts_with("volume{") {
            s = format!("\\{rest}");
        } else {
            s = rest.to_owned();
        }
    }

    let lower = s.to_ascii_lowercase();

    // 1. \SystemRoot\...
    if lower.starts_with("\\systemroot\\") {
        s = format!("{sys_drive}\\Windows{}", &s["\\systemroot".len()..]);
    } else if lower.starts_with("sysvol\\") {
        s = format!("{sys_drive}\\{}", &s["sysvol\\".len()..]);
    } else if lower.starts_with("\\device\\") {
        // 2. \Device\HarddiskVolumeN\... (ensure longest prefix first + path boundary!)
        let mut sorted: Vec<&(String, String)> = vol_map.device_to_drive.iter().collect();
        sorted.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
        let mut mapped = None;
        for (dev, drive) in sorted {
            if lower.starts_with(dev.as_str()) {
                let boundary = s.len() == dev.len() || s.as_bytes().get(dev.len()) == Some(&b'\\');
                if boundary {
                    mapped = Some(format!("{drive}{}", &s[dev.len()..]));
                    break;
                }
            }
        }
        s = mapped?;
    } else if lower.starts_with("\\volume{") {
        // 3. \VOLUME{guid}\... OR Win10/11 Prefetch \VOLUME{<ft_hex>-<serial_hex>}\...
        let close_idx = s.find('}')?;
        let inside = &s[7..=close_idx]; // includes `{...}`
        let inside_lower = inside.to_ascii_lowercase();
        let tail = &s[close_idx + 1..];
        let tail_slash = if tail.starts_with('\\') {
            tail.to_owned()
        } else {
            format!("\\{tail}")
        };

        let mut resolved_drive: Option<String> = vol_map.guid_to_drive.get(&inside_lower).cloned();
        if resolved_drive.is_none() {
            // Try Prefetch `{01d8f3a1b2c3d4e5-a1b2c3d4}` volume serial suffix
            let inner = inside.trim_start_matches('{').trim_end_matches('}');
            if let Some((_, serial_hex)) = inner.rsplit_once('-') {
                if serial_hex.len() == 8 && serial_hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    if let Ok(serial) = u32::from_str_radix(serial_hex, 16) {
                        resolved_drive = vol_map.serial_to_drive.get(&serial).cloned();
                    }
                }
            }
        }
        if resolved_drive.is_none() {
            if let Some(fb) = fallback_letter {
                let ch = fb.trim().trim_end_matches(':');
                if ch.len() == 1 && ch.as_bytes()[0].is_ascii_alphabetic() {
                    resolved_drive = Some(format!("{}:", ch.to_ascii_uppercase()));
                }
            }
        }
        let drive = resolved_drive?;
        s = format!("{drive}{tail_slash}");
    }

    // Collapse duplicate backslashes inside path
    while s.contains("\\\\") {
        s = s.replace("\\\\", "\\");
    }

    // Validate `X:\...`
    let bytes = s.as_bytes();
    if bytes.len() >= 6 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        let upper_drive = (bytes[0] as char).to_ascii_uppercase();
        Some(format!("{upper_drive}{}", &s[1..]))
    } else {
        None
    }
}

/// Parses a Windows 7/10/11 72-byte UserAssist binary registry payload.
pub fn parse_userassist_72(data: &[u8]) -> Option<(u32, Option<String>)> {
    if data.len() < 68 {
        return None;
    }
    let run_count = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let ft = i64::from_le_bytes(data[60..68].try_into().ok()?);
    Some((run_count, filetime_to_rfc3339(ft)))
}

/// Parses Windows 10/11 `AppCompatCache` (ShimCache) `10ts` binary entries.
pub fn parse_shimcache_10ts(data: &[u8]) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let Some(mut pos) = data.windows(4).position(|w| w == b"10ts") else {
        return out;
    };
    while pos + 14 <= data.len() {
        if &data[pos..pos + 4] != b"10ts" {
            let Some(rel) = data[pos..].windows(4).position(|w| w == b"10ts") else {
                break;
            };
            pos += rel;
            if pos + 14 > data.len() {
                break;
            }
        }
        let entry_size =
            u32::from_le_bytes([data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11]])
                as usize;
        let path_len = u16::from_le_bytes([data[pos + 12], data[pos + 13]]) as usize;
        let path_start = pos + 14;
        let path_end = path_start.saturating_add(path_len);
        if (8..=1024).contains(&path_len) && path_end <= data.len() {
            let raw_path = decode_utf16le_exact(&data[path_start..path_end]);
            let clean = raw_path
                .strip_prefix("\\??\\")
                .or_else(|| raw_path.strip_prefix("SYSVOL\\"))
                .unwrap_or(&raw_path)
                .to_owned();
            let modified = if path_end + 8 <= data.len() {
                let ft =
                    i64::from_le_bytes(data[path_end..path_end + 8].try_into().unwrap_or([0; 8]));
                filetime_to_rfc3339(ft)
            } else {
                None
            };
            if clean.len() >= 6 {
                out.push((clean, modified));
            }
        }
        let advance = if entry_size >= 12 {
            12usize.saturating_add(entry_size)
        } else {
            14usize.saturating_add(path_len).max(16)
        };
        if advance == 0 || pos.saturating_add(advance) <= pos {
            break;
        }
        pos = pos.saturating_add(advance);
    }
    out
}

/// Parses an uncompressed Windows Prefetch `SCCA` buffer (v23 / v26 / v30 / v31).
pub fn parse_prefetch_scca(data: &[u8]) -> Option<ParsedPrefetch> {
    if data.len() < 0xE0 || &data[4..8] != b"SCCA" {
        return None;
    }
    let version = u32::from_le_bytes(data[0..4].try_into().ok()?);
    if !(23..=31).contains(&version) {
        return None;
    }
    let exe_name = decode_utf16le(&data[0x10..0x4C]).trim().to_owned();
    if exe_name.is_empty() {
        return None;
    }
    let str_off = u32::from_le_bytes(data[0x64..0x68].try_into().ok()?) as usize;
    let str_sz = u32::from_le_bytes(data[0x68..0x6C].try_into().ok()?) as usize;
    let last_ft = i64::from_le_bytes(data[0x80..0x88].try_into().ok()?);
    let last_run_at = filetime_to_rfc3339(last_ft);

    // v30/v31 220-byte layout has RunCount at 0xD0; Win10 1903+/Win11 212-byte variant at 0xC8.
    let rc_d0 = u32::from_le_bytes(data[0xD0..0xD4].try_into().ok()?);
    let rc_c8 = u32::from_le_bytes(data[0xC8..0xCC].try_into().ok()?);
    let run_count = if (1..1_000_000).contains(&rc_d0) {
        rc_d0
    } else if (1..1_000_000).contains(&rc_c8) {
        rc_c8
    } else {
        1
    };

    let mut full_path = None;
    let mut prefix_fallback = None;
    if str_off < data.len() && str_sz >= 8 {
        let end = str_off.saturating_add(str_sz).min(data.len());
        let exe_lower = exe_name.to_ascii_lowercase();
        let needle_suffix = format!("\\{exe_lower}");
        let mut cursor = str_off;
        while cursor + 4 <= end {
            // Scan exact UTF-16LE NUL terminator to avoid cursor drift on invalid code units
            let mut term = cursor;
            while term + 2 <= end && (data[term] != 0 || data[term + 1] != 0) {
                term += 2;
            }
            if term > cursor {
                let s = decode_utf16le_exact(&data[cursor..term]);
                let sl = s.to_ascii_lowercase();
                if sl.ends_with(&needle_suffix) {
                    full_path = Some(s);
                    break;
                }
                // Handle Windows Prefetch 29-char truncated exe_name in header
                if prefix_fallback.is_none() && sl.ends_with(".exe") {
                    if let Some(fname) = sl.rsplit('\\').next() {
                        if fname.starts_with(&exe_lower) {
                            prefix_fallback = Some(s);
                        }
                    }
                }
            }
            if term + 2 > end {
                break;
            }
            cursor = term + 2;
        }
    }

    Some(ParsedPrefetch {
        exe_name,
        full_path: full_path.or(prefix_fallback),
        run_count,
        last_run_at,
    })
}

/// Fallback parser for Windows Prefetch `.pf` filenames (`APPNAME.EXE-A1B2C3D4.pf`) when
/// decompression or full-path string extraction fails.
pub fn parse_prefetch_filename_fallback(
    file_name: &str,
    modified_at: Option<String>,
) -> Option<ParsedPrefetch> {
    let trimmed = file_name.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.ends_with(".pf") {
        return None;
    }
    let without_pf = &trimmed[..trimmed.len() - 3];
    let (exe_part, hash_part) = without_pf.rsplit_once('-')?;
    if hash_part.len() != 8 || !hash_part.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let exe_clean = exe_part.trim();
    if exe_clean.len() < 4 {
        return None;
    }
    let exe_name = if exe_clean.to_ascii_lowercase().ends_with(".exe") {
        exe_clean.to_owned()
    } else if exe_clean.len() >= 25 {
        // Prefetch truncates long executable names around 29 chars
        format!("{exe_clean}.exe")
    } else {
        return None;
    };
    Some(ParsedPrefetch {
        exe_name,
        full_path: None,
        run_count: 1,
        last_run_at: modified_at,
    })
}

/// Parses a Windows Firewall rule string (`v2.31|Action=Allow|...|App=C:\Path\App.exe|Name=MyApp|`).
pub fn parse_firewall_rule_value(
    raw: &str,
    vol_map: &VolumeMap,
) -> Option<(String, Option<String>)> {
    let mut app_raw: Option<&str> = None;
    let mut name_raw: Option<&str> = None;
    for part in raw.split('|') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim();
        if val.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("App") {
            app_raw = Some(val);
        } else if key.eq_ignore_ascii_case("Name") {
            name_raw = Some(val);
        }
    }
    let app = app_raw?;
    if app.eq_ignore_ascii_case("System") {
        return None;
    }
    let normalized = normalize_windows_path(app, vol_map, None)?;
    if !normalized.to_ascii_lowercase().ends_with(".exe") {
        return None;
    }
    let friendly = name_raw
        .map(str::trim)
        .filter(|n| {
            n.len() >= 2
                && !n.starts_with('@')
                && !n.to_ascii_lowercase().starts_with("allow ")
                && !n.to_ascii_lowercase().starts_with("block ")
        })
        .map(str::to_owned);
    Some((normalized, friendly))
}

/// Parses a `HKCU\System\GameConfigStore\Children\*` entry (`MatchedExeFullPath` + `TitleName`).
pub fn parse_game_config_entry(
    matched_exe: &str,
    title_name: Option<&str>,
    vol_map: &VolumeMap,
) -> Option<ParsedGameConfig> {
    let normalized = normalize_windows_path(matched_exe, vol_map, None)?;
    if !normalized.to_ascii_lowercase().ends_with(".exe") {
        return None;
    }
    let title = title_name
        .map(str::trim)
        .filter(|t| {
            t.len() >= 2
                && !t.starts_with('@')
                && !t.eq_ignore_ascii_case("desktop")
                && !t.eq_ignore_ascii_case("window")
        })
        .map(str::to_owned);
    let fname_lower = normalized
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let stem = fname_lower.trim_end_matches(".exe");
    const NON_GAME_FULLSCREEN: &[&str] = &[
        "chrome",
        "msedge",
        "firefox",
        "brave",
        "opera",
        "vivaldi",
        "vlc",
        "mpc-hc",
        "mpc-hc64",
        "mpv",
        "potplayermini",
        "potplayermini64",
        "obs64",
        "obs32",
        "discord",
        "spotify",
        "telegram",
        "code",
        "blender",
        "zoom",
        "teams",
        "slack",
        "notion",
        "figma",
        "whatsapp",
        "winword",
        "excel",
        "powerpnt",
        "acrobat",
    ];
    let is_game = !NON_GAME_FULLSCREEN.contains(&stem);
    Some(ParsedGameConfig {
        exe_path: normalized,
        title,
        is_game,
    })
}

/// Parses `USN_RECORD_V2` structures returned by `FSCTL_READ_USN_JOURNAL`.
pub fn parse_usn_records_v2(buf: &[u8]) -> Vec<UsnDeletedRecord> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 60 <= buf.len() {
        let rec_len =
            u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        if rec_len < 60 || pos + rec_len > buf.len() {
            break;
        }
        let major = u16::from_le_bytes([buf[pos + 4], buf[pos + 5]]);
        if major == 2 {
            let file_ref = u64::from_le_bytes(buf[pos + 8..pos + 16].try_into().unwrap_or([0; 8]))
                & 0x0000_FFFF_FFFF_FFFF;
            let parent_ref =
                u64::from_le_bytes(buf[pos + 16..pos + 24].try_into().unwrap_or([0; 8]))
                    & 0x0000_FFFF_FFFF_FFFF;
            let ts = i64::from_le_bytes(buf[pos + 32..pos + 40].try_into().unwrap_or([0; 8]));
            let reason =
                u32::from_le_bytes([buf[pos + 40], buf[pos + 41], buf[pos + 42], buf[pos + 43]]);
            let attrs =
                u32::from_le_bytes([buf[pos + 52], buf[pos + 53], buf[pos + 54], buf[pos + 55]]);
            let name_len = u16::from_le_bytes([buf[pos + 56], buf[pos + 57]]) as usize;
            let name_off = u16::from_le_bytes([buf[pos + 58], buf[pos + 59]]) as usize;
            let is_dir = (attrs & 0x10) != 0;
            let is_delete = (reason & 0x0000_0200) != 0;
            if is_delete
                && !is_dir
                && name_len >= 4
                && name_off >= 60
                && pos + name_off + name_len <= pos + rec_len
            {
                let file_name =
                    decode_utf16le_exact(&buf[pos + name_off..pos + name_off + name_len]);
                if !file_name.is_empty() && !file_name.starts_with('$') {
                    out.push(UsnDeletedRecord {
                        file_ref,
                        parent_ref,
                        file_name,
                        deleted_at: filetime_to_rfc3339(ts),
                    });
                }
            }
        }
        pos += rec_len;
    }
    out
}

/// Carves ghost `$FILE_NAME` entries from the slack area of an NTFS `$I30` (`INDX`) block.
pub fn parse_indx_slack(block: &[u8]) -> Vec<IndxGhostEntry> {
    let mut out = Vec::new();
    if block.len() < 4096 || &block[0..4] != b"INDX" {
        return out;
    }
    let mut buf = block[..4096].to_vec();
    let usa_off = u16::from_le_bytes([buf[4], buf[5]]) as usize;
    let usa_cnt = u16::from_le_bytes([buf[6], buf[7]]) as usize;
    if usa_cnt >= 2 && usa_off + usa_cnt * 2 <= buf.len() {
        let usn = [buf[usa_off], buf[usa_off + 1]];
        for s in 1..usa_cnt {
            let end = s * 512 - 2;
            if end + 2 <= buf.len() && buf[end..end + 2] == usn {
                let seq_off = usa_off + s * 2;
                buf[end] = buf[seq_off];
                buf[end + 1] = buf[seq_off + 1];
            }
        }
    }
    let total_entries = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]) as usize;
    let slack_start = 24usize.saturating_add(total_entries).clamp(64, buf.len());
    let mut pos = (slack_start / 8) * 8;
    let mut seen = HashSet::new();

    while pos + 88 <= buf.len() {
        let entry_len = u16::from_le_bytes([buf[pos + 8], buf[pos + 9]]) as usize;
        let stream_len = u16::from_le_bytes([buf[pos + 10], buf[pos + 11]]) as usize;
        if (82..=580).contains(&entry_len)
            && (68..=560).contains(&stream_len)
            && stream_len <= entry_len
            && pos + entry_len <= buf.len()
        {
            let fn_base = pos + 16;
            let parent_ref =
                u64::from_le_bytes(buf[fn_base..fn_base + 8].try_into().unwrap_or([0; 8]))
                    & 0x0000_FFFF_FFFF_FFFF;
            let mtime =
                i64::from_le_bytes(buf[fn_base + 16..fn_base + 24].try_into().unwrap_or([0; 8]));
            let real_size =
                u64::from_le_bytes(buf[fn_base + 48..fn_base + 56].try_into().unwrap_or([0; 8]));
            let name_chars = buf[fn_base + 64] as usize;
            let ns = buf[fn_base + 65];
            let modified_at = filetime_to_rfc3339(mtime);
            if modified_at.is_some()
                && (3..=120).contains(&name_chars)
                && ns <= 3
                && ns != 2
                && real_size < 100 * 1024 * 1024 * 1024
                && fn_base + 66 + name_chars * 2 <= pos + entry_len
            {
                let name = decode_utf16le_exact(&buf[fn_base + 66..fn_base + 66 + name_chars * 2]);
                let ok_chars = name.contains('.')
                    && !name.starts_with('$')
                    && name.chars().all(|c| {
                        !c.is_control()
                            && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                    });
                if ok_chars && seen.insert(name.to_ascii_lowercase()) {
                    out.push(IndxGhostEntry {
                        parent_ref,
                        file_name: name,
                        real_size,
                        modified_at,
                    });
                    pos += entry_len.div_ceil(8) * 8;
                    continue;
                }
            }
        }
        pos += 8;
    }
    out
}

/// Pure-Rust MS-SHLLINK (`.lnk`) binary parser for local target path extraction.
pub fn parse_lnk_target_path(bytes: &[u8]) -> Option<String> {
    const LNK_CLSID: [u8; 16] = [
        0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    if bytes.len() < 78 {
        return None;
    }
    let hdr_sz = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    if hdr_sz != 0x4C || bytes[4..20] != LNK_CLSID {
        return None;
    }
    let flags = u32::from_le_bytes(bytes[20..24].try_into().ok()?);
    let mut pos = 76usize;
    if (flags & 0x01) != 0 {
        if pos + 2 > bytes.len() {
            return None;
        }
        let id_sz = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos = pos.saturating_add(2).saturating_add(id_sz);
    }
    if (flags & 0x02) == 0 || pos + 28 > bytes.len() {
        return None;
    }
    let li_size = u32::from_le_bytes(bytes[pos..pos + 4].try_into().ok()?) as usize;
    let li_hdr_sz = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
    let li_flags = u32::from_le_bytes(bytes[pos + 8..pos + 12].try_into().ok()?);
    if (li_flags & 0x01) == 0 || li_size < 28 || pos + li_size > bytes.len() {
        return None;
    }
    let li_end = pos + li_size;
    if li_hdr_sz >= 36 && pos + 32 <= li_end {
        let uni_off = u32::from_le_bytes(bytes[pos + 28..pos + 32].try_into().ok()?) as usize;
        if uni_off >= 36 && pos + uni_off < li_end {
            let p = decode_utf16le(&bytes[pos + uni_off..li_end]);
            if p.len() >= 4 {
                return Some(p);
            }
        }
    }
    let ansi_off = u32::from_le_bytes(bytes[pos + 16..pos + 20].try_into().ok()?) as usize;
    if ansi_off >= 28 && pos + ansi_off < li_end {
        let slice = &bytes[pos + ansi_off..li_end];
        let nul = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        let p = String::from_utf8_lossy(&slice[..nul]).trim().to_owned();
        if p.len() >= 4 {
            return Some(p);
        }
    }
    None
}

/// Ruthlessly filters Windows OS/system executables, updaters, uninstallers, temp stubs,
/// and short/hash-only garbage names while preserving real user apps & games.
pub fn is_noise_app_exe(exe_path: &str, friendly_name: Option<&str>) -> bool {
    let lower = exe_path.replace('/', "\\").to_ascii_lowercase();
    const NOISE_DIRS: &[&str] = &[
        "\\windows\\system32\\",
        "\\windows\\syswow64\\",
        "\\windows\\winsxs\\",
        "\\windows\\servicing\\",
        "\\windows\\temp\\",
        "\\windows\\softwaredistribution\\",
        "\\windows\\systemapps\\",
        "\\windows\\immersivecontrolpanel\\",
        "\\windows\\diagnostics\\",
        "\\windows\\panther\\",
        "\\windows\\microsoft.net\\",
        "\\windows\\shellcomponents\\",
        "\\windows\\systemresources\\",
        "\\windows\\installer\\",
        "\\appdata\\local\\temp\\",
        "\\windowsapps\\microsoft.",
        "\\windowsapps\\microsoftwindows.",
        "\\program files\\windows defender",
        "\\program files\\windows security",
        "\\program files\\common files\\microsoft shared\\",
        "\\program files (x86)\\common files\\microsoft shared\\",
        "\\programdata\\microsoft\\windows defender\\",
        "\\programdata\\package cache\\",
        "\\$windows.~bt\\",
        "\\$windows.~ws\\",
        "\\microsoft\\edgewebview\\",
        "\\microsoft\\edgeupdate\\",
        "\\google\\update\\",
    ];
    if NOISE_DIRS.iter().any(|d| lower.contains(d)) {
        return true;
    }

    let file_name = lower.rsplit('\\').next().unwrap_or(&lower);
    let stem = file_name
        .strip_suffix(".exe")
        .or_else(|| file_name.strip_suffix(".msi"))
        .or_else(|| file_name.strip_suffix(".msix"))
        .or_else(|| file_name.strip_suffix(".appimage"))
        .or_else(|| file_name.strip_suffix(".deb"))
        .or_else(|| file_name.strip_suffix(".rpm"))
        .or_else(|| file_name.strip_suffix(".flatpakref"))
        .unwrap_or(file_name)
        .trim();

    if stem.is_empty() {
        return true;
    }

    // Explicit allowlist for short well-known real apps (e.g. 7z, 7zFM, vlc, obs, git, mpv, cs2)
    const SHORT_ALLOW: &[&str] = &[
        "7z", "7zfm", "7zg", "vlc", "obs", "mpv", "git", "cs2", "arc",
    ];
    if SHORT_ALLOW.contains(&stem) {
        return false;
    }

    // Short garbage names (< 3 chars)
    if stem.len() < 3 {
        return true;
    }

    // Exact Windows OS / system / helper / uninstaller / Sentinel-self blocklist
    const NOISE_STEMS: &[&str] = &[
        "svchost",
        "conhost",
        "cmd",
        "powershell",
        "pwsh",
        "rundll32",
        "msiexec",
        "dllhost",
        "backgroundtaskhost",
        "searchhost",
        "searchapp",
        "searchindexer",
        "searchprotocolhost",
        "searchfilterhost",
        "runtimebroker",
        "smartscreen",
        "msedgewebview2",
        "webview2",
        "consent",
        "taskhostw",
        "taskhost",
        "sihost",
        "ctfmon",
        "explorer",
        "notepad",
        "calc",
        "calculator",
        "calculatorapp",
        "tiworker",
        "trustedinstaller",
        "wermgr",
        "werfault",
        "werfaultsecure",
        "compattelrunner",
        "smartctl",
        "ssd-sentinel",
        "ssdsentinel",
        "diskspd",
        "fio",
        "setup_temp",
        "unins000",
        "unins001",
        "unins002",
        "uninstall",
        "uninst",
        "uninstaller",
        "au_",
        "setup",
        "install",
        "installer",
        "update",
        "updater",
        "helper",
        "crashhandler",
        "crashreporter",
        "crashpad_handler",
        "bugsplat",
        "elevation_service",
        "regsvr32",
        "regedit",
        "reg",
        "netsh",
        "ping",
        "ipconfig",
        "taskkill",
        "tasklist",
        "schtasks",
        "wmic",
        "wscript",
        "cscript",
        "mshta",
        "certutil",
        "bitsadmin",
        "attrib",
        "xcopy",
        "robocopy",
        "icacls",
        "takeown",
        "chkdsk",
        "defrag",
        "diskpart",
        "dism",
        "sfc",
        "bcdedit",
        "vssadmin",
        "wevtutil",
        "spoolsv",
        "lsass",
        "csrss",
        "smss",
        "wininit",
        "winlogon",
        "services",
        "dwm",
        "fontdrvhost",
        "audiodg",
        "dashost",
        "shellexperiencehost",
        "startmenuexperiencehost",
        "lockapp",
        "textinputhost",
        "applicationframehost",
        "systemsettings",
        "systemsettingsbroker",
        "widgetservice",
        "widgets",
        "phoneexperiencehost",
        "yourphone",
        "gamebar",
        "gamebarpresencewriter",
        "gamingservices",
        "gamingservicesnet",
        "securityhealthsystray",
        "securityhealthservice",
        "msmpeng",
        "mpcmdrun",
        "nisrv",
        "mousocoreworker",
        "usoclient",
        "wuauclt",
        "waasmedicagent",
        "sedlauncher",
        "musnotification",
        "musnotifyicon",
        "pickerhost",
        "openwith",
        "prevhost",
        "printdialog",
        "splwow64",
        "mmc",
        "perfmon",
        "resmon",
        "taskmgr",
        "dxdiag",
        "msinfo32",
        "cleanmgr",
        "mstsc",
        "snippingtool",
        "screenclippinghost",
        "filecoauth",
        "onedrivesetup",
        "microsoftedgeupdate",
        "googleupdate",
        "googlecrashhandler",
        "googlecrashhandler64",
        "software_reporter_tool",
        "chrome_proxy",
        "notification_helper",
        "vcredist",
        "vc_redist",
        "dxsetup",
        "directxsetup",
        "webview2setup",
        "microsoftedgewebview2setup",
        "dotnetfx",
        "windowsupdatebox",
        "wusa",
        "wuapihost",
        "mobiledataservice",
        "sdiagnhost",
        "sppsvc",
        "slui",
        "tzsync",
        "upfc",
        "uhssvc",
        "provtool",
        "mdmagent",
        "omadmclient",
        "devicecensus",
        "clipup",
        "cloudexperiencehostbroker",
        "accountsbroker",
        "tokenbroker",
        "useroobebroker",
        "wwahost",
        "winrtutil",
        "appxdeploymentclient",
        "appinstaller",
    ];
    if NOISE_STEMS.contains(&stem) {
        return true;
    }

    const NOISE_PREFIXES: &[&str] = &[
        "unins0",
        "uninst",
        "uninstall",
        "setup_temp",
        "is-",
        "~nsu",
        "nsd",
        "7zs",
        "vc_redist",
        "vcredist",
        "windows10.0-kb",
        "windows11.0-kb",
        "crashpad",
        "crashreport",
        "crashhandler",
        "bugreport",
        "microsoftedgeupdate",
        "googleupdate",
        "googlecrashhandler",
        "adobeupdate",
        "waasmedic",
        "ssd-sentinel",
    ];
    if NOISE_PREFIXES.iter().any(|p| stem.starts_with(p)) {
        return true;
    }

    // Windows KB update packages (`kb5034441`)
    if stem.starts_with("kb") && stem.len() >= 7 && stem[2..].chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    // GUID / hex-hash / pure-digit garbage names (`{a1b2...}`, `4f8a9c12`, `12345678`)
    let stripped_punct: String = stem.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if stripped_punct.len() < 3 {
        return true;
    }
    let alpha_count = stripped_punct
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .count();
    let digit_count = stripped_punct
        .chars()
        .filter(|c| c.is_ascii_digit())
        .count();
    if alpha_count < 2 {
        return true;
    }
    let is_all_hex = stripped_punct.chars().all(|c| c.is_ascii_hexdigit());
    if is_all_hex && stripped_punct.len() >= 8 && digit_count >= 2 {
        return true;
    }
    if stem.starts_with('{') && stem.ends_with('}') {
        return true;
    }
    if stem.matches('-').count() >= 4 && is_all_hex {
        return true;
    }

    if let Some(name) = friendly_name {
        let nl = name.trim().to_ascii_lowercase();
        if nl.contains("windows shell experience")
            || nl.contains("microsoft edge webview2")
            || nl.contains("ssd sentinel")
            || nl.starts_with("microsoft visual c++")
            || nl.starts_with("update for microsoft windows")
            || nl.starts_with("security update for microsoft")
        {
            return true;
        }
    }

    false
}

/// Known popular Windows apps/games catalog:
/// `(alias_key, canonical_key, display_name, category, winget_id)`.
const KNOWN_APPS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "spotify",
        "spotify",
        "Spotify",
        "Uygulama",
        "Spotify.Spotify",
    ),
    (
        "discord",
        "discord",
        "Discord",
        "Uygulama",
        "Discord.Discord",
    ),
    (
        "discordptb",
        "discord",
        "Discord",
        "Uygulama",
        "Discord.Discord",
    ),
    (
        "discordcanary",
        "discord",
        "Discord",
        "Uygulama",
        "Discord.Discord",
    ),
    ("vlc", "vlc", "VLC Media Player", "Uygulama", "VideoLAN.VLC"),
    (
        "obs",
        "obsstudio",
        "OBS Studio",
        "Uygulama",
        "OBSProject.OBSStudio",
    ),
    (
        "obsstudio",
        "obsstudio",
        "OBS Studio",
        "Uygulama",
        "OBSProject.OBSStudio",
    ),
    ("steam", "steam", "Steam", "Oyun", "Valve.Steam"),
    (
        "epicgameslauncher",
        "epicgames",
        "Epic Games Launcher",
        "Oyun",
        "EpicGames.EpicGamesLauncher",
    ),
    (
        "epicgames",
        "epicgames",
        "Epic Games Launcher",
        "Oyun",
        "EpicGames.EpicGamesLauncher",
    ),
    (
        "epicinstaller",
        "epicgames",
        "Epic Games Launcher",
        "Oyun",
        "EpicGames.EpicGamesLauncher",
    ),
    (
        "blender",
        "blender",
        "Blender",
        "Uygulama",
        "BlenderFoundation.Blender",
    ),
    (
        "code",
        "vscode",
        "Visual Studio Code",
        "Araç",
        "Microsoft.VisualStudioCode",
    ),
    (
        "vscode",
        "vscode",
        "Visual Studio Code",
        "Araç",
        "Microsoft.VisualStudioCode",
    ),
    (
        "visualstudiocode",
        "vscode",
        "Visual Studio Code",
        "Araç",
        "Microsoft.VisualStudioCode",
    ),
    (
        "telegram",
        "telegram",
        "Telegram Desktop",
        "Uygulama",
        "Telegram.TelegramDesktop",
    ),
    (
        "telegramdesktop",
        "telegram",
        "Telegram Desktop",
        "Uygulama",
        "Telegram.TelegramDesktop",
    ),
    (
        "firefox",
        "firefox",
        "Mozilla Firefox",
        "Uygulama",
        "Mozilla.Firefox",
    ),
    (
        "chrome",
        "chrome",
        "Google Chrome",
        "Uygulama",
        "Google.Chrome",
    ),
    (
        "googlechrome",
        "chrome",
        "Google Chrome",
        "Uygulama",
        "Google.Chrome",
    ),
    ("brave", "brave", "Brave Browser", "Uygulama", "Brave.Brave"),
    ("opera", "opera", "Opera", "Uygulama", "Opera.Opera"),
    (
        "operagx",
        "operagx",
        "Opera GX",
        "Uygulama",
        "Opera.OperaGX",
    ),
    ("7z", "7zip", "7-Zip", "Araç", "7zip.7zip"),
    ("7zfm", "7zip", "7-Zip", "Araç", "7zip.7zip"),
    ("7zip", "7zip", "7-Zip", "Araç", "7zip.7zip"),
    ("winrar", "winrar", "WinRAR", "Araç", "RARLab.WinRAR"),
    (
        "notepad",
        "notepadpp",
        "Notepad++",
        "Araç",
        "Notepad++.Notepad++",
    ),
    (
        "notepadpp",
        "notepadpp",
        "Notepad++",
        "Araç",
        "Notepad++.Notepad++",
    ),
    (
        "notepadplusplus",
        "notepadpp",
        "Notepad++",
        "Araç",
        "Notepad++.Notepad++",
    ),
    ("gimp", "gimp", "GIMP", "Uygulama", "GIMP.GIMP"),
    (
        "audacity",
        "audacity",
        "Audacity",
        "Uygulama",
        "Audacity.Audacity",
    ),
    (
        "handbrake",
        "handbrake",
        "HandBrake",
        "Uygulama",
        "HandBrake.HandBrake",
    ),
    (
        "qbittorrent",
        "qbittorrent",
        "qBittorrent",
        "Araç",
        "qBittorrent.qBittorrent",
    ),
    ("rufus", "rufus", "Rufus", "Araç", "Rufus.Rufus"),
    (
        "everything",
        "everything",
        "Everything",
        "Araç",
        "voidtools.Everything",
    ),
    ("hwinfo", "hwinfo", "HWiNFO", "Araç", "REALiX.HWiNFO"),
    ("cpuz", "cpuz", "CPU-Z", "Araç", "CPUID.CPU-Z"),
    ("gpuz", "gpuz", "GPU-Z", "Araç", "TechPowerUp.GPU-Z"),
    (
        "crystaldiskinfo",
        "crystaldiskinfo",
        "CrystalDiskInfo",
        "Araç",
        "CrystalDewWorld.CrystalDiskInfo",
    ),
    (
        "crystaldiskmark",
        "crystaldiskmark",
        "CrystalDiskMark",
        "Araç",
        "CrystalDewWorld.CrystalDiskMark",
    ),
    (
        "msiafterburner",
        "msiafterburner",
        "MSI Afterburner",
        "Araç",
        "Guru3D.Afterburner",
    ),
    (
        "afterburner",
        "msiafterburner",
        "MSI Afterburner",
        "Araç",
        "Guru3D.Afterburner",
    ),
    ("notion", "notion", "Notion", "Uygulama", "Notion.Notion"),
    ("figma", "figma", "Figma", "Uygulama", "Figma.Figma"),
    (
        "slack",
        "slack",
        "Slack",
        "Uygulama",
        "SlackTechnologies.Slack",
    ),
    ("zoom", "zoom", "Zoom", "Uygulama", "Zoom.Zoom"),
    ("anydesk", "anydesk", "AnyDesk", "Araç", "AnyDesk.AnyDesk"),
    (
        "teamviewer",
        "teamviewer",
        "TeamViewer",
        "Araç",
        "TeamViewer.TeamViewer",
    ),
    ("putty", "putty", "PuTTY", "Araç", "PuTTY.PuTTY"),
    ("winscp", "winscp", "WinSCP", "Araç", "WinSCP.WinSCP"),
    (
        "filezilla",
        "filezilla",
        "FileZilla",
        "Araç",
        "TimKosse.FileZilla.Client",
    ),
    ("postman", "postman", "Postman", "Araç", "Postman.Postman"),
    (
        "docker",
        "dockerdesktop",
        "Docker Desktop",
        "Araç",
        "Docker.DockerDesktop",
    ),
    (
        "dockerdesktop",
        "dockerdesktop",
        "Docker Desktop",
        "Araç",
        "Docker.DockerDesktop",
    ),
    ("git", "git", "Git", "Araç", "Git.Git"),
    (
        "libreoffice",
        "libreoffice",
        "LibreOffice",
        "Uygulama",
        "TheDocumentFoundation.LibreOffice",
    ),
    (
        "soffice",
        "libreoffice",
        "LibreOffice",
        "Uygulama",
        "TheDocumentFoundation.LibreOffice",
    ),
    (
        "sumatrapdf",
        "sumatrapdf",
        "SumatraPDF",
        "Uygulama",
        "SumatraPDF.SumatraPDF",
    ),
    (
        "paintdotnet",
        "paintdotnet",
        "Paint.NET",
        "Uygulama",
        "dotPDN.PaintDotNet",
    ),
    ("sharex", "sharex", "ShareX", "Araç", "ShareX.ShareX"),
    (
        "powertoys",
        "powertoys",
        "Microsoft PowerToys",
        "Araç",
        "Microsoft.PowerToys",
    ),
    (
        "malwarebytes",
        "malwarebytes",
        "Malwarebytes",
        "Araç",
        "Malwarebytes.Malwarebytes",
    ),
    (
        "ccleaner",
        "ccleaner",
        "CCleaner",
        "Araç",
        "Piriform.CCleaner",
    ),
    (
        "revouninstaller",
        "revouninstaller",
        "Revo Uninstaller",
        "Araç",
        "VSRevoGroup.RevoUninstaller",
    ),
    (
        "virtualbox",
        "virtualbox",
        "Oracle VirtualBox",
        "Araç",
        "Oracle.VirtualBox",
    ),
    (
        "wireshark",
        "wireshark",
        "Wireshark",
        "Araç",
        "WiresharkFoundation.Wireshark",
    ),
    (
        "potplayer",
        "potplayer",
        "PotPlayer",
        "Uygulama",
        "Daum.PotPlayer",
    ),
    (
        "potplayermini",
        "potplayer",
        "PotPlayer",
        "Uygulama",
        "Daum.PotPlayer",
    ),
    ("mpv", "mpv", "mpv", "Uygulama", "mpv.net"),
    (
        "ubisoftconnect",
        "ubisoftconnect",
        "Ubisoft Connect",
        "Oyun",
        "Ubisoft.Connect",
    ),
    (
        "upc",
        "ubisoftconnect",
        "Ubisoft Connect",
        "Oyun",
        "Ubisoft.Connect",
    ),
    (
        "eadesktop",
        "eadesktop",
        "EA app",
        "Oyun",
        "ElectronicArts.EADesktop",
    ),
    (
        "battlenet",
        "battlenet",
        "Battle.net",
        "Oyun",
        "Blizzard.BattleNet",
    ),
    (
        "riotclientservices",
        "riotclient",
        "Riot Client",
        "Oyun",
        "RiotGames.Valorant.EU",
    ),
    (
        "riotclient",
        "riotclient",
        "Riot Client",
        "Oyun",
        "RiotGames.Valorant.EU",
    ),
    (
        "galaxyclient",
        "goggalaxy",
        "GOG Galaxy",
        "Oyun",
        "GOG.Galaxy",
    ),
    ("goggalaxy", "goggalaxy", "GOG Galaxy", "Oyun", "GOG.Galaxy"),
    (
        "playnite",
        "playnite",
        "Playnite",
        "Oyun",
        "Playnite.Playnite",
    ),
    (
        "curseforge",
        "curseforge",
        "CurseForge",
        "Oyun",
        "Overwolf.CurseForge",
    ),
    (
        "prismlauncher",
        "prismlauncher",
        "Prism Launcher",
        "Oyun",
        "PrismLauncher.PrismLauncher",
    ),
    (
        "minecraft",
        "minecraft",
        "Minecraft Launcher",
        "Oyun",
        "Mojang.MinecraftLauncher",
    ),
    (
        "robloxplayerbeta",
        "roblox",
        "Roblox",
        "Oyun",
        "Roblox.Roblox",
    ),
    ("roblox", "roblox", "Roblox", "Oyun", "Roblox.Roblox"),
    (
        "keepassxc",
        "keepassxc",
        "KeePassXC",
        "Araç",
        "KeePassXCTeam.KeePassXC",
    ),
    (
        "bitwarden",
        "bitwarden",
        "Bitwarden",
        "Araç",
        "Bitwarden.Bitwarden",
    ),
    (
        "tailscale",
        "tailscale",
        "Tailscale",
        "Araç",
        "tailscale.tailscale",
    ),
    (
        "wireguard",
        "wireguard",
        "WireGuard",
        "Araç",
        "WireGuard.WireGuard",
    ),
    ("parsec", "parsec", "Parsec", "Uygulama", "Parsec.Parsec"),
    (
        "moonlight",
        "moonlight",
        "Moonlight",
        "Oyun",
        "MoonlightGameStreamingProject.Moonlight",
    ),
    (
        "obsidian",
        "obsidian",
        "Obsidian",
        "Uygulama",
        "Obsidian.Obsidian",
    ),
    ("krita", "krita", "Krita", "Uygulama", "KDE.Krita"),
    (
        "inkscape",
        "inkscape",
        "Inkscape",
        "Uygulama",
        "Inkscape.Inkscape",
    ),
    (
        "kdenlive",
        "kdenlive",
        "Kdenlive",
        "Uygulama",
        "KDE.Kdenlive",
    ),
    (
        "davinciresolve",
        "davinciresolve",
        "DaVinci Resolve",
        "Uygulama",
        "Blackmagic.DaVinciResolve",
    ),
    (
        "unityhub",
        "unityhub",
        "Unity Hub",
        "Araç",
        "Unity.UnityHub",
    ),
    (
        "godot",
        "godot",
        "Godot Engine",
        "Araç",
        "GodotEngine.GodotEngine",
    ),
    (
        "thunderbird",
        "thunderbird",
        "Mozilla Thunderbird",
        "Uygulama",
        "Mozilla.Thunderbird",
    ),
    (
        "sublimetext",
        "sublimetext",
        "Sublime Text",
        "Araç",
        "SublimeHQ.SublimeText.4",
    ),
    (
        "wiztree",
        "wiztree",
        "WizTree",
        "Araç",
        "AntibodySoftware.WizTree",
    ),
    (
        "treesize",
        "treesize",
        "TreeSize Free",
        "Araç",
        "JAMSoftware.TreeSize.Free",
    ),
    ("ventoy", "ventoy", "Ventoy", "Araç", "Ventoy.Ventoy"),
];

pub fn lookup_known_app(
    canonical_key: &str,
) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    KNOWN_APPS
        .iter()
        .find(|&&(alias, _, _, _, _)| alias == canonical_key)
        .map(|&(_, canon, display, cat, winget)| (canon, display, cat, winget))
}

fn strip_trailing_app_modifiers(raw: &str) -> String {
    let mut s = raw.trim().to_owned();
    // Strip common file extensions
    for ext in [".exe", ".msi", ".msix", ".lnk", ".pf"] {
        if s.to_ascii_lowercase().ends_with(ext) {
            s.truncate(s.len() - ext.len());
        }
    }
    // Strip trailing parentheticals like `(64-bit)` or `(x64)`
    if let Some(idx) = s.rfind('(') {
        if s.ends_with(')') && idx >= 3 {
            s = s[..idx].trim().to_owned();
        }
    }
    // Iteratively strip trailing version numbers, platform tags, and installer suffixes
    const SUFFIX_WORDS: &[&str] = &[
        "win64-shipping",
        "win32-shipping",
        "shipping",
        "webinstaller",
        "websetup",
        "installer",
        "install",
        "setup",
        "bootstrapper",
        "downloader",
        "portable",
        "launcher",
        "desktop",
        "windows",
        "win64",
        "win32",
        "amd64",
        "arm64",
        "64bit",
        "32bit",
        "x86_64",
        "x64",
        "x86",
        "64",
        "32",
        "win",
    ];

    for _ in 0..5 {
        let trimmed = s.trim_end_matches([' ', '-', '_', '.']).to_owned();
        let lower = trimmed.to_ascii_lowercase();
        let mut changed = false;

        // 1. Delimited version tokens at end like `-3.0.21`, `_v2.4`, ` 1.2.3`
        if let Some(sep_pos) = trimmed.rfind(['-', '_', ' ']) {
            let tail = &trimmed[sep_pos + 1..];
            let tail_v = tail.trim_start_matches(['v', 'V']);
            let is_ver = !tail_v.is_empty()
                && tail_v.chars().all(|c| c.is_ascii_digit() || c == '.')
                && tail_v.chars().any(|c| c.is_ascii_digit());
            if is_ver && sep_pos >= 2 {
                let candidate = trimmed[..sep_pos].trim_end_matches([' ', '-', '_', '.']);
                if candidate
                    .chars()
                    .filter(|c| c.is_ascii_alphabetic())
                    .count()
                    >= 2
                {
                    s = candidate.to_owned();
                    changed = true;
                }
            }
        }
        if changed {
            continue;
        }

        // 2. Trailing modifier words (either delimited or CamelCase concatenated like `SpotifySetup`)
        for &word in SUFFIX_WORDS {
            if lower.ends_with(word) && trimmed.len() > word.len() {
                let cut = trimmed.len() - word.len();
                let before = trimmed[..cut].trim_end_matches([' ', '-', '_', '.']);
                let delimited = matches!(trimmed.as_bytes()[cut - 1], b' ' | b'-' | b'_' | b'.');
                let camel_boundary = trimmed.as_bytes()[cut].is_ascii_uppercase()
                    && trimmed.as_bytes()[cut - 1].is_ascii_lowercase();
                let digits_suffix = matches!(word, "64" | "32")
                    && trimmed.as_bytes()[cut - 1].is_ascii_alphabetic();
                let compound_setup = matches!(
                    word,
                    "setup" | "installer" | "install" | "websetup" | "portable"
                ) && before.len() >= 3;
                if (delimited || camel_boundary || digits_suffix || compound_setup)
                    && before.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 2
                {
                    s = before.to_owned();
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            s = trimmed;
            break;
        }
    }
    s
}

/// Computes a normalized deduplication key for an app name / exe / AppData folder so
/// `SpotifySetup.exe`, `Spotify.exe`, and `AppData\Roaming\Spotify` all map to `"spotify"`.
pub fn canonical_app_key(name_or_path: &str) -> String {
    let base = if name_or_path.contains('\\') || name_or_path.contains('/') {
        let p = name_or_path.replace('/', "\\");
        let file_part = p.rsplit('\\').next().unwrap_or(&p);
        let stem_clean = strip_trailing_app_modifiers(file_part);
        let stem_lower = stem_clean.to_ascii_lowercase();
        // If exe filename is generic (`launcher`, `client`, `game`, `app`, `start`, `main`),
        // prefer the meaningful parent app directory!
        if matches!(
            stem_lower.as_str(),
            "" | "app"
                | "main"
                | "start"
                | "play"
                | "run"
                | "game"
                | "client"
                | "launcher"
                | "bin"
                | "bin64"
                | "win64"
                | "x64"
        ) {
            extract_app_folder_from_path(&p).unwrap_or(stem_clean)
        } else {
            stem_clean
        }
    } else {
        strip_trailing_app_modifiers(name_or_path)
    };

    let mut key: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    // Preserve `++` distinction for `Notepad++` vs Windows `notepad`
    if name_or_path.contains("++") && key == "notepad" {
        key = "notepadpp".into();
    }
    if let Some((canon, _, _, _)) = lookup_known_app(&key) {
        return canon.to_owned();
    }
    key
}

fn is_generic_dir_segment(seg_lower: &str) -> bool {
    matches!(
        seg_lower,
        "bin"
            | "bin64"
            | "bin32"
            | "binaries"
            | "x64"
            | "x86"
            | "win64"
            | "win32"
            | "64bit"
            | "32bit"
            | "release"
            | "debug"
            | "current"
            | "application"
            | "program"
            | "programs"
            | "program files"
            | "program files (x86)"
            | "appdata"
            | "roaming"
            | "local"
            | "locallow"
            | "programdata"
            | "users"
            | "downloads"
            | "desktop"
            | "documents"
            | "steamapps"
            | "common"
            | "games"
            | "epic games"
            | "riot games"
            | "xboxgames"
            | "gog games"
            | "steamlibrary"
    ) || seg_lower.starts_with("app-")
}

fn extract_app_folder_from_path(path: &str) -> Option<String> {
    let norm = path.replace('/', "\\");
    let parts: Vec<&str> = norm.split('\\').filter(|s| !s.is_empty()).collect();
    if parts.len() < 3 {
        return None;
    }
    // Walk upwards from parent directory skipping generic `bin`, `x64`, `app-1.0.x`, etc.
    for &seg in parts[1..parts.len() - 1].iter().rev() {
        let sl = seg.to_ascii_lowercase();
        if !is_generic_dir_segment(&sl) && seg.len() >= 2 {
            return Some(seg.to_owned());
        }
    }
    None
}

/// Derives the app's install directory from an executable path, excluding `Downloads`/`Desktop`.
pub fn extract_install_dir(exe_path: &str) -> Option<String> {
    if exe_path.starts_with('/') {
        let parent = Path::new(exe_path).parent()?;
        let parent_str = parent.to_string_lossy().into_owned();
        let pl = parent_str.to_ascii_lowercase();
        if parent_str.len() <= 1
            || matches!(
                pl.as_str(),
                "/" | "/bin"
                    | "/sbin"
                    | "/usr"
                    | "/usr/bin"
                    | "/usr/sbin"
                    | "/usr/local/bin"
                    | "/opt"
            )
            || pl.ends_with("/downloads")
            || pl.ends_with("/desktop")
            || pl.ends_with("/documents")
            || pl.contains("/.local/share/trash")
        {
            return None;
        }
        let mut cur = parent;
        for _ in 0..3 {
            let Some(name) = cur.file_name().and_then(|s| s.to_str()) else {
                break;
            };
            let nl = name.to_ascii_lowercase();
            if matches!(
                nl.as_str(),
                "bin" | "bin64" | "bin32" | "lib" | "lib64" | "x64" | "x86"
            ) {
                if let Some(up) = cur.parent() {
                    let up_s = up.to_string_lossy();
                    if up_s.len() > 1 && up_s != "/usr" && up_s != "/usr/local" {
                        cur = up;
                        continue;
                    }
                }
            }
            break;
        }
        let out = cur.to_string_lossy().into_owned();
        return (out.len() > 1).then_some(out);
    }
    let norm = exe_path.replace('/', "\\");
    extract_drive_letter(&norm)?;
    let parent = Path::new(&norm).parent()?;
    let parent_str = parent.to_string_lossy().into_owned();
    let pl = parent_str.to_ascii_lowercase();
    // Skip drive root (`C:\`) or generic user folders (`Downloads`, `Desktop`, `Documents`)
    if parent_str.len() <= 3
        || pl.ends_with("\\downloads")
        || pl.ends_with("\\desktop")
        || pl.ends_with("\\documents")
        || pl.ends_with("\\temp")
        || pl.ends_with("\\program files")
        || pl.ends_with("\\program files (x86)")
        || pl.contains("$recycle.bin")
    {
        return None;
    }
    // Peel off generic binary leaf subdirs like `\bin\x64` or `\app-1.0.9015`
    let mut cur = parent;
    for _ in 0..3 {
        let Some(name) = cur.file_name().and_then(|s| s.to_str()) else {
            break;
        };
        let nl = name.to_ascii_lowercase();
        if matches!(
            nl.as_str(),
            "bin"
                | "bin64"
                | "bin32"
                | "binaries"
                | "x64"
                | "x86"
                | "win64"
                | "win32"
                | "64bit"
                | "32bit"
                | "current"
        ) || nl.starts_with("app-")
        {
            if let Some(up) = cur.parent() {
                if up.to_string_lossy().len() > 3 {
                    cur = up;
                    continue;
                }
            }
        }
        break;
    }
    let out = cur.to_string_lossy().into_owned();
    if out.len() <= 3 {
        None
    } else {
        Some(out)
    }
}

/// Classifies a deleted app into `"Oyun"` | `"Uygulama"` | `"Araç"`.
pub fn classify_app_category(
    canonical_key: &str,
    exe_path: &str,
    name: &str,
    is_game_hint: bool,
) -> &'static str {
    if let Some((_, _, cat, _)) = lookup_known_app(canonical_key) {
        return cat;
    }
    let lower_path = exe_path.to_ascii_lowercase();
    let lower_name = name.to_ascii_lowercase();
    const GAME_MARKERS: &[&str] = &[
        "\\steamapps\\",
        "\\steamlibrary\\",
        "\\epic games\\",
        "\\riot games\\",
        "\\xboxgames\\",
        "\\games\\",
        "\\oyun",
        "\\ubisoft",
        "\\ea games\\",
        "\\ea desktop\\",
        "\\gog games\\",
        "\\battlenet\\",
        "\\blizzard\\",
        "\\rockstar games\\",
        "-win64-shipping",
        "unityplayer",
        "unrealengine",
    ];
    if is_game_hint || GAME_MARKERS.iter().any(|m| lower_path.contains(m)) {
        return "Oyun";
    }
    const TOOL_MARKERS: &[&str] = &[
        "\\tools\\",
        "\\utils\\",
        "\\utilities\\",
        "\\sysinternals\\",
        "\\portable\\",
        "uninstaller",
        "benchmark",
        "monitor",
        "cleaner",
        "defrag",
        "partition",
        "backup",
        "recovery",
        "vpn",
        "ssh",
        "ftp",
        "terminal",
        "debugger",
        "compiler",
        "sdk",
        "cli",
        "zip",
        "rar",
        "driver",
    ];
    if TOOL_MARKERS
        .iter()
        .any(|m| lower_path.contains(m) || lower_name.contains(m))
    {
        return "Araç";
    }
    "Uygulama"
}

/// Returns the most accurate `winget` command (`winget install --id ... -e` for known apps,
/// otherwise `winget search "<name>"`).
pub fn winget_command_for_app(canonical_key: &str, display_name: &str) -> String {
    if let Some((_, _, _, winget_id)) = lookup_known_app(canonical_key) {
        return format!("winget install --id {winget_id} -e");
    }
    let clean = display_name.replace('"', "").trim().to_owned();
    format!("winget search \"{clean}\"")
}

/// Returns a native reinstall/search command for Linux package managers when the target
/// lives on a POSIX mount/path, otherwise preserves `winget_command_for_app`.
pub fn reinstall_command_for_app(
    canonical_key: &str,
    display_name: &str,
    exe_path: &str,
    selected_letter: &str,
) -> String {
    if !selected_letter.starts_with('/') && !exe_path.starts_with('/') {
        return winget_command_for_app(canonical_key, display_name);
    }
    let clean_name = display_name.replace(['"', '\''], "").trim().to_owned();
    let pkg_slug: String = canonical_key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let pkg = if pkg_slug.len() >= 2 {
        pkg_slug.as_str()
    } else {
        clean_name.as_str()
    };
    if Path::new("/usr/bin/pacman").is_file() || Path::new("/bin/pacman").is_file() {
        return format!("sudo pacman -S {pkg}  # or: pacman -Ss \"{clean_name}\"");
    }
    if Path::new("/usr/bin/apt-get").is_file() || Path::new("/usr/bin/apt").is_file() {
        return format!("sudo apt install {pkg}  # or: apt search \"{clean_name}\"");
    }
    if Path::new("/usr/bin/dnf").is_file() {
        return format!("sudo dnf install {pkg}  # or: dnf search \"{clean_name}\"");
    }
    if Path::new("/usr/bin/zypper").is_file() {
        return format!("sudo zypper install {pkg}  # or: zypper search \"{clean_name}\"");
    }
    format!("flatpak search \"{clean_name}\"")
}

/// Returns a clean user-facing display name for an app, preferring known catalog names or
/// stripping ugly installer/arch suffixes.
pub fn clean_app_display_name(raw_name: &str, exe_path: &str) -> String {
    let key = canonical_app_key(if raw_name.trim().len() >= 2 {
        raw_name
    } else {
        exe_path
    });
    if let Some((_, known_display, _, _)) = lookup_known_app(&key) {
        return known_display.to_owned();
    }
    let base = if raw_name.trim().len() >= 2 {
        strip_trailing_app_modifiers(raw_name)
    } else {
        let stem = Path::new(exe_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Uygulama");
        let stripped = strip_trailing_app_modifiers(stem);
        let sl = stripped.to_ascii_lowercase();
        if matches!(
            sl.as_str(),
            "" | "app" | "main" | "start" | "play" | "game" | "client" | "launcher"
        ) {
            extract_app_folder_from_path(exe_path).unwrap_or(stripped)
        } else {
            stripped
        }
    };
    let spaced = base.replace(['_', '-'], " ");
    let collapsed: Vec<&str> = spaced.split_whitespace().collect();
    if collapsed.is_empty() {
        return "Uygulama".into();
    }
    let joined = collapsed.join(" ");
    // Title-case ALL-UPPERCASE Prefetch names like `DISCORD` -> `Discord`
    if joined.len() >= 4
        && joined
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .all(|c| c.is_ascii_uppercase())
    {
        let mut chars = joined.chars();
        if let Some(first) = chars.next() {
            return format!(
                "{}{}",
                first.to_ascii_uppercase(),
                chars.as_str().to_ascii_lowercase()
            );
        }
    }
    joined
}

#[cfg(windows)]
fn clean_exe_from_command(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if let Some(rest) = trimmed.strip_prefix('"') {
        rest.split('"').next().unwrap_or("")
    } else if let Some(idx) = trimmed.to_ascii_lowercase().find(".exe") {
        &trimmed[..idx + 4]
    } else {
        trimmed.split(',').next().unwrap_or("")
    };
    let clean = candidate.trim().trim_matches('"');
    if clean.len() >= 6 && clean.to_ascii_lowercase().ends_with(".exe") {
        Some(clean.to_owned())
    } else {
        None
    }
}

fn friendly_name_from_path(path: &str) -> String {
    clean_app_display_name("", path)
}

pub fn collect_forensic_sweep(letter: &str) -> ForensicSweep {
    #[cfg(windows)]
    {
        win::collect_windows_forensics(letter)
    }
    #[cfg(target_os = "linux")]
    {
        linux_forensics::collect_linux_forensics(letter)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = letter;
        ForensicSweep::default()
    }
}

pub fn read_usn_deleted_records(letter: &str, max_records: usize) -> Vec<UsnDeletedRecord> {
    #[cfg(windows)]
    {
        win::read_usn_deleted_records(letter, max_records)
    }
    #[cfg(not(windows))]
    {
        let _ = (letter, max_records);
        Vec::new()
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::platform;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetDriveTypeW,
        GetVolumeInformationW, GetVolumePathNamesForVolumeNameW, QueryDosDeviceW, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, HKEY,
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const GENERIC_READ: u32 = 0x8000_0000;
    const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00F4;
    const FSCTL_READ_USN_JOURNAL: u32 = 0x0009_00BB;
    const COMPRESSION_FORMAT_XPRESS_HUFF: u16 = 0x0004;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;

    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetCompressionWorkSpaceSize(
            compression_format_and_engine: u16,
            compress_buffer_work_space_size: *mut u32,
            compress_fragment_work_space_size: *mut u32,
        ) -> i32;
        fn RtlDecompressBufferEx(
            compression_format: u16,
            uncompressed_buffer: *mut u8,
            uncompressed_buffer_size: u32,
            compressed_buffer: *const u8,
            compressed_buffer_size: u32,
            final_uncompressed_size: *mut u32,
            work_space: *mut c_void,
        ) -> i32;
    }

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

    struct VolumeFindHandle(HANDLE);
    impl Drop for VolumeFindHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    FindVolumeClose(self.0);
                }
            }
        }
    }

    struct RegKey(HKEY);
    impl Drop for RegKey {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    RegCloseKey(self.0);
                }
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn open_reg(root: HKEY, subkey: &str) -> Option<RegKey> {
        let w = wide(subkey);
        let mut h: HKEY = null_mut();
        if unsafe { RegOpenKeyExW(root, w.as_ptr(), 0, KEY_READ, &mut h) } == 0 && !h.is_null() {
            Some(RegKey(h))
        } else {
            None
        }
    }

    fn reg_query_bytes(hkey: HKEY, name: &str) -> Option<(u32, Vec<u8>)> {
        let w = wide(name);
        let mut val_type = 0u32;
        let mut len = 0u32;
        if unsafe {
            RegQueryValueExW(
                hkey,
                w.as_ptr(),
                null_mut(),
                &mut val_type,
                null_mut(),
                &mut len,
            )
        } != 0
            || len == 0
            || len > 32 * 1024 * 1024
        {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        if unsafe {
            RegQueryValueExW(
                hkey,
                w.as_ptr(),
                null_mut(),
                &mut val_type,
                buf.as_mut_ptr(),
                &mut len,
            )
        } != 0
        {
            return None;
        }
        buf.truncate(len as usize);
        Some((val_type, buf))
    }

    fn reg_query_string(hkey: HKEY, name: &str) -> Option<String> {
        let (vt, bytes) = reg_query_bytes(hkey, name)?;
        if vt == 1 || vt == 2 {
            let s = decode_utf16le(&bytes).trim().to_owned();
            if !s.is_empty() {
                return Some(s);
            }
        }
        None
    }

    fn reg_query_u32(hkey: HKEY, name: &str) -> Option<u32> {
        let (vt, bytes) = reg_query_bytes(hkey, name)?;
        if vt == 4 && bytes.len() >= 4 {
            Some(u32::from_le_bytes(bytes[0..4].try_into().ok()?))
        } else {
            None
        }
    }

    fn reg_enum_subkeys(hkey: HKEY) -> Vec<String> {
        let mut out = Vec::new();
        let mut idx = 0u32;
        loop {
            let mut buf = [0u16; 260];
            let mut len = buf.len() as u32;
            let rc = unsafe {
                RegEnumKeyExW(
                    hkey,
                    idx,
                    buf.as_mut_ptr(),
                    &mut len,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null_mut(),
                )
            };
            if rc != 0 {
                break;
            }
            idx += 1;
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
            if out.len() >= 4096 {
                break;
            }
        }
        out
    }

    fn volume_serial(root_path: &str) -> Option<u32> {
        let w = wide(root_path);
        let mut serial = 0u32;
        let ok = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                null_mut(),
                0,
                &mut serial,
                null_mut(),
                null_mut(),
                null_mut(),
                0,
            )
        };
        (ok != 0 && serial != 0).then_some(serial)
    }

    fn build_volume_map() -> VolumeMap {
        let sys_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let mut map = VolumeMap {
            device_to_drive: Vec::new(),
            guid_to_drive: HashMap::new(),
            serial_to_drive: HashMap::new(),
            system_drive: sys_drive,
        };

        // 1. QueryDosDeviceW + volume_serial for every mounted drive letter
        for ch in 'A'..='Z' {
            let drive = format!("{ch}:");
            let dev = wide(&drive);
            let mut out = [0u16; 512];
            let n = unsafe { QueryDosDeviceW(dev.as_ptr(), out.as_mut_ptr(), out.len() as u32) };
            if n > 0 {
                let s = decode_utf16le(unsafe {
                    std::slice::from_raw_parts(out.as_ptr().cast::<u8>(), (n as usize) * 2)
                });
                if s.starts_with("\\Device\\") {
                    map.device_to_drive
                        .push((s.to_ascii_lowercase(), drive.clone()));
                }
            }
            let root_w = wide(&format!("{ch}:\\"));
            let dtype = unsafe { GetDriveTypeW(root_w.as_ptr()) };
            if dtype == DRIVE_FIXED || dtype == DRIVE_REMOVABLE {
                if let Some(serial) = volume_serial(&format!("{ch}:\\")) {
                    map.serial_to_drive.insert(serial, drive);
                }
            }
        }
        // Sort longest device path first so HarddiskVolume10 matches before HarddiskVolume1.
        map.device_to_drive
            .sort_by_key(|a| std::cmp::Reverse(a.0.len()));

        // 2. FindFirstVolumeW / GetVolumePathNamesForVolumeNameW for \VOLUME{guid}\...
        let mut vol_buf = [0u16; 260];
        let fh = unsafe { FindFirstVolumeW(vol_buf.as_mut_ptr(), vol_buf.len() as u32) };
        if !fh.is_null() && fh != INVALID_HANDLE_VALUE {
            let vh = VolumeFindHandle(fh);
            loop {
                let vol_name = decode_utf16le(unsafe {
                    std::slice::from_raw_parts(vol_buf.as_ptr().cast::<u8>(), vol_buf.len() * 2)
                });
                let mut paths_buf = [0u16; 512];
                let mut ret_len = 0u32;
                let ok = unsafe {
                    GetVolumePathNamesForVolumeNameW(
                        vol_buf.as_ptr(),
                        paths_buf.as_mut_ptr(),
                        paths_buf.len() as u32,
                        &mut ret_len,
                    )
                };
                if ok != 0 && ret_len >= 3 {
                    let first_mount = decode_utf16le(unsafe {
                        std::slice::from_raw_parts(
                            paths_buf.as_ptr().cast::<u8>(),
                            (ret_len as usize).min(paths_buf.len()) * 2,
                        )
                    });
                    if let Some(letter) = extract_drive_letter(&first_mount) {
                        if let (Some(lb), Some(rb)) = (vol_name.find('{'), vol_name.find('}')) {
                            if lb < rb {
                                let guid = vol_name[lb..=rb].to_ascii_lowercase();
                                map.guid_to_drive.insert(guid, format!("{letter}:"));
                            }
                        }
                    }
                }
                vol_buf.fill(0);
                let next_ok =
                    unsafe { FindNextVolumeW(vh.0, vol_buf.as_mut_ptr(), vol_buf.len() as u32) };
                if next_ok == 0 {
                    break;
                }
            }
        }

        map
    }

    fn decompress_mam_prefetch(raw: &[u8]) -> Option<Vec<u8>> {
        if raw.len() < 16 || &raw[0..3] != b"MAM" {
            return None;
        }
        let uncomp_sz = u32::from_le_bytes(raw[4..8].try_into().ok()?) as usize;
        if !(256..=16 * 1024 * 1024).contains(&uncomp_sz) {
            return None;
        }
        let data_off = if (raw[3] & 0x80) != 0 {
            12usize
        } else {
            8usize
        };
        if data_off >= raw.len() {
            return None;
        }
        let mut ws_buf_sz = 0u32;
        let mut ws_frag_sz = 0u32;
        let st1 = unsafe {
            RtlGetCompressionWorkSpaceSize(
                COMPRESSION_FORMAT_XPRESS_HUFF,
                &mut ws_buf_sz,
                &mut ws_frag_sz,
            )
        };
        if st1 != 0 || ws_frag_sz > 16 * 1024 * 1024 {
            return None;
        }
        let ws_len = (ws_buf_sz.max(ws_frag_sz) as usize).clamp(8192, 16 * 1024 * 1024);
        let mut workspace = vec![0u8; ws_len];
        let mut out = vec![0u8; uncomp_sz];
        let mut final_sz = 0u32;
        let st2 = unsafe {
            RtlDecompressBufferEx(
                COMPRESSION_FORMAT_XPRESS_HUFF,
                out.as_mut_ptr(),
                uncomp_sz as u32,
                raw[data_off..].as_ptr(),
                (raw.len() - data_off) as u32,
                &mut final_sz,
                workspace.as_mut_ptr().cast(),
            )
        };
        if st2 == 0 && (final_sz as usize) >= 224 {
            out.truncate(final_sz as usize);
            Some(out)
        } else {
            None
        }
    }

    fn find_matching_vss_shadows(letter: &str) -> Vec<String> {
        let Some(target_serial) = volume_serial(&format!("{letter}:\\")) else {
            return Vec::new();
        };
        let mut matched = Vec::new();
        for idx in 1..=64u32 {
            let root = format!("\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy{idx}\\");
            if volume_serial(&root) == Some(target_serial) {
                matched.push(root);
            }
        }
        matched.reverse(); // Newest shadow copies first.
        matched
    }

    fn vss_relative_path(shadow_root: &str, abs_path: &str) -> Option<PathBuf> {
        if abs_path.len() < 4 || abs_path.as_bytes()[1] != b':' {
            return None;
        }
        let rel = abs_path[3..].trim_start_matches('\\');
        if rel.is_empty() {
            return None;
        }
        let p = PathBuf::from(format!("{shadow_root}{rel}"));
        p.is_file().then_some(p)
    }

    fn dir_contains_live_exe(dir: &Path, depth: usize) -> bool {
        if depth > 2 || platform::reparse(dir) {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten().take(80) {
            let p = entry.path();
            if platform::reparse(&p) {
                continue;
            }
            if p.is_file() {
                if p.extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case("exe"))
                    == Some(true)
                {
                    let stem = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if !stem.starts_with("unins")
                        && stem != "uninstall"
                        && stem != "crashpad_handler"
                    {
                        return true;
                    }
                }
            } else if p.is_dir() && depth < 2 && dir_contains_live_exe(&p, depth + 1) {
                return true;
            }
        }
        false
    }

    fn dir_size_bounded(dir: &Path, max_depth: usize, max_entries: usize) -> u64 {
        let mut total = 0u64;
        let mut visited = 0usize;
        fn walk(
            dir: &Path,
            depth: usize,
            max_depth: usize,
            max_entries: usize,
            visited: &mut usize,
            total: &mut u64,
        ) {
            if depth > max_depth || *visited >= max_entries || platform::reparse(dir) {
                return;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                if *visited >= max_entries {
                    break;
                }
                *visited += 1;
                let p = entry.path();
                if platform::reparse(&p) {
                    continue;
                }
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        *total = total.saturating_add(meta.len());
                    } else if meta.is_dir() && depth < max_depth {
                        walk(&p, depth + 1, max_depth, max_entries, visited, total);
                    }
                }
            }
        }
        walk(dir, 0, max_depth, max_entries, &mut visited, &mut total);
        total
    }

    struct InstalledAppIndex {
        keys: HashSet<String>,
        exe_stems: HashSet<String>,
    }

    impl InstalledAppIndex {
        fn add_name(&mut self, raw: &str) {
            let k = canonical_app_key(raw);
            if k.len() >= 2 {
                self.keys.insert(k);
            }
        }

        fn add_exe(&mut self, path: &str) {
            if let Some(stem) = Path::new(path).file_stem().and_then(|s| s.to_str()) {
                let sl = stem.to_ascii_lowercase();
                if sl.len() >= 2 {
                    self.exe_stems.insert(sl);
                }
            }
            self.add_name(path);
        }

        fn build(vol_map: &VolumeMap) -> Self {
            let mut idx = Self {
                keys: HashSet::new(),
                exe_stems: HashSet::new(),
            };

            // 1. Live Uninstall entries
            let uninstall_roots = [
                (
                    HKEY_LOCAL_MACHINE,
                    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
                ),
                (
                    HKEY_LOCAL_MACHINE,
                    "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
                ),
                (
                    HKEY_CURRENT_USER,
                    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
                ),
            ];
            for (hive, sub) in uninstall_roots {
                let Some(rk) = open_reg(hive, sub) else {
                    continue;
                };
                for child in reg_enum_subkeys(rk.0) {
                    let Some(ck) = open_reg(hive, &format!("{sub}\\{child}")) else {
                        continue;
                    };
                    let display = reg_query_string(ck.0, "DisplayName");
                    let install_loc = reg_query_string(ck.0, "InstallLocation")
                        .map(|s| s.trim_matches('"').to_owned());
                    let icon_exe = reg_query_string(ck.0, "DisplayIcon")
                        .and_then(|s| clean_exe_from_command(&s));
                    let uninst_exe = reg_query_string(ck.0, "UninstallString")
                        .and_then(|s| clean_exe_from_command(&s));

                    let exe_exists = icon_exe.as_deref().is_some_and(|p| Path::new(p).exists())
                        || uninst_exe.as_deref().is_some_and(|p| Path::new(p).exists());
                    let dir_exists = install_loc
                        .as_deref()
                        .is_some_and(|d| !d.is_empty() && Path::new(d).is_dir());

                    if exe_exists || dir_exists {
                        if let Some(ref d) = display {
                            idx.add_name(d);
                        }
                        if let Some(ref p) = icon_exe {
                            idx.add_exe(p);
                        }
                        if let Some(ref d) = install_loc {
                            if let Some(fname) = Path::new(d).file_name().and_then(|s| s.to_str()) {
                                idx.add_name(fname);
                            }
                        }
                    }
                }
            }

            // 2. Live App Paths
            for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
                let sub = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths";
                let Some(rk) = open_reg(hive, sub) else {
                    continue;
                };
                for child in reg_enum_subkeys(rk.0) {
                    let Some(ck) = open_reg(hive, &format!("{sub}\\{child}")) else {
                        continue;
                    };
                    if let Some(raw) = reg_query_string(ck.0, "") {
                        if let Some(exe) = clean_exe_from_command(&raw) {
                            if Path::new(&exe).exists() {
                                idx.add_exe(&exe);
                                idx.add_name(&child);
                            }
                        }
                    }
                }
            }

            // 3. Top-level directories in Program Files / Games libraries on fixed drives
            let mut drive_letters: HashSet<String> = HashSet::new();
            drive_letters.insert(
                vol_map
                    .system_drive
                    .trim_end_matches(':')
                    .to_ascii_uppercase(),
            );
            for d in vol_map.serial_to_drive.values() {
                drive_letters.insert(d.trim_end_matches(':').to_ascii_uppercase());
            }
            for dl in drive_letters {
                for rel in [
                    "Program Files",
                    "Program Files (x86)",
                    "Games",
                    "SteamLibrary\\steamapps\\common",
                    "Program Files (x86)\\Steam\\steamapps\\common",
                    "Epic Games",
                    "Riot Games",
                    "XboxGames",
                ] {
                    let dir = PathBuf::from(format!("{dl}:\\{rel}"));
                    let Ok(entries) = std::fs::read_dir(&dir) else {
                        continue;
                    };
                    for entry in entries.flatten().take(300) {
                        let p = entry.path();
                        if p.is_dir() {
                            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                                idx.add_name(name);
                            }
                        }
                    }
                }
            }

            // 4. LocalAppData\Programs live installs
            if let Ok(local) = std::env::var("LOCALAPPDATA") {
                let progs = PathBuf::from(format!("{local}\\Programs"));
                if let Ok(entries) = std::fs::read_dir(&progs) {
                    for entry in entries.flatten().take(200) {
                        let p = entry.path();
                        if p.is_dir() && dir_contains_live_exe(&p, 0) {
                            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                                idx.add_name(name);
                            }
                        }
                    }
                }
            }

            idx
        }

        fn is_installed_name(&self, name: &str) -> bool {
            let k = canonical_app_key(name);
            if k.len() >= 2 && self.keys.contains(&k) {
                return true;
            }
            let stem = Path::new(name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(name)
                .to_ascii_lowercase();
            self.exe_stems.contains(&stem)
        }
    }

    fn is_os_or_runtime_appdata_folder(folder_name: &str) -> bool {
        let nl = folder_name.trim().to_ascii_lowercase();
        if nl.len() < 3 || nl.starts_with('.') || nl.starts_with('{') || nl.starts_with('$') {
            return true;
        }
        const BLOCKED: &[&str] = &[
            "microsoft",
            "windows",
            "packages",
            "temp",
            "cache",
            "caches",
            "crashdumps",
            "d3dscache",
            "elevateddiagnostics",
            "fontcache",
            "history",
            "iconcache",
            "inetcache",
            "inetsrv",
            "local",
            "locallow",
            "low",
            "package cache",
            "peerdistrepub",
            "publishers",
            "squirreltemp",
            "temporary internet files",
            "virtualstore",
            "windowsapps",
            "assembly",
            "common files",
            "complus applications",
            "dotnet",
            "drivers",
            "intel",
            "amd",
            "nvidia",
            "nvidia corporation",
            "realtek",
            "qualcomm",
            "broadcom",
            "khronos",
            "vulkan",
            "directx",
            "opengl",
            "pip",
            "npm",
            "npm-cache",
            "yarn",
            "pnpm",
            "cargo",
            "rustup",
            "go-build",
            "node-gyp",
            "nuget",
            "uv",
            "ms-playwright",
            "default",
            "public",
            "all users",
            "usoprivate",
            "usoshared",
            "ssh",
            "gnupg",
            "identities",
            "cef",
            "electron",
            "squirrel",
            "updater",
            "diagnostics",
            "telemetry",
            "logs",
            "backup",
            "backups",
            "ssd-sentinel",
            "ssdsentinel",
            "programs",
            "connecteddevicesplatform",
            "comms",
            "dbg",
            "speech",
            "placeholderTileLogoFolder",
            "placeholdertilelogofolder",
            "servicehub",
            "application data",
            "start menu",
            "desktop",
            "documents",
            "downloads",
            "recent",
            "sendto",
            "templates",
            "favorites",
            "links",
        ];
        if BLOCKED.contains(&nl.as_str()) {
            return true;
        }
        nl.starts_with("regid.")
            || nl.starts_with("microsoft.")
            || nl.starts_with("windows.")
            || nl.ends_with("cache")
            || nl.ends_with("temp")
            || nl.ends_with("updater")
    }

    fn collect_hkcu_software_keys() -> (HashSet<String>, Vec<(String, String)>) {
        let mut key_set = HashSet::new();
        let mut missing_paths = Vec::new();
        let Some(sw) = open_reg(HKEY_CURRENT_USER, "Software") else {
            return (key_set, missing_paths);
        };
        const SKIP_SW: &[&str] = &[
            "classes",
            "clients",
            "microsoft",
            "policies",
            "registeredapplications",
            "odbc",
            "wow6432node",
            "appdatalow",
            "intel",
            "amd",
            "nvidia corporation",
            "realtek",
            "khronos",
            "ssd-sentinel",
        ];
        for child in reg_enum_subkeys(sw.0).into_iter().take(400) {
            let cl = child.to_ascii_lowercase();
            if SKIP_SW.contains(&cl.as_str()) || cl.len() < 3 {
                continue;
            }
            let canon = canonical_app_key(&child);
            if canon.len() >= 2 {
                key_set.insert(canon);
            }
            if let Some(ck) = open_reg(HKEY_CURRENT_USER, &format!("Software\\{child}")) {
                for val_name in [
                    "InstallPath",
                    "InstallLocation",
                    "InstallDir",
                    "Path",
                    "ExePath",
                ] {
                    if let Some(raw_val) = reg_query_string(ck.0, val_name) {
                        if let Some(exe) = clean_exe_from_command(&raw_val) {
                            if !Path::new(&exe).exists() {
                                missing_paths.push((exe, child.clone()));
                                break;
                            }
                        }
                    }
                }
            }
        }
        (key_set, missing_paths)
    }

    #[allow(clippy::too_many_arguments)]
    fn fuse_trace(
        map: &mut HashMap<String, ForensicAppTrace>,
        counts: &mut HashMap<String, usize>,
        source_tag: &str,
        exe_path: String,
        friendly_name: Option<String>,
        publisher: Option<String>,
        run_count: Option<u32>,
        last_run_at: Option<String>,
        is_game: bool,
        default_letter: &str,
    ) {
        if is_noise_app_exe(&exe_path, friendly_name.as_deref()) {
            return;
        }
        let drive_letter =
            extract_drive_letter(&exe_path).unwrap_or_else(|| default_letter.to_ascii_uppercase());
        let install_dir = extract_install_dir(&exe_path);
        let key = exe_path.to_ascii_lowercase();
        *counts.entry(source_tag.to_owned()).or_insert(0) += 1;
        let display = friendly_name
            .clone()
            .filter(|s| s.trim().len() >= 2)
            .map(|s| clean_app_display_name(&s, &exe_path))
            .unwrap_or_else(|| friendly_name_from_path(&exe_path));

        let entry = map.entry(key).or_insert_with(|| ForensicAppTrace {
            name: display.clone(),
            exe_path,
            publisher: publisher.clone(),
            sources: Vec::new(),
            run_count,
            last_run_at: last_run_at.clone(),
            vss_path: None,
            drive_letter,
            is_game,
            install_dir,
            appdata_paths: Vec::new(),
            leftover_bytes: 0,
        });
        if !entry.sources.iter().any(|s| s == source_tag) {
            entry.sources.push(source_tag.to_owned());
        }
        if entry.publisher.is_none() && publisher.is_some() {
            entry.publisher = publisher;
        }
        if friendly_name.is_some() && display.len() > entry.name.len() {
            entry.name = display;
        }
        if is_game {
            entry.is_game = true;
        }
        if let Some(rc) = run_count {
            entry.run_count = Some(entry.run_count.unwrap_or(0).max(rc));
        }
        if let Some(ts) = last_run_at {
            if entry.last_run_at.as_deref().unwrap_or("") < ts.as_str() {
                entry.last_run_at = Some(ts);
            }
        }
    }

    pub fn collect_windows_forensics(letter: &str) -> ForensicSweep {
        let mut sweep = ForensicSweep::default();
        let selected_upper = letter.trim().trim_end_matches(':').to_ascii_uppercase();
        let selected_prefix = format!("{selected_upper}:\\").to_ascii_lowercase();
        let mut map: HashMap<String, ForensicAppTrace> = HashMap::new();
        let vol_map = build_volume_map();
        let sys_drive = vol_map.system_drive.clone();
        let sys_letter = sys_drive.trim_end_matches(':').to_ascii_uppercase();
        let installed_index = InstalledAppIndex::build(&vol_map);

        // 1. AppCompatCache (ShimCache 10ts) — collect across ALL local drive letters!
        if let Some(key) = open_reg(
            HKEY_LOCAL_MACHINE,
            "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\AppCompatCache",
        ) {
            if let Some((_, bin)) = reg_query_bytes(key.0, "AppCompatCache") {
                for (raw_path, mtime) in parse_shimcache_10ts(&bin) {
                    let Some(path) = normalize_windows_path(&raw_path, &vol_map, None) else {
                        continue;
                    };
                    if path.to_ascii_lowercase().ends_with(".exe") && !Path::new(&path).exists() {
                        fuse_trace(
                            &mut map,
                            &mut sweep.source_counts,
                            "ShimCache",
                            path,
                            None,
                            None,
                            None,
                            mtime,
                            false,
                            &selected_upper,
                        );
                    }
                }
            }
        }

        // 2. UserAssist (ROT13 + 72B run count & timestamp) — all local drives
        if let Some(ua_root) = open_reg(
            HKEY_CURRENT_USER,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\UserAssist",
        ) {
            for guid in reg_enum_subkeys(ua_root.0) {
                let count_path = format!(
                    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\UserAssist\\{guid}\\Count"
                );
                let Some(cnt_key) = open_reg(HKEY_CURRENT_USER, &count_path) else {
                    continue;
                };
                let mut idx = 0u32;
                loop {
                    let mut nbuf = [0u16; 512];
                    let mut nlen = nbuf.len() as u32;
                    let mut dbuf = [0u8; 256];
                    let mut dlen = dbuf.len() as u32;
                    let mut vtype = 0u32;
                    let rc = unsafe {
                        RegEnumValueW(
                            cnt_key.0,
                            idx,
                            nbuf.as_mut_ptr(),
                            &mut nlen,
                            null_mut(),
                            &mut vtype,
                            dbuf.as_mut_ptr(),
                            &mut dlen,
                        )
                    };
                    if rc != 0 {
                        break;
                    }
                    idx += 1;
                    let encoded = String::from_utf16_lossy(&nbuf[..nlen as usize]);
                    let decoded_raw = rot13_decode(&encoded);
                    let Some(decoded) = normalize_windows_path(&decoded_raw, &vol_map, None) else {
                        continue;
                    };
                    if decoded.to_ascii_lowercase().ends_with(".exe")
                        && !Path::new(&decoded).exists()
                    {
                        let (runs, last_ts) = parse_userassist_72(&dbuf[..dlen as usize])
                            .map(|(r, t)| (Some(r), t))
                            .unwrap_or((None, None));
                        fuse_trace(
                            &mut map,
                            &mut sweep.source_counts,
                            "UserAssist",
                            decoded,
                            None,
                            None,
                            runs,
                            last_ts,
                            false,
                            &selected_upper,
                        );
                    }
                }
            }
        }

        // 3. BAM / DAM (Background Activity Moderator) — full \Device\HarddiskVolumeN mapping
        for svc in ["bam", "dam"] {
            let base = format!("SYSTEM\\CurrentControlSet\\Services\\{svc}\\State\\UserSettings");
            let Some(root_k) = open_reg(HKEY_LOCAL_MACHINE, &base) else {
                continue;
            };
            for sid in reg_enum_subkeys(root_k.0) {
                let Some(sk) = open_reg(HKEY_LOCAL_MACHINE, &format!("{base}\\{sid}")) else {
                    continue;
                };
                let mut idx = 0u32;
                loop {
                    let mut nbuf = [0u16; 512];
                    let mut nlen = nbuf.len() as u32;
                    let mut dbuf = [0u8; 64];
                    let mut dlen = dbuf.len() as u32;
                    let mut vtype = 0u32;
                    let rc = unsafe {
                        RegEnumValueW(
                            sk.0,
                            idx,
                            nbuf.as_mut_ptr(),
                            &mut nlen,
                            null_mut(),
                            &mut vtype,
                            dbuf.as_mut_ptr(),
                            &mut dlen,
                        )
                    };
                    if rc != 0 {
                        break;
                    }
                    idx += 1;
                    let raw_kpath = String::from_utf16_lossy(&nbuf[..nlen as usize]);
                    let Some(dos_path) = normalize_windows_path(&raw_kpath, &vol_map, None) else {
                        continue;
                    };
                    if dos_path.to_ascii_lowercase().ends_with(".exe")
                        && !Path::new(&dos_path).exists()
                    {
                        let last_ts = if dlen >= 8 {
                            let ft = i64::from_le_bytes(dbuf[0..8].try_into().unwrap_or([0; 8]));
                            filetime_to_rfc3339(ft)
                        } else {
                            None
                        };
                        fuse_trace(
                            &mut map,
                            &mut sweep.source_counts,
                            "BAM",
                            dos_path,
                            None,
                            None,
                            None,
                            last_ts,
                            false,
                            &selected_upper,
                        );
                    }
                }
            }
        }

        // 4. Windows Firewall Rules (survive long after uninstall/deletion!)
        if let Some(fw_key) = open_reg(
            HKEY_LOCAL_MACHINE,
            "SYSTEM\\CurrentControlSet\\Services\\SharedAccess\\Parameters\\FirewallPolicy\\FirewallRules",
        ) {
            let mut idx = 0u32;
            loop {
                let mut nbuf = [0u16; 256];
                let mut nlen = nbuf.len() as u32;
                let mut dbuf = [0u8; 4096];
                let mut dlen = dbuf.len() as u32;
                let mut vtype = 0u32;
                let rc = unsafe {
                    RegEnumValueW(
                        fw_key.0,
                        idx,
                        nbuf.as_mut_ptr(),
                        &mut nlen,
                        null_mut(),
                        &mut vtype,
                        dbuf.as_mut_ptr(),
                        &mut dlen,
                    )
                };
                if rc != 0 || idx >= 4096 {
                    break;
                }
                idx += 1;
                if vtype != 1 && vtype != 2 {
                    continue;
                }
                let rule_str = decode_utf16le(&dbuf[..dlen as usize]);
                if let Some((exe_path, friendly)) = parse_firewall_rule_value(&rule_str, &vol_map) {
                    if !Path::new(&exe_path).exists() {
                        fuse_trace(
                            &mut map,
                            &mut sweep.source_counts,
                            "Güvenlik Duvarı",
                            exe_path,
                            friendly,
                            None,
                            None,
                            None,
                            false,
                            &selected_upper,
                        );
                    }
                }
            }
        }

        // 5. GameConfigStore (HKCU\System\GameConfigStore\Children\* — catches games & fullscreen apps!)
        if let Some(gcs_root) = open_reg(HKEY_CURRENT_USER, "System\\GameConfigStore\\Children") {
            for child in reg_enum_subkeys(gcs_root.0).into_iter().take(1500) {
                let Some(ck) = open_reg(
                    HKEY_CURRENT_USER,
                    &format!("System\\GameConfigStore\\Children\\{child}"),
                ) else {
                    continue;
                };
                let Some(matched_exe) = reg_query_string(ck.0, "MatchedExeFullPath") else {
                    continue;
                };
                let title = reg_query_string(ck.0, "TitleName");
                if let Some(parsed) =
                    parse_game_config_entry(&matched_exe, title.as_deref(), &vol_map)
                {
                    if !Path::new(&parsed.exe_path).exists() {
                        fuse_trace(
                            &mut map,
                            &mut sweep.source_counts,
                            "GameConfigStore",
                            parsed.exe_path,
                            parsed.title,
                            None,
                            None,
                            None,
                            parsed.is_game,
                            &selected_upper,
                        );
                    }
                }
            }
        }

        // 6. Windows 10/11 Prefetch (MAM XPRESS-Huffman + SCCA v30/v31 + .pf filename fallback!)
        let pf_dir = PathBuf::from(format!("{sys_drive}\\Windows\\Prefetch"));
        if let Ok(entries) = std::fs::read_dir(&pf_dir) {
            for entry in entries.flatten().take(1500) {
                let p = entry.path();
                if p.extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case("pf"))
                    != Some(true)
                {
                    continue;
                }
                let pf_mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(system_time_to_rfc3339);
                let fname = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_owned();

                let parsed_scca = std::fs::read(&p).ok().and_then(|raw| {
                    let uncomp = if raw.starts_with(b"MAM") {
                        decompress_mam_prefetch(&raw)
                    } else if raw.len() > 8 && &raw[4..8] == b"SCCA" {
                        Some(raw)
                    } else {
                        None
                    };
                    uncomp.as_deref().and_then(parse_prefetch_scca)
                });

                if let Some(scca) = parsed_scca {
                    let ts = scca.last_run_at.clone().or_else(|| pf_mtime.clone());
                    if let Some(raw_full) = scca.full_path.as_deref() {
                        if let Some(dos_path) = normalize_windows_path(raw_full, &vol_map, None) {
                            if dos_path.to_ascii_lowercase().ends_with(".exe")
                                && !Path::new(&dos_path).exists()
                            {
                                fuse_trace(
                                    &mut map,
                                    &mut sweep.source_counts,
                                    "Prefetch",
                                    dos_path,
                                    None,
                                    None,
                                    Some(scca.run_count),
                                    ts,
                                    false,
                                    &selected_upper,
                                );
                            }
                            continue;
                        }
                    }
                    // Full path was absent or belonged to an unmapped volume: use exe_name fallback
                    // ONLY when confirmed NOT currently installed!
                    if !is_noise_app_exe(&scca.exe_name, None)
                        && !installed_index.is_installed_name(&scca.exe_name)
                    {
                        fuse_trace(
                            &mut map,
                            &mut sweep.source_counts,
                            "Prefetch",
                            scca.exe_name,
                            None,
                            None,
                            Some(scca.run_count),
                            ts,
                            false,
                            &sys_letter,
                        );
                    }
                } else if let Some(fb) = parse_prefetch_filename_fallback(&fname, pf_mtime) {
                    if !is_noise_app_exe(&fb.exe_name, None)
                        && !installed_index.is_installed_name(&fb.exe_name)
                    {
                        fuse_trace(
                            &mut map,
                            &mut sweep.source_counts,
                            "Prefetch",
                            fb.exe_name,
                            None,
                            None,
                            Some(fb.run_count),
                            fb.last_run_at,
                            false,
                            &sys_letter,
                        );
                    }
                }
            }
        }

        // 7. Revo / BCUninstaller Style Orphaned Uninstall & App Paths Keys (all local drives)
        let uninstall_roots = [
            (
                HKEY_LOCAL_MACHINE,
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            ),
            (
                HKEY_LOCAL_MACHINE,
                "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            ),
            (
                HKEY_CURRENT_USER,
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            ),
        ];
        for (hive, sub) in uninstall_roots {
            let Some(rk) = open_reg(hive, sub) else {
                continue;
            };
            for child in reg_enum_subkeys(rk.0) {
                let Some(ck) = open_reg(hive, &format!("{sub}\\{child}")) else {
                    continue;
                };
                if reg_query_u32(ck.0, "SystemComponent") == Some(1)
                    || reg_query_string(ck.0, "ParentKeyName").is_some()
                {
                    continue;
                }
                let display = reg_query_string(ck.0, "DisplayName");
                let publisher = reg_query_string(ck.0, "Publisher");
                let install_loc = reg_query_string(ck.0, "InstallLocation")
                    .map(|s| s.trim_matches('"').trim_end_matches('\\').to_owned());
                let icon_exe =
                    reg_query_string(ck.0, "DisplayIcon").and_then(|s| clean_exe_from_command(&s));
                let uninst_exe = reg_query_string(ck.0, "UninstallString")
                    .and_then(|s| clean_exe_from_command(&s));
                let cand_exe = icon_exe.or(uninst_exe).or_else(|| {
                    install_loc.as_ref().and_then(|dir| {
                        if dir.is_empty() {
                            return None;
                        }
                        display.as_ref().map(|name| {
                            format!(
                                "{dir}\\{}.exe",
                                name.split_whitespace().next().unwrap_or("app")
                            )
                        })
                    })
                });
                let Some(raw_exe) = cand_exe else {
                    continue;
                };
                let Some(exe_path) = normalize_windows_path(&raw_exe, &vol_map, None) else {
                    continue;
                };
                let missing_exe = !Path::new(&exe_path).exists();
                let dir_still_live = install_loc
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .is_some_and(|d| {
                        Path::new(d).is_dir() && dir_contains_live_exe(Path::new(d), 0)
                    });
                if missing_exe && !dir_still_live {
                    fuse_trace(
                        &mut map,
                        &mut sweep.source_counts,
                        "Yetim Kayıt",
                        exe_path,
                        display,
                        publisher,
                        None,
                        None,
                        false,
                        &selected_upper,
                    );
                }
            }
        }

        // App Paths orphaned check (all local drives)
        for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            let sub = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths";
            let Some(rk) = open_reg(hive, sub) else {
                continue;
            };
            for child in reg_enum_subkeys(rk.0) {
                let Some(ck) = open_reg(hive, &format!("{sub}\\{child}")) else {
                    continue;
                };
                let Some(raw_def) = reg_query_string(ck.0, "") else {
                    continue;
                };
                let Some(raw_exe) = clean_exe_from_command(&raw_def) else {
                    continue;
                };
                let Some(exe_path) = normalize_windows_path(&raw_exe, &vol_map, None) else {
                    continue;
                };
                if !Path::new(&exe_path).exists() {
                    fuse_trace(
                        &mut map,
                        &mut sweep.source_counts,
                        "App Paths",
                        exe_path,
                        None,
                        None,
                        None,
                        None,
                        false,
                        &selected_upper,
                    );
                }
            }
        }

        // 8. Dead Start Menu & Desktop Shortcuts (User + Public Desktop, MS-SHLLINK `.lnk` parser)
        let mut lnk_roots = Vec::new();
        if let Ok(up) = std::env::var("USERPROFILE") {
            lnk_roots.push(PathBuf::from(format!("{up}\\Desktop")));
        }
        if let Ok(pub_dir) = std::env::var("PUBLIC") {
            lnk_roots.push(PathBuf::from(format!("{pub_dir}\\Desktop")));
        }
        if let Ok(ap) = std::env::var("APPDATA") {
            lnk_roots.push(PathBuf::from(format!(
                "{ap}\\Microsoft\\Windows\\Start Menu\\Programs"
            )));
        }
        if let Ok(pd) = std::env::var("ProgramData") {
            lnk_roots.push(PathBuf::from(format!(
                "{pd}\\Microsoft\\Windows\\Start Menu\\Programs"
            )));
        }
        for root_dir in lnk_roots {
            scan_lnk_dir(
                &root_dir,
                0,
                &vol_map,
                &selected_upper,
                &mut map,
                &mut sweep.source_counts,
            );
        }

        // 9. Orphaned AppData / ProgramData / LocalAppData\Programs + HKCU\Software correlation
        let (hkcu_sw_keys, hkcu_missing_exes) = collect_hkcu_software_keys();
        for (raw_exe, sw_name) in hkcu_missing_exes {
            if let Some(exe_path) = normalize_windows_path(&raw_exe, &vol_map, None) {
                if !Path::new(&exe_path).exists() {
                    fuse_trace(
                        &mut map,
                        &mut sweep.source_counts,
                        "Kayıt Defteri",
                        exe_path,
                        Some(sw_name),
                        None,
                        None,
                        None,
                        false,
                        &selected_upper,
                    );
                }
            }
        }

        let mut appdata_roots: Vec<(PathBuf, &str)> = Vec::new();
        if let Ok(ap) = std::env::var("APPDATA") {
            appdata_roots.push((PathBuf::from(ap), "AppData Kalıntısı"));
        }
        if let Ok(lap) = std::env::var("LOCALAPPDATA") {
            appdata_roots.push((
                PathBuf::from(format!("{lap}\\Programs")),
                "AppData Kalıntısı",
            ));
            appdata_roots.push((PathBuf::from(lap), "AppData Kalıntısı"));
        }
        if let Ok(pd) = std::env::var("ProgramData") {
            appdata_roots.push((PathBuf::from(pd), "ProgramData Kalıntısı"));
        }

        let mut seen_appdata = HashSet::new();
        for (base_dir, scope) in appdata_roots {
            let Ok(entries) = std::fs::read_dir(&base_dir) else {
                continue;
            };
            for entry in entries.flatten().take(350) {
                let p = entry.path();
                if !p.is_dir() || platform::reparse(&p) {
                    continue;
                }
                let Some(folder_name) = p.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if is_os_or_runtime_appdata_folder(folder_name) {
                    continue;
                }
                let path_str = p.to_string_lossy().into_owned();
                if !seen_appdata.insert(path_str.to_ascii_lowercase()) {
                    continue;
                }
                let has_live_exe = dir_contains_live_exe(&p, 0);
                let is_installed = has_live_exe || installed_index.is_installed_name(folder_name);
                let canon = canonical_app_key(folder_name);
                if canon.len() < 3 {
                    continue;
                }
                let has_reg = hkcu_sw_keys.contains(&canon);
                let size_bytes = dir_size_bounded(&p, 4, 800);
                let modified_at = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(system_time_to_rfc3339);

                sweep.appdata_entries.push(AppDataEntry {
                    folder_name: folder_name.to_owned(),
                    path: path_str.clone(),
                    size_bytes,
                    scope: scope.to_owned(),
                    is_installed,
                    has_registry_key: has_reg,
                    modified_at: modified_at.clone(),
                });

                // Standalone orphaned AppData/ProgramData candidate if truly uninstalled and meaningful
                if !is_installed && (size_bytes >= 16 * 1024 || (has_reg && size_bytes >= 4096)) {
                    let already_traced = map.values().any(|t| {
                        canonical_app_key(&t.name) == canon
                            || canonical_app_key(&t.exe_path) == canon
                    });
                    if !already_traced {
                        let key = path_str.to_ascii_lowercase();
                        *sweep.source_counts.entry(scope.to_owned()).or_insert(0) += 1;
                        let mut srcs = vec![scope.to_owned()];
                        if has_reg {
                            srcs.push("Kayıt Defteri".into());
                            *sweep
                                .source_counts
                                .entry("Kayıt Defteri".into())
                                .or_insert(0) += 1;
                        }
                        let clean_name = clean_app_display_name(folder_name, &path_str);
                        map.entry(key).or_insert_with(|| ForensicAppTrace {
                            exe_path: path_str.clone(),
                            name: clean_name,
                            publisher: None,
                            sources: srcs,
                            run_count: None,
                            last_run_at: modified_at,
                            vss_path: None,
                            drive_letter: extract_drive_letter(&path_str)
                                .unwrap_or_else(|| sys_letter.clone()),
                            is_game: false,
                            install_dir: None,
                            appdata_paths: vec![path_str],
                            leftover_bytes: size_bytes,
                        });
                    }
                }
            }
        }

        // 10. Downloads folder installers (for correlation with deleted apps!)
        if let Ok(up) = std::env::var("USERPROFILE") {
            let dl_dir = PathBuf::from(format!("{up}\\Downloads"));
            if let Ok(entries) = std::fs::read_dir(&dl_dir) {
                for entry in entries.flatten().take(300) {
                    let p = entry.path();
                    if !p.is_file() || platform::reparse(&p) {
                        continue;
                    }
                    let ext = p
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if !matches!(ext.as_str(), "exe" | "msi" | "msix") {
                        continue;
                    }
                    let Ok(meta) = entry.metadata() else {
                        continue;
                    };
                    if meta.len() < 64 * 1024 {
                        continue;
                    }
                    let Some(file_name) = p.file_name().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    let path_str = p.to_string_lossy().into_owned();
                    if is_noise_app_exe(&path_str, None) {
                        continue;
                    }
                    sweep.downloads_installers.push(DownloadsInstaller {
                        drive_letter: extract_drive_letter(&path_str)
                            .unwrap_or_else(|| sys_letter.clone()),
                        path: path_str,
                        file_name: file_name.to_owned(),
                        size_bytes: meta.len(),
                        modified_at: meta.modified().ok().and_then(system_time_to_rfc3339),
                    });
                }
            }
        }

        // 11. Hidden C:\Windows\Installer\*.msi Cache Scanner
        let inst_dir = PathBuf::from(format!("{sys_drive}\\Windows\\Installer"));
        if let Ok(entries) = std::fs::read_dir(&inst_dir) {
            let mut probe = vec![0u8; 64 * 1024];
            for entry in entries.flatten().take(120) {
                let p = entry.path();
                if p.extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case("msi"))
                    != Some(true)
                {
                    continue;
                }
                let sz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                if sz < 16 * 1024 {
                    continue;
                }
                let got = File::open(&p)
                    .and_then(|mut f| f.read(&mut probe))
                    .unwrap_or(0);
                if got < 512 {
                    continue;
                }
                let prod =
                    crate::recovery::extract_utf16_version_field(&probe[..got], "ProductName")
                        .unwrap_or_else(|| {
                            p.file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("CachedInstaller")
                                .to_owned()
                        });
                let pub_name =
                    crate::recovery::extract_utf16_version_field(&probe[..got], "CompanyName");
                sweep.cached_msis.push(CachedMsiFile {
                    msi_path: p.to_string_lossy().into_owned(),
                    product_name: prod,
                    publisher: pub_name,
                    size_bytes: sz,
                });
                if sweep.cached_msis.len() >= 25 {
                    break;
                }
            }
        }

        // 12. Volume Shadow Copy (VSS) Cross-Recovery Check (bypasses SSD TRIM!)
        let mut shadows_by_letter: HashMap<String, Vec<String>> = HashMap::new();
        let selected_shadows = find_matching_vss_shadows(&selected_upper);
        sweep.vss_shadows_found = selected_shadows.len();
        shadows_by_letter.insert(selected_upper.clone(), selected_shadows);
        if sys_letter != selected_upper {
            shadows_by_letter.insert(sys_letter.clone(), find_matching_vss_shadows(&sys_letter));
        }

        for trace in map.values_mut() {
            let Some(shadows) = shadows_by_letter.get(&trace.drive_letter) else {
                continue;
            };
            for shadow_root in shadows {
                if let Some(vp) = vss_relative_path(shadow_root, &trace.exe_path) {
                    let sz = std::fs::metadata(&vp).map(|m| m.len()).unwrap_or(0);
                    let vss_str = vp.to_string_lossy().into_owned();
                    trace.vss_path = Some(vss_str.clone());
                    if !trace.sources.iter().any(|s| s == "VSS Gölge Kopya") {
                        trace.sources.push("VSS Gölge Kopya".into());
                    }
                    *sweep
                        .source_counts
                        .entry("VSS Gölge Kopya".into())
                        .or_insert(0) += 1;
                    sweep.vss_files.push(VssSnapshotFile {
                        original_path: trace.exe_path.clone(),
                        vss_path: vss_str,
                        size_bytes: sz,
                        modified_at: trace.last_run_at.clone(),
                        drive_letter: trace.drive_letter.clone(),
                    });
                    break;
                }
            }
        }

        // Also scan newest Shadow Copy's Downloads/Desktop/Documents for deleted user files on selected drive.
        if let (Some(newest_shadow), Ok(up)) = (
            shadows_by_letter
                .get(&selected_upper)
                .and_then(|v| v.first()),
            std::env::var("USERPROFILE"),
        ) {
            if up.to_ascii_lowercase().starts_with(&selected_prefix) {
                let rel_up = up[3..].trim_start_matches('\\');
                for sub in ["Downloads", "Desktop", "Documents"] {
                    let shadow_sub = PathBuf::from(format!("{newest_shadow}{rel_up}\\{sub}"));
                    let Ok(entries) = std::fs::read_dir(&shadow_sub) else {
                        continue;
                    };
                    for entry in entries.flatten().take(200) {
                        let sp = entry.path();
                        if !sp.is_file() {
                            continue;
                        }
                        let Some(fname) = sp.file_name().and_then(|s| s.to_str()) else {
                            continue;
                        };
                        let live_p = PathBuf::from(format!("{up}\\{sub}\\{fname}"));
                        if !live_p.exists() {
                            let sz = std::fs::metadata(&sp).map(|m| m.len()).unwrap_or(0);
                            if sz > 0 {
                                sweep.vss_files.push(VssSnapshotFile {
                                    original_path: live_p.to_string_lossy().into_owned(),
                                    vss_path: sp.to_string_lossy().into_owned(),
                                    size_bytes: sz,
                                    modified_at: None,
                                    drive_letter: selected_upper.clone(),
                                });
                                if sweep.vss_files.len() >= 100 {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut traces: Vec<ForensicAppTrace> = map.into_values().collect();
        traces.sort_by(|a, b| {
            let a_sel = a.drive_letter.eq_ignore_ascii_case(&selected_upper);
            let b_sel = b.drive_letter.eq_ignore_ascii_case(&selected_upper);
            b.vss_path
                .is_some()
                .cmp(&a.vss_path.is_some())
                .then_with(|| b_sel.cmp(&a_sel))
                .then_with(|| b.sources.len().cmp(&a.sources.len()))
                .then_with(|| b.run_count.unwrap_or(0).cmp(&a.run_count.unwrap_or(0)))
                .then_with(|| b.last_run_at.cmp(&a.last_run_at))
        });
        traces.truncate(MAX_FORENSIC_APPS);
        sweep.traces = traces;
        sweep
    }

    fn scan_lnk_dir(
        dir: &Path,
        depth: usize,
        vol_map: &VolumeMap,
        selected_upper: &str,
        map: &mut HashMap<String, ForensicAppTrace>,
        counts: &mut HashMap<String, usize>,
    ) {
        if depth > 2 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten().take(200) {
            let p = entry.path();
            if p.is_dir() {
                scan_lnk_dir(&p, depth + 1, vol_map, selected_upper, map, counts);
            } else if p
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("lnk"))
                == Some(true)
            {
                let Ok(bytes) = std::fs::read(&p) else {
                    continue;
                };
                if let Some(raw_target) = parse_lnk_target_path(&bytes) {
                    let Some(target) = normalize_windows_path(&raw_target, vol_map, None) else {
                        continue;
                    };
                    if target.to_ascii_lowercase().ends_with(".exe") && !Path::new(&target).exists()
                    {
                        let friendly = p.file_stem().and_then(|s| s.to_str()).map(str::to_owned);
                        fuse_trace(
                            map,
                            counts,
                            "Kırık Kısayol (.lnk)",
                            target,
                            friendly,
                            None,
                            None,
                            None,
                            false,
                            selected_upper,
                        );
                    }
                }
            }
        }
    }

    #[repr(C)]
    struct ReadUsnJournalDataV0 {
        start_usn: i64,
        reason_mask: u32,
        return_only_on_close: u32,
        timeout: u64,
        bytes_to_wait_for: u64,
        usn_journal_id: u64,
    }

    pub fn read_usn_deleted_records(letter: &str, max_records: usize) -> Vec<UsnDeletedRecord> {
        let w = wide(&format!("\\\\.\\{letter}:"));
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        let vh = WinHandle(h);
        let mut qbuf = [0u8; 64];
        let mut qret = 0u32;
        let qok = unsafe {
            DeviceIoControl(
                vh.0,
                FSCTL_QUERY_USN_JOURNAL,
                null(),
                0,
                qbuf.as_mut_ptr().cast(),
                qbuf.len() as u32,
                &mut qret,
                null_mut(),
            )
        };
        if qok == 0 || qret < 24 {
            return Vec::new();
        }
        let journal_id = u64::from_le_bytes(qbuf[0..8].try_into().unwrap_or([0; 8]));
        let first_usn = i64::from_le_bytes(qbuf[8..16].try_into().unwrap_or([0; 8]));
        let next_usn = i64::from_le_bytes(qbuf[16..24].try_into().unwrap_or([0; 8]));
        let mut cursor_usn = first_usn.max(next_usn.saturating_sub(64 * 1024 * 1024));

        let mut out_buf = vec![0u8; 256 * 1024];
        let mut results = Vec::new();
        for _ in 0..96 {
            if results.len() >= max_records || cursor_usn >= next_usn {
                break;
            }
            let req = ReadUsnJournalDataV0 {
                start_usn: cursor_usn,
                reason_mask: 0x0000_0200, // USN_REASON_FILE_DELETE
                return_only_on_close: 0,
                timeout: 0,
                bytes_to_wait_for: 0,
                usn_journal_id: journal_id,
            };
            let mut got = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    vh.0,
                    FSCTL_READ_USN_JOURNAL,
                    (&req as *const ReadUsnJournalDataV0).cast(),
                    std::mem::size_of::<ReadUsnJournalDataV0>() as u32,
                    out_buf.as_mut_ptr().cast(),
                    out_buf.len() as u32,
                    &mut got,
                    null_mut(),
                )
            };
            if ok == 0 || got <= 8 {
                break;
            }
            let next_cursor = i64::from_le_bytes(out_buf[0..8].try_into().unwrap_or([0; 8]));
            results.extend(parse_usn_records_v2(&out_buf[8..got as usize]));
            if next_cursor <= cursor_usn {
                break;
            }
            cursor_usn = next_cursor;
        }
        results.reverse(); // Newest deletions first.
        results.truncate(max_records);
        results
    }
}

#[cfg(target_os = "linux")]
mod linux_forensics {
    use super::*;
    use crate::platform;
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::fs::MetadataExt;

    fn linux_homes() -> Vec<PathBuf> {
        let mut homes = Vec::new();
        let mut seen = HashSet::new();
        if let Some(h) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
            let p = PathBuf::from(h);
            if p.is_dir() && seen.insert(p.clone()) {
                homes.push(p);
            }
        }
        if let Ok(entries) = std::fs::read_dir("/home") {
            for ent in entries.flatten() {
                let p = ent.path();
                if p.is_dir() && !platform::reparse(&p) && seen.insert(p.clone()) {
                    homes.push(p);
                }
            }
        }
        homes
    }

    fn linux_mount_for_path(path: &str, fallback_letter: &str) -> String {
        if !path.starts_with('/') {
            return platform::normalize_volume_id(fallback_letter);
        }
        crate::platform_linux::mounted_volumes()
            .into_iter()
            .filter(|v| {
                let mp = v.mount_point.as_str();
                mp == "/" || path == mp || path.starts_with(&format!("{mp}/"))
            })
            .max_by_key(|v| v.mount_point.len())
            .map(|v| platform::normalize_volume_id(&v.mount_point))
            .unwrap_or_else(|| platform::normalize_volume_id(fallback_letter))
    }

    fn bump_source(sweep: &mut ForensicSweep, name: &str) {
        *sweep.source_counts.entry(name.to_owned()).or_insert(0) += 1;
    }

    fn read_tail(path: &Path, max_bytes: usize) -> Option<String> {
        let mut f = File::open(path).ok()?;
        let len = f.metadata().ok()?.len();
        if len > max_bytes as u64 {
            let _ = f.seek(SeekFrom::Start(len - max_bytes as u64));
        }
        let mut buf = String::new();
        f.take(max_bytes as u64).read_to_string(&mut buf).ok()?;
        Some(buf)
    }

    fn percent_decode_file_uri(href: &str) -> Option<String> {
        let rest = href.strip_prefix("file://")?;
        let bytes = rest.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let (Some(a), Some(b)) = (
                    (bytes[i + 1] as char).to_digit(16),
                    (bytes[i + 2] as char).to_digit(16),
                ) {
                    out.push(((a << 4) | b) as u8);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        let s = String::from_utf8_lossy(&out).into_owned();
        s.starts_with('/').then_some(s)
    }

    fn command_binary_exists(cmd: &str) -> bool {
        let tok = cmd
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches('"');
        if tok.is_empty() {
            return true;
        }
        if tok.starts_with('/') {
            return Path::new(tok).exists();
        }
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                if dir.join(tok).is_file() {
                    return true;
                }
            }
        }
        false
    }

    fn scan_snapshot_subtree(
        snap_root: &Path,
        sub_base: &Path,
        live_prefix: &Path,
        drive_letter: &str,
        expected_dev: u64,
        depth: usize,
        sweep: &mut ForensicSweep,
    ) {
        if depth > 3 || sweep.vss_files.len() >= 150 || platform::reparse(snap_root) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(snap_root) else {
            return;
        };
        for ent in entries.flatten() {
            if sweep.vss_files.len() >= 150 {
                break;
            }
            let p = ent.path();
            if platform::reparse(&p) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.dev() != expected_dev {
                continue;
            }
            let Ok(rel) = p.strip_prefix(sub_base) else {
                continue;
            };
            let rel_s = rel.to_string_lossy();
            if rel_s.starts_with("proc/")
                || rel_s.starts_with("sys/")
                || rel_s.starts_with("dev/")
                || rel_s.starts_with("run/")
                || rel_s.starts_with("usr/")
                || rel_s.starts_with("var/cache/")
            {
                continue;
            }
            if meta.is_dir() {
                scan_snapshot_subtree(
                    &p,
                    sub_base,
                    live_prefix,
                    drive_letter,
                    expected_dev,
                    depth + 1,
                    sweep,
                );
            } else if meta.is_file() && meta.len() >= 512 {
                let live_path = live_prefix.join(rel);
                if !live_path.exists() {
                    let modified_at = meta.modified().ok().and_then(system_time_to_rfc3339);
                    bump_source(sweep, "Linux Snapshot");
                    sweep.vss_files.push(VssSnapshotFile {
                        original_path: live_path.to_string_lossy().into_owned(),
                        vss_path: p.to_string_lossy().into_owned(),
                        size_bytes: meta.len(),
                        modified_at,
                        drive_letter: drive_letter.to_owned(),
                    });
                }
            }
        }
    }

    fn collect_linux_snapshots(letter: &str, sweep: &mut ForensicSweep) {
        let mount = PathBuf::from(if letter.starts_with('/') { letter } else { "/" });
        let candidates = [
            mount.join(".snapshots"),
            PathBuf::from("/.snapshots"),
            mount.join(".zfs/snapshot"),
            PathBuf::from("/run/timeshift/backup"),
        ];
        let mut seen = HashSet::new();
        for base in candidates {
            if !base.is_dir() || platform::reparse(&base) {
                continue;
            }
            let Ok(canon) = base.canonicalize() else {
                continue;
            };
            if !seen.insert(canon.clone()) {
                continue;
            }
            let Ok(snaps) = std::fs::read_dir(&canon) else {
                continue;
            };
            for snap in snaps.flatten().take(4) {
                if sweep.vss_files.len() >= 150 {
                    break;
                }
                let sp = snap.path();
                if !sp.is_dir() || platform::reparse(&sp) {
                    continue;
                }
                let snap_dir = if sp.join("snapshot").is_dir() {
                    sp.join("snapshot")
                } else if sp.join("localhost").is_dir() {
                    sp.join("localhost")
                } else {
                    sp
                };
                if let Ok(meta) = std::fs::metadata(&snap_dir) {
                    sweep.vss_shadows_found += 1;
                    let home_sub = snap_dir.join("home");
                    if home_sub.is_dir() && !platform::reparse(&home_sub) {
                        if let Ok(hm) = std::fs::metadata(&home_sub) {
                            scan_snapshot_subtree(
                                &home_sub,
                                &snap_dir,
                                &mount,
                                &platform::normalize_volume_id(letter),
                                hm.dev(),
                                0,
                                sweep,
                            );
                        }
                    } else {
                        scan_snapshot_subtree(
                            &snap_dir,
                            &snap_dir,
                            &mount,
                            &platform::normalize_volume_id(letter),
                            meta.dev(),
                            0,
                            sweep,
                        );
                    }
                }
            }
        }
    }

    fn collect_desktop_and_xbel(home: &Path, letter: &str, sweep: &mut ForensicSweep) {
        let apps_dir = home.join(".local/share/applications");
        if let Ok(entries) = std::fs::read_dir(&apps_dir) {
            for ent in entries.flatten().take(200) {
                if sweep.traces.len() >= MAX_FORENSIC_APPS {
                    break;
                }
                let p = ent.path();
                if p.extension().and_then(|s| s.to_str()) != Some("desktop") {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let mut name = None;
                let mut exec = None;
                let mut is_game = false;
                let mut no_display = false;
                for line in content.lines() {
                    let l = line.trim();
                    if let Some(v) = l.strip_prefix("Name=") {
                        name = Some(v.to_owned());
                    } else if let Some(v) = l.strip_prefix("Exec=") {
                        exec = Some(v.to_owned());
                    } else if let Some(v) = l.strip_prefix("Categories=") {
                        if v.to_ascii_lowercase().contains("game") {
                            is_game = true;
                        }
                    } else if l == "NoDisplay=true" {
                        no_display = true;
                    }
                }
                if no_display {
                    continue;
                }
                let Some(exec_cmd) = exec else {
                    continue;
                };
                if !command_binary_exists(&exec_cmd) {
                    let bin_path = exec_cmd
                        .split_whitespace()
                        .next()
                        .unwrap_or(&exec_cmd)
                        .trim_matches('"')
                        .to_owned();
                    let app_name = name.unwrap_or_else(|| friendly_name_from_path(&bin_path));
                    let modified_at = ent
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(system_time_to_rfc3339);
                    bump_source(sweep, ".desktop Kısayol");
                    let dl = linux_mount_for_path(&bin_path, letter);
                    sweep.traces.push(ForensicAppTrace {
                        install_dir: extract_install_dir(&bin_path),
                        exe_path: bin_path,
                        name: app_name,
                        publisher: None,
                        sources: vec![".desktop".into()],
                        run_count: None,
                        last_run_at: modified_at,
                        vss_path: None,
                        drive_letter: dl,
                        is_game,
                        appdata_paths: Vec::new(),
                        leftover_bytes: 0,
                    });
                }
            }
        }

        let xbel = home.join(".local/share/recently-used.xbel");
        if let Ok(xml) = std::fs::read_to_string(&xbel) {
            if let Ok(doc) = roxmltree::Document::parse(&xml) {
                for node in doc.descendants().filter(|n| n.has_tag_name("bookmark")) {
                    if sweep.traces.len() >= MAX_FORENSIC_APPS {
                        break;
                    }
                    let Some(href) = node.attribute("href") else {
                        continue;
                    };
                    let Some(file_path) = percent_decode_file_uri(href) else {
                        continue;
                    };
                    if Path::new(&file_path).exists() {
                        continue;
                    }
                    let ext = Path::new(&file_path)
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if matches!(ext.as_str(), "appimage" | "deb" | "rpm" | "exe" | "sh") {
                        bump_source(sweep, "XBEL Son Kullanılan");
                        let modified_at = node.attribute("modified").map(str::to_owned);
                        let dl = linux_mount_for_path(&file_path, letter);
                        sweep.traces.push(ForensicAppTrace {
                            install_dir: extract_install_dir(&file_path),
                            name: friendly_name_from_path(&file_path),
                            exe_path: file_path,
                            publisher: None,
                            sources: vec!["recently-used.xbel".into()],
                            run_count: None,
                            last_run_at: modified_at,
                            vss_path: None,
                            drive_letter: dl,
                            is_game: false,
                            appdata_paths: Vec::new(),
                            leftover_bytes: 0,
                        });
                    }
                }
            }
        }
    }

    fn collect_game_launchers(home: &Path, letter: &str, sweep: &mut ForensicSweep) {
        let steam_dirs = [
            home.join(".local/share/Steam/steamapps"),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps"),
        ];
        for sa in steam_dirs {
            let Ok(entries) = std::fs::read_dir(&sa) else {
                continue;
            };
            for ent in entries.flatten().take(120) {
                if sweep.traces.len() >= MAX_FORENSIC_APPS {
                    break;
                }
                let p = ent.path();
                let Some(fname) = p.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if !fname.starts_with("appmanifest_") || !fname.ends_with(".acf") {
                    continue;
                }
                let Ok(txt) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let mut app_name = None;
                let mut install_dir = None;
                for line in txt.lines() {
                    let parts: Vec<&str> = line.split('"').collect();
                    if parts.len() >= 4 {
                        match parts[1] {
                            "name" => app_name = Some(parts[3].to_owned()),
                            "installdir" => install_dir = Some(parts[3].to_owned()),
                            _ => {}
                        }
                    }
                }
                let (Some(name), Some(dir_name)) = (app_name, install_dir) else {
                    continue;
                };
                let game_path = sa.join("common").join(&dir_name);
                if !game_path.exists() {
                    bump_source(sweep, "Steam Manifest");
                    let gp_str = game_path.to_string_lossy().into_owned();
                    let dl = linux_mount_for_path(&gp_str, letter);
                    sweep.traces.push(ForensicAppTrace {
                        exe_path: format!("{gp_str}/{dir_name}"),
                        name,
                        publisher: Some("Steam".into()),
                        sources: vec!["Steam ACF".into()],
                        run_count: None,
                        last_run_at: None,
                        vss_path: None,
                        drive_letter: dl,
                        is_game: true,
                        install_dir: Some(gp_str),
                        appdata_paths: Vec::new(),
                        leftover_bytes: 0,
                    });
                }
            }
        }

        let lutris_dir = home.join(".config/lutris/games");
        if let Ok(entries) = std::fs::read_dir(&lutris_dir) {
            for ent in entries.flatten().take(80) {
                if sweep.traces.len() >= MAX_FORENSIC_APPS {
                    break;
                }
                let Ok(txt) = std::fs::read_to_string(ent.path()) else {
                    continue;
                };
                let mut exe = None;
                let mut name = None;
                for line in txt.lines() {
                    let t = line.trim();
                    if let Some(v) = t.strip_prefix("exe:") {
                        exe = Some(v.trim().trim_matches('"').to_owned());
                    } else if let Some(v) = t.strip_prefix("name:") {
                        name = Some(v.trim().trim_matches('"').to_owned());
                    }
                }
                if let Some(exe_path) = exe.filter(|e| e.starts_with('/') && !Path::new(e).exists())
                {
                    bump_source(sweep, "Lutris");
                    let dl = linux_mount_for_path(&exe_path, letter);
                    sweep.traces.push(ForensicAppTrace {
                        install_dir: extract_install_dir(&exe_path),
                        name: name.unwrap_or_else(|| friendly_name_from_path(&exe_path)),
                        exe_path,
                        publisher: Some("Lutris".into()),
                        sources: vec!["Lutris".into()],
                        run_count: None,
                        last_run_at: None,
                        vss_path: None,
                        drive_letter: dl,
                        is_game: true,
                        appdata_paths: Vec::new(),
                        leftover_bytes: 0,
                    });
                }
            }
        }

        let heroic_json = home.join(".config/heroic/installed.json");
        if let Ok(raw) = std::fs::read_to_string(&heroic_json) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                for g in val["installed"].as_array().into_iter().flatten().take(80) {
                    if sweep.traces.len() >= MAX_FORENSIC_APPS {
                        break;
                    }
                    let Some(ip) = g["install_path"].as_str().filter(|s| s.starts_with('/')) else {
                        continue;
                    };
                    if !Path::new(ip).exists() {
                        bump_source(sweep, "Heroic Launcher");
                        let title = g["title"].as_str().unwrap_or("Heroic Game").to_owned();
                        let exe = g["executable"].as_str().unwrap_or("game");
                        let full = format!("{ip}/{exe}");
                        let dl = linux_mount_for_path(&full, letter);
                        sweep.traces.push(ForensicAppTrace {
                            exe_path: full,
                            name: title,
                            publisher: Some("Heroic".into()),
                            sources: vec!["Heroic".into()],
                            run_count: None,
                            last_run_at: None,
                            vss_path: None,
                            drive_letter: dl,
                            is_game: true,
                            install_dir: Some(ip.to_owned()),
                            appdata_paths: Vec::new(),
                            leftover_bytes: 0,
                        });
                    }
                }
            }
        }
    }

    fn bounded_dir_size(path: &Path, expected_dev: u64, limit: usize) -> u64 {
        let mut stack = vec![path.to_path_buf()];
        let mut total = 0u64;
        let mut count = 0usize;
        while let Some(cur) = stack.pop() {
            if count >= limit {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&cur) else {
                continue;
            };
            for ent in entries.flatten() {
                if count >= limit {
                    break;
                }
                let p = ent.path();
                if platform::reparse(&p) {
                    continue;
                }
                let Ok(m) = std::fs::symlink_metadata(&p) else {
                    continue;
                };
                if m.dev() != expected_dev {
                    continue;
                }
                if m.is_dir() {
                    stack.push(p);
                } else {
                    total = total.saturating_add(m.len());
                    count += 1;
                }
            }
        }
        total
    }

    fn collect_appdata_and_downloads(home: &Path, letter: &str, sweep: &mut ForensicSweep) {
        for (sub, scope) in [(".config", "~/.config"), (".local/share", "~/.local/share")] {
            let dir = home.join(sub);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for ent in entries.flatten().take(120) {
                let p = ent.path();
                if !p.is_dir() || platform::reparse(&p) {
                    continue;
                }
                let Some(fname) = p.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let lower = fname.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "systemd"
                        | "dconf"
                        | "pulse"
                        | "gtk-3.0"
                        | "gtk-4.0"
                        | "gnome-session"
                        | "fontconfig"
                        | "applications"
                        | "trash"
                        | "icons"
                        | "mime"
                        | "keyrings"
                ) {
                    continue;
                }
                let Ok(m) = std::fs::symlink_metadata(&p) else {
                    continue;
                };
                let sz = bounded_dir_size(&p, m.dev(), 120);
                if sz < 4096 {
                    continue;
                }
                sweep.appdata_entries.push(AppDataEntry {
                    folder_name: fname.to_owned(),
                    path: p.to_string_lossy().into_owned(),
                    size_bytes: sz,
                    scope: scope.into(),
                    is_installed: false,
                    has_registry_key: false,
                    modified_at: m.modified().ok().and_then(system_time_to_rfc3339),
                });
            }
        }

        let dl_dir = home.join("Downloads");
        if let Ok(entries) = std::fs::read_dir(&dl_dir) {
            for ent in entries.flatten().take(150) {
                let p = ent.path();
                if !p.is_file() || platform::reparse(&p) {
                    continue;
                }
                let Some(fname) = p.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let ext = p
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(
                    ext.as_str(),
                    "appimage" | "deb" | "rpm" | "flatpakref" | "exe" | "msi"
                ) {
                    let meta = std::fs::metadata(&p).ok();
                    let full = p.to_string_lossy().into_owned();
                    let dl = linux_mount_for_path(&full, letter);
                    sweep.downloads_installers.push(DownloadsInstaller {
                        path: full,
                        file_name: fname.to_owned(),
                        size_bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                        modified_at: meta
                            .and_then(|m| m.modified().ok())
                            .and_then(system_time_to_rfc3339),
                        drive_letter: dl,
                    });
                }
            }
        }
    }

    fn collect_package_removal_logs(sweep: &mut ForensicSweep) {
        let mut push_pkg = |pkg: &str, src: &str, ts: Option<String>| {
            if pkg.len() < 3
                || pkg.starts_with("lib")
                || pkg.ends_with("-dev")
                || pkg.ends_with("-common")
                || sweep.traces.len() >= MAX_FORENSIC_APPS
            {
                return;
            }
            if Path::new("/usr/bin").join(pkg).exists() {
                return;
            }
            bump_source(sweep, src);
            sweep.traces.push(ForensicAppTrace {
                exe_path: format!("/usr/bin/{pkg}"),
                name: pkg.to_owned(),
                publisher: None,
                sources: vec![src.to_owned()],
                run_count: None,
                last_run_at: ts,
                vss_path: None,
                drive_letter: "/".into(),
                is_game: false,
                install_dir: None,
                appdata_paths: Vec::new(),
                leftover_bytes: 0,
            });
        };

        if let Some(txt) = read_tail(Path::new("/var/log/pacman.log"), 512 * 1024) {
            for line in txt.lines().rev().take(400) {
                if let Some(idx) = line.find("[ALPM] removed ") {
                    let pkg = line[idx + 15..].split_whitespace().next().unwrap_or("");
                    let ts = line
                        .strip_prefix('[')
                        .and_then(|s| s.split(']').next())
                        .map(str::to_owned);
                    push_pkg(pkg, "pacman.log", ts);
                }
            }
        }
        if let Some(txt) = read_tail(Path::new("/var/log/dpkg.log"), 512 * 1024) {
            for line in txt.lines().rev().take(400) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 && parts[2] == "remove" {
                    let pkg = parts[3].split(':').next().unwrap_or("");
                    let ts = Some(format!("{}T{}", parts[0], parts[1]));
                    push_pkg(pkg, "dpkg.log", ts);
                }
            }
        }
        if let Some(txt) = read_tail(Path::new("/var/log/dnf.log"), 512 * 1024) {
            for line in txt.lines().rev().take(400) {
                if let Some(idx) = line.find("Erased: ") {
                    let raw = line[idx + 8..].split_whitespace().next().unwrap_or("");
                    let pkg = raw.split('-').next().unwrap_or("");
                    push_pkg(pkg, "dnf.log", None);
                }
            }
        }
    }

    pub fn collect_linux_forensics(letter: &str) -> ForensicSweep {
        let mut sweep = ForensicSweep::default();
        collect_linux_snapshots(letter, &mut sweep);
        for home in linux_homes() {
            collect_desktop_and_xbel(&home, letter, &mut sweep);
            collect_game_launchers(&home, letter, &mut sweep);
            collect_appdata_and_downloads(&home, letter, &mut sweep);
        }
        collect_package_removal_logs(&mut sweep);
        sweep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_volume_map() -> VolumeMap {
        let mut guid_to_drive = HashMap::new();
        guid_to_drive.insert(
            "{a1b2c3d4-1111-2222-3333-444455556666}".to_owned(),
            "C:".to_owned(),
        );
        guid_to_drive.insert(
            "{f0e1d2c3-aaaa-bbbb-cccc-ddddeeeeffff}".to_owned(),
            "D:".to_owned(),
        );
        let mut serial_to_drive = HashMap::new();
        serial_to_drive.insert(0xCAFE_BABEu32, "C:".to_owned());
        serial_to_drive.insert(0xDEAD_BEEFu32, "D:".to_owned());
        VolumeMap {
            device_to_drive: vec![
                ("\\device\\harddiskvolume1".to_owned(), "C:".to_owned()),
                ("\\device\\harddiskvolume10".to_owned(), "E:".to_owned()),
                ("\\device\\harddiskvolume3".to_owned(), "D:".to_owned()),
            ],
            guid_to_drive,
            serial_to_drive,
            system_drive: "C:".to_owned(),
        }
    }

    #[test]
    fn volume_guid_serial_and_device_normalization_handles_all_windows_formats() {
        let vm = test_volume_map();

        // Standard \VOLUME{guid}\...
        assert_eq!(
            normalize_windows_path(
                "\\VOLUME{f0e1d2c3-aaaa-bbbb-cccc-ddddeeeeffff}\\Games\\Hades\\Hades.exe",
                &vm,
                None,
            )
            .as_deref(),
            Some("D:\\Games\\Hades\\Hades.exe")
        );

        // Prefetch Win10/11 \VOLUME{filetime-serial}\...
        assert_eq!(
            normalize_windows_path(
                "\\VOLUME{01da3a4b5c6d7e8f-deadbeef}\\SteamLibrary\\Cyberpunk2077.exe",
                &vm,
                None,
            )
            .as_deref(),
            Some("D:\\SteamLibrary\\Cyberpunk2077.exe")
        );
        assert_eq!(
            normalize_windows_path(
                "\\VOLUME{01da3a4b5c6d7e8f-cafebabe}\\Users\\Ali\\Discord.exe",
                &vm,
                None,
            )
            .as_deref(),
            Some("C:\\Users\\Ali\\Discord.exe")
        );

        // \Device\HarddiskVolume10 must NOT match \Device\HarddiskVolume1!
        assert_eq!(
            normalize_windows_path(
                "\\Device\\HarddiskVolume10\\Apps\\Blender\\blender.exe",
                &vm,
                None,
            )
            .as_deref(),
            Some("E:\\Apps\\Blender\\blender.exe")
        );
        assert_eq!(
            normalize_windows_path(
                "\\Device\\HarddiskVolume1\\Program Files\\Spotify\\Spotify.exe",
                &vm,
                None,
            )
            .as_deref(),
            Some("C:\\Program Files\\Spotify\\Spotify.exe")
        );

        // Unmapped volume returns None without fallback (prevents false cross-drive mapping)
        assert_eq!(
            normalize_windows_path("\\VOLUME{01da000000000000-11223344}\\App.exe", &vm, None),
            None
        );
        // With explicit fallback letter it maps cleanly
        assert_eq!(
            normalize_windows_path(
                "\\VOLUME{01da000000000000-11223344}\\App.exe",
                &vm,
                Some("F"),
            )
            .as_deref(),
            Some("F:\\App.exe")
        );
    }

    #[test]
    fn firewall_rules_and_game_config_store_parsers_extract_apps_and_games() {
        let vm = test_volume_map();

        let fw_rule = "v2.31|Action=Allow|Active=TRUE|Dir=In|Protocol=6|Profile=Private|App=D:\\SteamLibrary\\steamapps\\common\\Hades\\x64\\Hades.exe|Name=Hades|Desc=Hades|";
        let (exe, name) = parse_firewall_rule_value(fw_rule, &vm).unwrap();
        assert_eq!(
            exe,
            "D:\\SteamLibrary\\steamapps\\common\\Hades\\x64\\Hades.exe"
        );
        assert_eq!(name.as_deref(), Some("Hades"));

        // Indirect Windows resource Name=@FirewallAPI.dll should be ignored while keeping exe
        let fw_indirect =
            "v2.31|Action=Allow|App=C:\\Apps\\ObsStudio\\obs64.exe|Name=@FirewallAPI.dll,-28752|";
        let (exe2, name2) = parse_firewall_rule_value(fw_indirect, &vm).unwrap();
        assert_eq!(exe2, "C:\\Apps\\ObsStudio\\obs64.exe");
        assert_eq!(name2, None);

        // System rule without .exe is skipped
        assert!(
            parse_firewall_rule_value("v2.31|Action=Allow|App=System|Name=Kernel|", &vm).is_none()
        );

        // GameConfigStore game entry
        let gcs = parse_game_config_entry(
            "D:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe",
            Some("Cyberpunk 2077"),
            &vm,
        )
        .unwrap();
        assert_eq!(
            gcs.exe_path,
            "D:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe"
        );
        assert_eq!(gcs.title.as_deref(), Some("Cyberpunk 2077"));
        assert!(gcs.is_game);

        // Fullscreen browser/media player in GameConfigStore is NOT tagged as game
        let gcs_vlc = parse_game_config_entry(
            "C:\\Program Files\\VideoLAN\\VLC\\vlc.exe",
            Some("VLC media player"),
            &vm,
        )
        .unwrap();
        assert!(!gcs_vlc.is_game);
    }

    #[test]
    fn noise_filter_blocks_windows_system_binaries_and_keeps_real_apps() {
        for noisy in [
            "C:\\Windows\\System32\\svchost.exe",
            "C:\\Windows\\System32\\conhost.exe",
            "C:\\Windows\\System32\\RuntimeBroker.exe",
            "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application\\msedgewebview2.exe",
            "C:\\Users\\Ali\\AppData\\Local\\Temp\\is-A1B2C.tmp\\setup_temp.exe",
            "C:\\Program Files\\MyApp\\unins000.exe",
            "C:\\Program Files\\MyApp\\uninstall.exe",
            "C:\\Tools\\smartctl.exe",
            "C:\\Program Files\\SSD Sentinel\\ssd-sentinel.exe",
            "C:\\Apps\\4f8a9c12.exe",
            "C:\\Apps\\ab.exe",
            "TIWORKER.EXE",
            "COMPATTELRUNNER.EXE",
        ] {
            assert!(is_noise_app_exe(noisy, None), "Expected noise: {noisy}");
        }

        for real in [
            "C:\\Users\\Ali\\AppData\\Roaming\\Spotify\\Spotify.exe",
            "D:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe",
            "C:\\Program Files\\7-Zip\\7zFM.exe",
            "C:\\Program Files\\VideoLAN\\VLC\\vlc.exe",
            "C:\\Program Files\\obs-studio\\bin\\64bit\\obs64.exe",
            "D:\\Tools\\Rufus\\rufus-4.5.exe",
            "C:\\Users\\Ali\\Downloads\\DiscordSetup.exe",
        ] {
            assert!(
                !is_noise_app_exe(real, None),
                "False positive noise: {real}"
            );
        }
    }

    #[test]
    fn canonical_app_key_and_prefetch_filename_fallback_normalize_cleanly() {
        assert_eq!(
            canonical_app_key("C:\\Users\\Ali\\Downloads\\SpotifySetup.exe"),
            "spotify"
        );
        assert_eq!(
            canonical_app_key("C:\\Users\\Ali\\AppData\\Roaming\\Spotify\\Spotify.exe"),
            "spotify"
        );
        assert_eq!(canonical_app_key("Spotify"), "spotify");
        assert_eq!(
            canonical_app_key("OBS-Studio-30.2.3-Windows-Installer.exe"),
            "obsstudio"
        );
        assert_eq!(canonical_app_key("obs64.exe"), "obsstudio");
        assert_eq!(canonical_app_key("Hades-Win64-Shipping.exe"), "hades");
        assert_eq!(
            winget_command_for_app("spotify", "Spotify"),
            "winget install --id Spotify.Spotify -e"
        );
        assert_eq!(
            winget_command_for_app("mycustomtool", "My Custom Tool"),
            "winget search \"My Custom Tool\""
        );

        let fb = parse_prefetch_filename_fallback(
            "DISCORD.EXE-A1B2C3D4.pf",
            Some("2026-05-01T10:00:00+00:00".into()),
        )
        .unwrap();
        assert_eq!(fb.exe_name, "DISCORD.EXE");
        assert_eq!(fb.full_path, None);
        assert_eq!(fb.run_count, 1);
        assert!(parse_prefetch_filename_fallback("NOTVALID.pf", None).is_none());
    }

    #[test]
    fn rot13_and_userassist_72byte_decoder_extract_runs_and_timestamp() {
        assert_eq!(
            rot13_decode("P:\\Cebtenz Svyrf\\Ncc\\tnzr.rkr"),
            "C:\\Program Files\\App\\game.exe"
        );
        let mut blob = [0u8; 72];
        blob[4..8].copy_from_slice(&17u32.to_le_bytes());
        let ft = WINDOWS_EPOCH_OFFSET_100NS + 1_730_000_000 * 10_000_000;
        blob[60..68].copy_from_slice(&ft.to_le_bytes());
        let (runs, ts) = parse_userassist_72(&blob).unwrap();
        assert_eq!(runs, 17);
        assert!(ts.unwrap().starts_with("2024-10-27"));
    }

    #[test]
    fn shimcache_10ts_parser_reads_paths_and_timestamps() {
        let mut data = vec![0u8; 52]; // Win10 global header
        let path_u16: Vec<u16> = "\\??\\D:\\Games\\CyberApp.exe".encode_utf16().collect();
        let path_bytes: Vec<u8> = path_u16.iter().flat_map(|w| w.to_le_bytes()).collect();
        let entry_body_len = (2 + path_bytes.len() + 8 + 4) as u32;
        data.extend_from_slice(b"10ts");
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&entry_body_len.to_le_bytes());
        data.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
        data.extend_from_slice(&path_bytes);
        let ft = WINDOWS_EPOCH_OFFSET_100NS + 1_720_000_000 * 10_000_000;
        data.extend_from_slice(&ft.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());

        let parsed = parse_shimcache_10ts(&data);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "D:\\Games\\CyberApp.exe");
        assert!(parsed[0].1.as_deref().unwrap().starts_with("2024-07-03"));
    }

    #[test]
    fn prefetch_scca_v30_extracts_exe_path_run_count_and_timestamp() {
        let mut pf = vec![0u8; 512];
        pf[0..4].copy_from_slice(&30u32.to_le_bytes());
        pf[4..8].copy_from_slice(b"SCCA");
        for (i, w) in "DISCORD.EXE".encode_utf16().enumerate() {
            pf[0x10 + i * 2..0x12 + i * 2].copy_from_slice(&w.to_le_bytes());
        }
        let strings_off = 0x100usize;
        let full_str: Vec<u16> = "\\VOLUME{123}\\USERS\\DEMO\\DISCORD.EXE"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let s_bytes: Vec<u8> = full_str.iter().flat_map(|w| w.to_le_bytes()).collect();
        pf[0x64..0x68].copy_from_slice(&(strings_off as u32).to_le_bytes());
        pf[0x68..0x6C].copy_from_slice(&(s_bytes.len() as u32).to_le_bytes());
        let ft = WINDOWS_EPOCH_OFFSET_100NS + 1_735_000_000 * 10_000_000;
        pf[0x80..0x88].copy_from_slice(&ft.to_le_bytes());
        pf[0xD0..0xD4].copy_from_slice(&24u32.to_le_bytes());
        pf[strings_off..strings_off + s_bytes.len()].copy_from_slice(&s_bytes);

        let parsed = parse_prefetch_scca(&pf).unwrap();
        assert_eq!(parsed.exe_name, "DISCORD.EXE");
        assert_eq!(parsed.run_count, 24);
        assert_eq!(
            parsed.full_path.as_deref(),
            Some("\\VOLUME{123}\\USERS\\DEMO\\DISCORD.EXE")
        );
    }

    #[test]
    fn usn_record_v2_and_indx_slack_and_lnk_parsers_work() {
        // USN_RECORD_V2 deleted file
        let fname: Vec<u16> = "SteamSetup.exe".encode_utf16().collect();
        let fbytes: Vec<u8> = fname.iter().flat_map(|w| w.to_le_bytes()).collect();
        let rec_len = (60 + fbytes.len()).div_ceil(8) * 8;
        let mut usn = vec![0u8; rec_len];
        usn[0..4].copy_from_slice(&(rec_len as u32).to_le_bytes());
        usn[4..6].copy_from_slice(&2u16.to_le_bytes());
        usn[8..16].copy_from_slice(&777u64.to_le_bytes());
        usn[16..24].copy_from_slice(&55u64.to_le_bytes());
        let ft = WINDOWS_EPOCH_OFFSET_100NS + 1_740_000_000 * 10_000_000;
        usn[32..40].copy_from_slice(&ft.to_le_bytes());
        usn[40..44].copy_from_slice(&0x0000_0200u32.to_le_bytes()); // FILE_DELETE
        usn[56..58].copy_from_slice(&(fbytes.len() as u16).to_le_bytes());
        usn[58..60].copy_from_slice(&60u16.to_le_bytes());
        usn[60..60 + fbytes.len()].copy_from_slice(&fbytes);

        let usn_parsed = parse_usn_records_v2(&usn);
        assert_eq!(usn_parsed.len(), 1);
        assert_eq!(usn_parsed[0].file_name, "SteamSetup.exe");
        assert_eq!(usn_parsed[0].parent_ref, 55);

        // INDX slack ghost entry
        let mut indx = vec![0u8; 4096];
        indx[0..4].copy_from_slice(b"INDX");
        indx[4..6].copy_from_slice(&40u16.to_le_bytes());
        indx[6..8].copy_from_slice(&1u16.to_le_bytes());
        indx[28..32].copy_from_slice(&200u32.to_le_bytes()); // slack starts at 224
        let gpos = 256usize;
        let gname: Vec<u16> = "OyunSave.zip".encode_utf16().collect();
        let glen = (16 + 66 + gname.len() * 2).div_ceil(8) * 8;
        indx[gpos + 8..gpos + 10].copy_from_slice(&(glen as u16).to_le_bytes());
        indx[gpos + 10..gpos + 12].copy_from_slice(&((66 + gname.len() * 2) as u16).to_le_bytes());
        indx[gpos + 16..gpos + 24].copy_from_slice(&88u64.to_le_bytes());
        indx[gpos + 16 + 16..gpos + 16 + 24].copy_from_slice(&ft.to_le_bytes());
        indx[gpos + 16 + 48..gpos + 16 + 56].copy_from_slice(&65536u64.to_le_bytes());
        indx[gpos + 16 + 64] = gname.len() as u8;
        indx[gpos + 16 + 65] = 1;
        for (i, w) in gname.iter().enumerate() {
            indx[gpos + 16 + 66 + i * 2..gpos + 16 + 68 + i * 2].copy_from_slice(&w.to_le_bytes());
        }
        let ghosts = parse_indx_slack(&indx);
        assert_eq!(ghosts.len(), 1);
        assert_eq!(ghosts[0].file_name, "OyunSave.zip");
        assert_eq!(ghosts[0].real_size, 65536);

        // MS-SHLLINK (.lnk)
        let mut lnk = vec![0u8; 76];
        lnk[0..4].copy_from_slice(&0x4Cu32.to_le_bytes());
        lnk[4..20].copy_from_slice(&[
            0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x46,
        ]);
        lnk[20..24].copy_from_slice(&0x02u32.to_le_bytes()); // HasLinkInfo only
        let target = b"D:\\Apps\\Blender\\blender.exe\0";
        let li_sz = (28 + target.len()) as u32;
        lnk.extend_from_slice(&li_sz.to_le_bytes());
        lnk.extend_from_slice(&28u32.to_le_bytes());
        lnk.extend_from_slice(&1u32.to_le_bytes()); // VolumeIDAndLocalBasePath
        lnk.extend_from_slice(&0u32.to_le_bytes());
        lnk.extend_from_slice(&28u32.to_le_bytes()); // LocalBasePathOffset
        lnk.extend_from_slice(&0u32.to_le_bytes());
        lnk.extend_from_slice(&0u32.to_le_bytes());
        lnk.extend_from_slice(target);
        assert_eq!(
            parse_lnk_target_path(&lnk).as_deref(),
            Some("D:\\Apps\\Blender\\blender.exe")
        );
    }
}
