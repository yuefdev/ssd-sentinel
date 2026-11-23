use crate::{
    core::{lock, Core, JobRequest},
    forensics::{self, AppDataEntry, ForensicAppTrace},
    jobs::{cancelled, phase},
    media_repair,
    model::Disk,
    platform,
};
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::Instant,
};

const WINDOWS_EPOCH_OFFSET_100NS: i64 = 116_444_736_000_000_000;
const MAX_RESULTS: usize = 800;
const MAX_MFT_COLLECT: usize = 8_000;
const MAX_MFT_PROBE: usize = 800;
const MAX_MFT_DIRS: usize = 250_000;
const MAX_CARVED_ITEMS: usize = 500;
const MAX_SYSTEM_CARVED: usize = 40;
const MAX_APPS: usize = 100;
const MAX_RESTORE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MEDIA_REPAIR_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MEDIA_INSPECT_BYTES: usize = 8 * 1024 * 1024;
const MAX_PREVIEW_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PREVIEW_TEXT_BYTES: usize = 16 * 1024;
const FULL_SCAN_CAP_BYTES: u64 = 512 * 1024 * 1024 * 1024;

fn default_openable() -> bool {
    true
}

fn default_app_category() -> String {
    "Uygulama".into()
}

fn default_recovery_status() -> String {
    "reinstall_only".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveredItem {
    pub id: String,
    pub name: String,
    pub original_path: String,
    pub extension: String,
    pub category: String,
    #[serde(default)]
    pub sub_type: String,
    #[serde(default)]
    pub product_name: Option<String>,
    #[serde(default)]
    pub publisher: Option<String>,
    pub size_bytes: u64,
    pub deleted_at: Option<String>,
    pub source: String,
    pub confidence: u32,
    pub status_label: String,
    pub entropy: f64,
    pub trim_zeroed: bool,
    pub ai_summary: String,
    pub source_letter: String,
    pub locator: Value,
    #[serde(default)]
    pub media_status: Option<String>,
    #[serde(default = "default_openable")]
    pub openable: bool,
    #[serde(default)]
    pub repair_note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeletedAppBundle {
    pub name: String,
    pub publisher: Option<String>,
    pub exe_path: String,
    pub installer_found: bool,
    pub recoverable_item_id: Option<String>,
    pub appdata_paths: Vec<String>,
    pub winget_hint: String,
    pub confidence: u32,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub run_count: Option<u32>,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub vss_recoverable: bool,
    #[serde(default = "default_app_category")]
    pub category: String,
    #[serde(default = "default_recovery_status")]
    pub recovery_status: String,
    #[serde(default)]
    pub leftover_bytes: u64,
    #[serde(default)]
    pub install_dir: Option<String>,
    #[serde(default)]
    pub drive_letter: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedMftDeleted {
    pub record_index: u64,
    pub parent_record: u64,
    pub name: String,
    pub real_size: u64,
    pub deleted_at: Option<String>,
    pub resident_bytes: Option<Vec<u8>>,
    pub runs: Vec<(i64, u64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MftEntry {
    Directory {
        record_index: u64,
        parent_record: u64,
        name: String,
    },
    DeletedFile(ParsedMftDeleted),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeIntelligence {
    pub is_64bit: bool,
    pub is_dll: bool,
    pub subsystem: u16,
    pub estimated_size: u64,
    pub rsrc_offset: Option<(u64, u64)>,
    pub installer_kind: Option<String>,
    pub product_name: Option<String>,
    pub original_filename: Option<String>,
    pub file_description: Option<String>,
    pub company_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CarvedSignature {
    pub ext: &'static str,
    pub category: &'static str,
    pub sub_type: String,
    pub estimated_size: u64,
    pub product_name: Option<String>,
    pub publisher: Option<String>,
    pub suggested_name: Option<String>,
    pub rsrc_offset: Option<(u64, u64)>,
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("Kurtarma verisi uzunluğu geçersiz.".into());
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16)
            .map_err(|_| "Kurtarma verisi çözümlenemedi.".to_owned())?;
        out.push(byte);
    }
    Ok(out)
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn filetime_to_rfc3339(ft: i64) -> Option<String> {
    if ft <= WINDOWS_EPOCH_OFFSET_100NS {
        return None;
    }
    let secs = (ft - WINDOWS_EPOCH_OFFSET_100NS) / 10_000_000;
    Utc.timestamp_opt(secs, 0)
        .single()
        .map(|dt| dt.to_rfc3339())
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

pub fn parse_recycle_i_bytes(bytes: &[u8]) -> Option<(u64, Option<String>, String)> {
    if bytes.len() < 26 {
        return None;
    }
    let version = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let size = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let ft = i64::from_le_bytes(bytes[16..24].try_into().ok()?);
    let path_slice = match version {
        1 => &bytes[24..],
        2 if bytes.len() >= 30 => {
            let chars = u32::from_le_bytes(bytes[24..28].try_into().ok()?) as usize;
            let max_end = 28usize
                .saturating_add(chars.saturating_mul(2))
                .min(bytes.len());
            &bytes[28..max_end]
        }
        _ => return None,
    };
    let original_path = decode_utf16le(path_slice);
    if original_path.trim().is_empty() {
        return None;
    }
    Some((size, filetime_to_rfc3339(ft), original_path))
}

pub fn decode_ntfs_data_runs(bytes: &[u8]) -> Vec<(i64, u64)> {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut cur_lcn = 0i64;
    while pos < bytes.len() {
        let header = bytes[pos];
        if header == 0 {
            break;
        }
        pos += 1;
        let len_sz = (header & 0x0F) as usize;
        let off_sz = (header >> 4) as usize;
        if len_sz == 0 || len_sz > 8 || off_sz > 8 || pos + len_sz + off_sz > bytes.len() {
            break;
        }
        let mut run_len = 0u64;
        for i in 0..len_sz {
            run_len |= u64::from(bytes[pos + i]) << (i * 8);
        }
        pos += len_sz;
        if off_sz == 0 {
            continue;
        }
        let mut delta = 0i64;
        for i in 0..off_sz {
            delta |= i64::from(bytes[pos + i]) << (i * 8);
        }
        if off_sz < 8 && (bytes[pos + off_sz - 1] & 0x80) != 0 {
            for i in off_sz..8 {
                delta |= 0xFFi64 << (i * 8);
            }
        }
        pos += off_sz;
        let Some(next_lcn) = cur_lcn.checked_add(delta) else {
            break;
        };
        cur_lcn = next_lcn;
        if cur_lcn >= 0 && run_len > 0 {
            runs.push((cur_lcn, run_len));
        }
    }
    runs
}

fn apply_usa_fixup(record: &[u8]) -> Option<Vec<u8>> {
    if record.len() < 1024 || &record[0..4] != b"FILE" {
        return None;
    }
    let mut buf = record[..1024].to_vec();
    let usa_off = u16::from_le_bytes(buf[4..6].try_into().ok()?) as usize;
    let usa_cnt = u16::from_le_bytes(buf[6..8].try_into().ok()?) as usize;
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
    Some(buf)
}

#[cfg(windows)]
fn extract_mft_runs(record: &[u8], cluster_bytes: u64) -> Option<Vec<(i64, u64)>> {
    let _ = cluster_bytes;
    let buf = apply_usa_fixup(record)?;
    let mut attr_off = u16::from_le_bytes(buf[20..22].try_into().ok()?) as usize;
    while attr_off + 16 <= buf.len() {
        let attr_type = u32::from_le_bytes(buf[attr_off..attr_off + 4].try_into().ok()?);
        if attr_type == 0xFFFF_FFFF {
            break;
        }
        let attr_len =
            u32::from_le_bytes(buf[attr_off + 4..attr_off + 8].try_into().ok()?) as usize;
        if attr_len < 16 || attr_off + attr_len > buf.len() {
            break;
        }
        let non_resident = buf[attr_off + 8] != 0;
        let name_len = buf[attr_off + 9];
        if attr_type == 0x80 && name_len == 0 && non_resident && attr_off + 56 <= buf.len() {
            let run_off =
                u16::from_le_bytes(buf[attr_off + 32..attr_off + 34].try_into().ok()?) as usize;
            if attr_off + run_off <= attr_off + attr_len {
                let runs = decode_ntfs_data_runs(&buf[attr_off + run_off..attr_off + attr_len]);
                if !runs.is_empty() {
                    return Some(runs);
                }
            }
        }
        attr_off += attr_len;
    }
    None
}

pub fn parse_mft_entry(record: &[u8], record_index: u64, cluster_bytes: u64) -> Option<MftEntry> {
    let buf = apply_usa_fixup(record)?;
    let flags = u16::from_le_bytes(buf[22..24].try_into().ok()?);
    let in_use = (flags & 0x0001) != 0;
    let is_dir = (flags & 0x0002) != 0;
    // Skip active files; keep directories (for full path resolution) and deleted files.
    if in_use && !is_dir {
        return None;
    }

    let mut attr_off = u16::from_le_bytes(buf[20..22].try_into().ok()?) as usize;
    let mut best_name = String::new();
    let mut best_ns = u8::MAX;
    let mut parent_record = 5u64;
    let mut deleted_at = None;
    let mut resident_bytes = None;
    let mut runs = Vec::new();
    let mut real_size = 0u64;

    while attr_off + 16 <= buf.len() {
        let attr_type = u32::from_le_bytes(buf[attr_off..attr_off + 4].try_into().ok()?);
        if attr_type == 0xFFFF_FFFF {
            break;
        }
        let attr_len =
            u32::from_le_bytes(buf[attr_off + 4..attr_off + 8].try_into().ok()?) as usize;
        if attr_len < 16 || attr_off + attr_len > buf.len() {
            break;
        }
        let non_resident = buf[attr_off + 8] != 0;
        let name_len = buf[attr_off + 9];

        if attr_type == 0x10 && !non_resident && !is_dir && attr_off + 22 <= buf.len() {
            let val_off =
                u16::from_le_bytes(buf[attr_off + 20..attr_off + 22].try_into().ok()?) as usize;
            if attr_off + val_off + 16 <= attr_off + attr_len {
                let ft = i64::from_le_bytes(
                    buf[attr_off + val_off + 8..attr_off + val_off + 16]
                        .try_into()
                        .ok()?,
                );
                deleted_at = filetime_to_rfc3339(ft);
            }
        } else if attr_type == 0x30 && !non_resident && attr_off + 22 <= buf.len() {
            let val_off =
                u16::from_le_bytes(buf[attr_off + 20..attr_off + 22].try_into().ok()?) as usize;
            let base = attr_off + val_off;
            if base + 66 <= attr_off + attr_len {
                let p_raw = u64::from_le_bytes(buf[base..base + 8].try_into().ok()?);
                let p_idx = p_raw & 0x0000_FFFF_FFFF_FFFF;
                let fn_chars = buf[base + 64] as usize;
                let ns = buf[base + 65];
                let name_end = (base + 66 + fn_chars * 2).min(attr_off + attr_len);
                let decoded = decode_utf16le(&buf[base + 66..name_end]);
                if !decoded.is_empty() && !decoded.starts_with('$') && (best_ns == 2 || ns != 2) {
                    best_name = decoded;
                    best_ns = ns;
                    parent_record = p_idx;
                }
            }
        } else if attr_type == 0x80 && name_len == 0 && !is_dir {
            if !non_resident && attr_off + 22 <= buf.len() {
                let val_len =
                    u32::from_le_bytes(buf[attr_off + 16..attr_off + 20].try_into().ok()?) as usize;
                let val_off =
                    u16::from_le_bytes(buf[attr_off + 20..attr_off + 22].try_into().ok()?) as usize;
                let start = attr_off + val_off;
                let end = start.saturating_add(val_len).min(attr_off + attr_len);
                if start <= end {
                    real_size = (end - start) as u64;
                    resident_bytes = Some(buf[start..end].to_vec());
                }
            } else if non_resident && attr_off + 56 <= buf.len() {
                let run_off =
                    u16::from_le_bytes(buf[attr_off + 32..attr_off + 34].try_into().ok()?) as usize;
                let rs = u64::from_le_bytes(buf[attr_off + 48..attr_off + 56].try_into().ok()?);
                if attr_off + run_off <= attr_off + attr_len {
                    runs = decode_ntfs_data_runs(&buf[attr_off + run_off..attr_off + attr_len]);
                    let total_run_bytes = runs
                        .iter()
                        .map(|(_, c)| c.saturating_mul(cluster_bytes))
                        .sum::<u64>();
                    real_size = if rs > 0 && rs <= total_run_bytes.saturating_add(cluster_bytes) {
                        rs
                    } else {
                        total_run_bytes
                    };
                }
            }
        }
        attr_off += attr_len;
    }

    if best_name.is_empty() {
        return None;
    }
    if is_dir {
        return Some(MftEntry::Directory {
            record_index,
            parent_record,
            name: best_name,
        });
    }
    if resident_bytes.is_none() && runs.is_empty() {
        return None;
    }
    Some(MftEntry::DeletedFile(ParsedMftDeleted {
        record_index,
        parent_record,
        name: best_name,
        real_size,
        deleted_at,
        resident_bytes,
        runs,
    }))
}

pub fn parse_mft_record(
    record: &[u8],
    record_index: u64,
    cluster_bytes: u64,
) -> Option<ParsedMftDeleted> {
    match parse_mft_entry(record, record_index, cluster_bytes)? {
        MftEntry::DeletedFile(f) => Some(f),
        MftEntry::Directory { .. } => None,
    }
}

pub fn reconstruct_mft_path(
    letter: &str,
    parent_record: u64,
    file_name: &str,
    dirs: &HashMap<u64, (u64, String)>,
) -> String {
    let mut parts = Vec::new();
    let mut cur = parent_record;
    let mut seen = HashSet::new();
    let mut reached_root = cur == 5;
    for _ in 0..12 {
        if cur == 5 {
            reached_root = true;
            break;
        }
        if !seen.insert(cur) {
            break;
        }
        let Some((next_parent, dir_name)) = dirs.get(&cur) else {
            break;
        };
        parts.push(dir_name.clone());
        if *next_parent == cur || *next_parent == 5 {
            reached_root = *next_parent == 5;
            break;
        }
        cur = *next_parent;
    }
    parts.reverse();
    if parts.is_empty() {
        return format!("{letter}:\\{file_name}");
    }
    let joined = parts.join("\\");
    if reached_root {
        format!("{letter}:\\{joined}\\{file_name}")
    } else {
        format!("{letter}:\\...\\{joined}\\{file_name}")
    }
}

pub fn is_junk_noise_path(path_or_name: &str, ext: &str) -> bool {
    let ext_lower = ext.trim_start_matches('.').to_ascii_lowercase();
    if matches!(
        ext_lower.as_str(),
        "tmp" | "etl" | "pf" | "lock" | "dmp" | "cache" | "chk" | "mui" | "evtx" | "dat"
    ) {
        return true;
    }
    let lower = path_or_name.to_ascii_lowercase();
    if lower.contains("$recycle.bin") {
        return false;
    }
    const JUNK_DIRS: &[&str] = &[
        "\\windows\\temp\\",
        "\\windows\\prefetch\\",
        "\\windows\\servicing\\",
        "\\windows\\winsxs\\",
        "\\windows\\softwaredistribution\\",
        "\\cache\\cache_data\\",
        "\\code cache\\",
        "\\gpucache\\",
        "\\shadercache\\",
        "\\dawncache\\",
        "\\appdata\\local\\temp\\",
        "\\appdata\\local\\google\\chrome\\user data\\",
        "\\appdata\\local\\microsoft\\edge\\user data\\",
        "\\appdata\\local\\mozilla\\firefox\\profiles\\",
        "\\programdata\\microsoft\\windows defender\\",
        "\\$extend\\",
    ];
    JUNK_DIRS.iter().any(|needle| lower.contains(needle))
}

fn is_game_path(path_lower: &str) -> bool {
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
        "\\gog games\\",
        "unityplayer",
        "unrealengine",
    ];
    GAME_MARKERS.iter().any(|m| path_lower.contains(m))
}

pub fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for &b in bytes {
        counts[usize::from(b)] += 1;
    }
    let len = bytes.len() as f64;
    let mut entropy = 0.0f64;
    for &c in &counts {
        if c > 0 {
            let p = c as f64 / len;
            entropy -= p * p.log2();
        }
    }
    entropy
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Decodes a UTF-16LE `VS_VERSION_INFO` string entry (`ProductName`, `OriginalFilename`, etc.).
pub fn extract_utf16_version_field(bytes: &[u8], key: &str) -> Option<String> {
    let mut pat = Vec::with_capacity((key.len() + 1) * 2);
    for u in key.encode_utf16() {
        pat.extend_from_slice(&u.to_le_bytes());
    }
    pat.extend_from_slice(&[0, 0]);

    let mut search_from = 0usize;
    while search_from + pat.len() + 4 <= bytes.len() {
        let rel = find_bytes(&bytes[search_from..], &pat)?;
        let pos = search_from + rel;
        search_from = pos + 2;

        // `String` header before szKey is wLength(2) + wValueLength(2) + wType(2).
        if pos >= 6 {
            let w_val_len = u16::from_le_bytes([bytes[pos - 4], bytes[pos - 3]]);
            let w_type = u16::from_le_bytes([bytes[pos - 2], bytes[pos - 1]]);
            if w_type != 1 || !(2..=128).contains(&w_val_len) {
                continue;
            }
        }
        let mut val_off = pos + pat.len();
        let mut skipped = 0usize;
        while skipped < 4 && val_off + 2 <= bytes.len() && bytes[val_off..val_off + 2] == [0, 0] {
            val_off += 2;
            skipped += 2;
        }
        let end = (val_off + 180).min(bytes.len());
        let decoded = decode_utf16le(&bytes[val_off..end]);
        let clean = decoded.trim();
        let valid_chars = clean
            .chars()
            .all(|c| !c.is_control() && (c.is_ascii_graphic() || c == ' ' || c.is_alphanumeric()));
        if valid_chars && clean.len() >= 2 && clean.len() <= 96 {
            return Some(clean.to_owned());
        }
    }
    None
}

pub fn extract_pe_metadata(meta_bytes: &[u8], info: &mut PeIntelligence) {
    if info.original_filename.is_none() {
        info.original_filename = extract_utf16_version_field(meta_bytes, "OriginalFilename");
    }
    if info.product_name.is_none() {
        info.product_name = extract_utf16_version_field(meta_bytes, "ProductName");
    }
    if info.file_description.is_none() {
        info.file_description = extract_utf16_version_field(meta_bytes, "FileDescription");
    }
    if info.company_name.is_none() {
        info.company_name = extract_utf16_version_field(meta_bytes, "CompanyName");
    }
    if info.installer_kind.is_none() {
        if find_bytes(meta_bytes, b"NullsoftInst").is_some()
            || find_bytes(meta_bytes, b"Nullsoft Install System").is_some()
        {
            info.installer_kind = Some("NSIS Kurulum Paketi".into());
        } else if find_bytes(meta_bytes, b"Inno Setup").is_some()
            || find_bytes(meta_bytes, b"JR.Inno.Setup").is_some()
        {
            info.installer_kind = Some("Inno Setup Kurulumu".into());
        } else if find_bytes(meta_bytes, b"7z\xBC\xAF\x27\x1C").is_some() {
            info.installer_kind = Some("7-Zip SFX Kurulumu".into());
        } else if find_bytes(meta_bytes, b"WixBundle").is_some()
            || find_bytes(meta_bytes, b"Windows Installer").is_some()
        {
            info.installer_kind = Some("WiX / MSI Paketi".into());
        } else if find_bytes(meta_bytes, b"UnityPlayer").is_some()
            || find_bytes(meta_bytes, b"UnrealEngine").is_some()
        {
            info.installer_kind = Some("Oyun Motoru İkilisi".into());
        }
    }
}

/// Structurally parses DOS/PE/COFF headers, sections, digital signatures, overlays, and UTF-16LE resources.
pub fn analyze_pe_binary(bytes: &[u8]) -> Option<PeIntelligence> {
    if bytes.len() < 256 || &bytes[0..2] != b"MZ" {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(bytes[0x3C..0x40].try_into().ok()?) as usize;
    if !(64..=2048).contains(&e_lfanew) || e_lfanew + 24 > bytes.len() {
        return None;
    }
    if &bytes[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
        return None;
    }

    let coff = e_lfanew + 4;
    let machine = u16::from_le_bytes([bytes[coff], bytes[coff + 1]]);
    if !matches!(machine, 0x8664 | 0x014C | 0xAA64) {
        return None;
    }
    let num_sections = u16::from_le_bytes([bytes[coff + 2], bytes[coff + 3]]) as usize;
    if num_sections == 0 || num_sections > 64 {
        return None;
    }
    let opt_size = u16::from_le_bytes([bytes[coff + 16], bytes[coff + 17]]) as usize;
    let characteristics = u16::from_le_bytes([bytes[coff + 18], bytes[coff + 19]]);
    if (characteristics & 0x0002) == 0 {
        return None;
    }
    let is_dll = (characteristics & 0x2000) != 0;

    let opt = coff + 20;
    if opt + 72 > bytes.len() || opt_size < 96 {
        return None;
    }
    let magic = u16::from_le_bytes([bytes[opt], bytes[opt + 1]]);
    let is_64bit = match magic {
        0x020B => true,
        0x010B => false,
        _ => return None,
    };
    let subsystem = u16::from_le_bytes([bytes[opt + 68], bytes[opt + 69]]);
    let file_align = u32::from_le_bytes(bytes[opt + 36..opt + 40].try_into().ok()?);
    if !(512..=65536).contains(&file_align) || !file_align.is_power_of_two() {
        return None;
    }
    let sec_dir_off = if is_64bit { opt + 144 } else { opt + 128 };
    let mut cert_end = 0u64;
    if sec_dir_off + 8 <= opt + opt_size && sec_dir_off + 8 <= bytes.len() {
        let cert_off = u64::from(u32::from_le_bytes(
            bytes[sec_dir_off..sec_dir_off + 4].try_into().ok()?,
        ));
        let cert_sz = u64::from(u32::from_le_bytes(
            bytes[sec_dir_off + 4..sec_dir_off + 8].try_into().ok()?,
        ));
        if cert_off > 0 && cert_sz > 0 && cert_off < MAX_RESTORE_BYTES {
            cert_end = cert_off.saturating_add(cert_sz);
        }
    }

    let sec_table = opt + opt_size;
    let mut max_sec_end = 0u64;
    let mut rsrc_offset = None;
    for s in 0..num_sections {
        let row = sec_table + s * 40;
        if row + 40 > bytes.len() {
            break;
        }
        let size_raw = u64::from(u32::from_le_bytes(
            bytes[row + 16..row + 20].try_into().ok()?,
        ));
        let ptr_raw = u64::from(u32::from_le_bytes(
            bytes[row + 20..row + 24].try_into().ok()?,
        ));
        if ptr_raw < MAX_RESTORE_BYTES && size_raw < MAX_RESTORE_BYTES {
            max_sec_end = max_sec_end.max(ptr_raw.saturating_add(size_raw));
            if &bytes[row..row + 5] == b".rsrc" && ptr_raw > 0 && size_raw >= 64 {
                rsrc_offset = Some((ptr_raw, size_raw));
            }
        }
    }

    // NSIS overlay exact archive stream sizing when unsigned (`\xEF\xBE\xAD\xDENullsoftInst`).
    let mut nsis_end = 0u64;
    if let Some(magic_pos) = find_bytes(bytes, b"\xEF\xBE\xAD\xDENullsoftInst") {
        if magic_pos >= 4 && magic_pos + 24 <= bytes.len() {
            let stream_sz = u64::from(u32::from_le_bytes(
                bytes[magic_pos + 20..magic_pos + 24].try_into().ok()?,
            ));
            if (512..MAX_RESTORE_BYTES).contains(&stream_sz) {
                nsis_end = (magic_pos as u64 - 4).saturating_add(stream_sz);
            }
        }
    }

    let estimated_size = max_sec_end
        .max(cert_end)
        .max(nsis_end)
        .clamp(4096, MAX_RESTORE_BYTES);

    let mut info = PeIntelligence {
        is_64bit,
        is_dll,
        subsystem,
        estimated_size,
        rsrc_offset,
        installer_kind: None,
        product_name: None,
        original_filename: None,
        file_description: None,
        company_name: None,
    };
    let meta_slice = match rsrc_offset {
        Some((ptr_raw, size_raw)) if (ptr_raw as usize) < bytes.len() => {
            let start = ptr_raw as usize;
            let end = start
                .saturating_add((size_raw as usize).min(512 * 1024))
                .min(bytes.len());
            &bytes[start..end]
        }
        _ => &bytes[..bytes.len().min(256 * 1024)],
    };
    extract_pe_metadata(meta_slice, &mut info);
    if info.installer_kind.is_none() {
        let ov_slice = &bytes[..bytes.len().min(512 * 1024)];
        extract_pe_metadata(ov_slice, &mut info);
    }
    Some(info)
}

fn pe_subtype_label(pe: &PeIntelligence) -> String {
    let arch = if pe.is_64bit { "64-bit" } else { "32-bit" };
    if pe.is_dll {
        return format!("{arch} DLL Kütüphanesi");
    }
    if let Some(ref inst) = pe.installer_kind {
        return format!("{arch} {inst}");
    }
    match pe.subsystem {
        1 => format!("{arch} Çekirdek Sürücüsü (.sys)"),
        3 => format!("{arch} Konsol Uygulaması"),
        _ => format!("{arch} Masaüstü Uygulaması"),
    }
}

fn file_name_lower(path_or_name: &str) -> String {
    path_or_name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

pub fn is_env_filename(path_or_name: &str) -> bool {
    let name = file_name_lower(path_or_name);
    name == ".env" || name.starts_with(".env.")
}

pub fn is_sensitive_dev_file(name_or_path: &str) -> bool {
    let name = file_name_lower(name_or_path);
    is_env_filename(&name)
        || matches!(
            name.as_str(),
            ".npmrc" | "secrets.json" | "secrets.yaml" | "secrets.yml" | "credentials.json"
        )
        || name.ends_with(".pem")
        || name.ends_with(".key")
}

pub fn is_dev_project_file(full_path: &str, ext: &str) -> bool {
    let name = file_name_lower(full_path);
    if is_env_filename(&name)
        || matches!(
            name.as_str(),
            ".gitignore"
                | ".dockerignore"
                | ".npmrc"
                | "dockerfile"
                | "docker-compose.yml"
                | "docker-compose.yaml"
                | "makefile"
                | "cargo.toml"
                | "package.json"
                | "tsconfig.json"
                | "pyproject.toml"
        )
    {
        return true;
    }
    let lower_ext = ext.trim_start_matches('.').to_ascii_lowercase();
    matches!(
        lower_ext.as_str(),
        "tsx"
            | "jsx"
            | "mjs"
            | "cjs"
            | "vue"
            | "svelte"
            | "astro"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "swift"
            | "rb"
            | "php"
            | "c"
            | "h"
            | "hpp"
            | "prisma"
            | "graphql"
            | "gql"
            | "proto"
            | "tf"
            | "hcl"
            | "sh"
            | "scss"
            | "sass"
            | "less"
            | "env"
            | "sql"
            | "sqlite"
            | "sqlite3"
            | "db"
            | "duckdb"
            | "rs"
            | "ts"
            | "js"
            | "py"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "md"
            | "cpp"
            | "cs"
            | "html"
            | "css"
            | "xml"
            | "ini"
            | "cfg"
    )
}

pub fn category_for_file(
    ext: &str,
    full_path: &str,
    bytes: &[u8],
    pe: Option<&PeIntelligence>,
) -> (&'static str, String) {
    let lower_ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let lower_path = full_path.to_ascii_lowercase();
    let fname = file_name_lower(full_path);

    if is_env_filename(&fname) {
        return ("database_code", "Ortam Değişkenleri (.env)".into());
    }
    if matches!(
        fname.as_str(),
        ".gitignore"
            | ".dockerignore"
            | ".npmrc"
            | "dockerfile"
            | "docker-compose.yml"
            | "docker-compose.yaml"
            | "makefile"
    ) {
        return ("database_code", format!("Proje Yapılandırması ({fname})"));
    }

    if let Some(p) = pe {
        let sub = pe_subtype_label(p);
        if p.is_dll
            || p.subsystem == 1
            || matches!(lower_ext.as_str(), "dll" | "sys" | "ocx" | "drv")
        {
            return ("system_lib", sub);
        }
        if is_game_path(&lower_path)
            || p.installer_kind
                .as_deref()
                .is_some_and(|k| k.contains("Oyun"))
        {
            return ("game_portable", sub);
        }
        return ("exe_setup", sub);
    }

    match lower_ext.as_str() {
        "exe" if is_game_path(&lower_path) => ("game_portable", "Oyun Çalıştırıcısı (.exe)".into()),
        "exe" => ("exe_setup", "Windows Uygulaması (.exe)".into()),
        "msi" | "msix" | "appx" | "apk" => ("exe_setup", format!("Kurulum Paketi (.{lower_ext})")),
        "pak" if is_game_path(&lower_path) => ("game_portable", "Oyun Veri Paketi (.pak)".into()),
        "pdf" => ("document", "PDF Belgesi".into()),
        "docx" | "doc" | "odt" | "rtf" => ("document", format!("Metin Belgesi (.{lower_ext})")),
        "xlsx" | "xls" | "ods" | "csv" => ("document", format!("Tablo Belgesi (.{lower_ext})")),
        "pptx" | "ppt" => ("document", format!("Sunum Belgesi (.{lower_ext})")),
        "txt" | "md" => ("document", format!("Metin Dosyası (.{lower_ext})")),
        "jpg" | "jpeg" | "png" | "webp" | "heic" | "bmp" | "gif" | "tif" | "tiff" | "psd"
        | "svg" | "ico" | "raw" | "cr2" | "nef" => {
            ("photo", format!("Görsel / Fotoğraf (.{lower_ext})"))
        }
        "mp4" | "mkv" | "mov" | "m4v" | "avi" | "webm" | "wmv" | "flv" => {
            ("video_audio", format!("Video Dosyası (.{lower_ext})"))
        }
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => {
            ("video_audio", format!("Ses Dosyası (.{lower_ext})"))
        }
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "cab" | "iso" => {
            ("archive", format!("Sıkıştırılmış Arşiv (.{lower_ext})"))
        }
        "sqlite" | "sqlite3" | "db" | "mdb" | "sql" | "bak" | "duckdb" => {
            ("database_code", format!("Veritabanı (.{lower_ext})"))
        }
        "json" | "xml" | "yaml" | "yml" | "toml" | "ini" | "cfg" | "rs" | "ts" | "tsx" | "js"
        | "jsx" | "mjs" | "cjs" | "vue" | "svelte" | "astro" | "py" | "go" | "java" | "kt"
        | "kts" | "swift" | "rb" | "php" | "c" | "h" | "hpp" | "cs" | "cpp" | "html" | "css"
        | "scss" | "sass" | "less" | "prisma" | "graphql" | "gql" | "proto" | "tf" | "hcl"
        | "sh" | "env" => (
            "database_code",
            format!("Kod / Yapılandırma (.{lower_ext})"),
        ),
        "dll" | "sys" | "ocx" | "drv" | "lnk" | "bat" | "cmd" | "ps1" => {
            ("system_lib", format!("Sistem / Kısayol (.{lower_ext})"))
        }
        _ if bytes.starts_with(b"%PDF-") => ("document", "PDF Belgesi".into()),
        _ if bytes.starts_with(b"SQLite format 3\0") => {
            ("database_code", "SQLite 3 Veritabanı".into())
        }
        _ => ("other", "Diğer Kullanıcı Dosyası".into()),
    }
}

pub fn category_for_extension(ext: &str, bytes: &[u8]) -> &'static str {
    let pe = analyze_pe_binary(bytes);
    category_for_file(ext, "", bytes, pe.as_ref()).0
}

fn find_marker_size(slice: &[u8], marker: &[u8], tail: usize, min_off: usize) -> Option<u64> {
    if slice.len() <= min_off {
        return None;
    }
    let idx = slice[min_off..]
        .windows(marker.len())
        .position(|w| w == marker)?;
    Some((min_off + idx + tail) as u64)
}

/// Structurally verifies headers in free sectors, rejecting false positives and computing real sizes
/// using `media_repair` boundary walkers for JPEG/PNG/RIFF/MP4.
pub fn detect_carved_signature(slice: &[u8]) -> Option<CarvedSignature> {
    if slice.len() < 64 {
        return None;
    }

    // 1. PE Executables (.exe / .dll)
    if slice.starts_with(b"MZ") {
        let pe = analyze_pe_binary(slice)?;
        let (cat, sub) =
            category_for_file(if pe.is_dll { "dll" } else { "exe" }, "", slice, Some(&pe));
        let suggested = pe
            .original_filename
            .clone()
            .filter(|n| {
                n.to_ascii_lowercase().ends_with(".exe") || n.to_ascii_lowercase().ends_with(".dll")
            })
            .or_else(|| {
                pe.product_name.as_ref().map(|p| {
                    let clean = sanitize_filename(p);
                    if clean.len() >= 2 {
                        format!("{clean}.{}", if pe.is_dll { "dll" } else { "exe" })
                    } else {
                        String::new()
                    }
                })
            })
            .filter(|s| !s.is_empty());
        return Some(CarvedSignature {
            ext: if pe.is_dll { "dll" } else { "exe" },
            category: cat,
            sub_type: sub,
            estimated_size: pe.estimated_size,
            product_name: pe.product_name,
            publisher: pe.company_name,
            suggested_name: suggested,
            rsrc_offset: pe.rsrc_offset,
        });
    }

    // 2. OLE2 Compound Binary (.msi installers vs .doc/.xls/.ppt)
    if slice.starts_with(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") && slice.len() >= 512 {
        let win = &slice[..slice.len().min(32768)];
        let is_msi = find_bytes(win, b"Windows Installer").is_some()
            || extract_utf16_version_field(win, "ProductName").is_some();
        return Some(if is_msi {
            CarvedSignature {
                ext: "msi",
                category: "exe_setup",
                sub_type: "Windows MSI Kurulum Paketi".into(),
                estimated_size: 4 * 1024 * 1024,
                product_name: None,
                publisher: None,
                suggested_name: None,
                rsrc_offset: None,
            }
        } else {
            CarvedSignature {
                ext: "doc",
                category: "document",
                sub_type: "OLE2 Office Belgesi".into(),
                estimated_size: 512 * 1024,
                product_name: None,
                publisher: None,
                suggested_name: None,
                rsrc_offset: None,
            }
        });
    }

    // 3. PKZIP / OOXML (.docx, .xlsx, .pptx, .apk, .zip)
    if slice.starts_with(b"PK\x03\x04") {
        let name_len = u16::from_le_bytes([slice[26], slice[27]]) as usize;
        if name_len == 0 || name_len > 220 || 30 + name_len > slice.len() {
            return None;
        }
        let first_entry = std::str::from_utf8(&slice[30..30 + name_len]).ok()?;
        let lower_entry = first_entry.to_ascii_lowercase();
        let eocd_sz = find_marker_size(slice, b"PK\x05\x06", 22, 30).map(|s| s.max(4096));
        let inspect = String::from_utf8_lossy(&slice[..slice.len().min(8192)]).to_ascii_lowercase();
        let (ext, cat, sub, default_sz) = if inspect.contains("word/") {
            ("docx", "document", "Word Belgesi (.docx)", 256 * 1024)
        } else if inspect.contains("xl/") {
            ("xlsx", "document", "Excel Tablosu (.xlsx)", 256 * 1024)
        } else if inspect.contains("ppt/") {
            ("pptx", "document", "PowerPoint Sunumu (.pptx)", 512 * 1024)
        } else if inspect.contains("androidmanifest.xml") {
            ("apk", "exe_setup", "Android Paketi (.apk)", 4 * 1024 * 1024)
        } else if lower_entry.ends_with('/') || lower_entry.contains('.') || name_len >= 3 {
            ("zip", "archive", "ZIP Arşivi", 1024 * 1024)
        } else {
            return None;
        };
        return Some(CarvedSignature {
            ext,
            category: cat,
            sub_type: sub.into(),
            estimated_size: eocd_sz.unwrap_or(default_sz),
            product_name: None,
            publisher: None,
            suggested_name: None,
            rsrc_offset: None,
        });
    }

    // 4. 7-Zip Archive (exact header NextHeaderOffset + NextHeaderSize + 32)
    if slice.starts_with(b"7z\xBC\xAF\x27\x1C") {
        let next_off = u64::from_le_bytes(slice[12..20].try_into().ok()?);
        let next_sz = u64::from_le_bytes(slice[20..28].try_into().ok()?);
        let total = 32u64.saturating_add(next_off).saturating_add(next_sz);
        if (64..=MAX_RESTORE_BYTES).contains(&total) {
            return Some(CarvedSignature {
                ext: "7z",
                category: "archive",
                sub_type: "7-Zip Arşivi".into(),
                estimated_size: total,
                product_name: None,
                publisher: None,
                suggested_name: None,
                rsrc_offset: None,
            });
        }
    }

    // 5. RAR5 / RAR4
    if slice.starts_with(b"Rar!\x1A\x07\x00") || slice.starts_with(b"Rar!\x1A\x07\x01\x00") {
        return Some(CarvedSignature {
            ext: "rar",
            category: "archive",
            sub_type: "RAR Arşivi".into(),
            estimated_size: 2 * 1024 * 1024,
            product_name: None,
            publisher: None,
            suggested_name: None,
            rsrc_offset: None,
        });
    }

    // 6. PDF
    if slice.starts_with(b"%PDF-1.") || slice.starts_with(b"%PDF-2.") {
        let sz = find_marker_size(slice, b"%%EOF", 5, 16)
            .map(|s| s.max(4096))
            .unwrap_or(512 * 1024);
        return Some(CarvedSignature {
            ext: "pdf",
            category: "document",
            sub_type: "PDF Belgesi".into(),
            estimated_size: sz,
            product_name: None,
            publisher: None,
            suggested_name: None,
            rsrc_offset: None,
        });
    }

    // 7. PNG (exact chunk-walking boundary via media_repair::find_png_end)
    if slice.starts_with(b"\x89PNG\r\n\x1A\n") && &slice[12..16] == b"IHDR" {
        let w = u32::from_be_bytes(slice[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(slice[20..24].try_into().ok()?);
        if (1..=32768).contains(&w) && (1..=32768).contains(&h) {
            let sz = media_repair::find_png_end(slice)
                .map(|n| (n as u64).max(4096))
                .or_else(|| {
                    find_marker_size(slice, b"IEND\xAE\x42\x60\x82", 8, 24).map(|s| s.max(4096))
                })
                .unwrap_or(512 * 1024);
            return Some(CarvedSignature {
                ext: "png",
                category: "photo",
                sub_type: format!("PNG Görsel ({w}×{h})"),
                estimated_size: sz,
                product_name: None,
                publisher: None,
                suggested_name: None,
                rsrc_offset: None,
            });
        }
    }

    // 8. JPEG (proper segment-walking boundary via media_repair::find_jpeg_end — never cuts at EXIF FF D9!)
    if slice.starts_with(b"\xFF\xD8\xFF") && matches!(slice[3], 0xE0..=0xEF | 0xDB | 0xC0 | 0xC2) {
        let sz = media_repair::find_jpeg_end(slice)
            .map(|n| (n as u64).clamp(4096, MAX_RESTORE_BYTES))
            .unwrap_or(1024 * 1024);
        return Some(CarvedSignature {
            ext: "jpg",
            category: "photo",
            sub_type: "JPEG Fotoğraf".into(),
            estimated_size: sz,
            product_name: None,
            publisher: None,
            suggested_name: None,
            rsrc_offset: None,
        });
    }

    // 9. GIF87a / GIF89a
    if slice.starts_with(b"GIF87a") || slice.starts_with(b"GIF89a") {
        let w = u16::from_le_bytes([slice[6], slice[7]]);
        let h = u16::from_le_bytes([slice[8], slice[9]]);
        if (1..=16384).contains(&w) && (1..=16384).contains(&h) {
            let sz = slice[13..]
                .iter()
                .rposition(|&b| b == 0x3B)
                .map(|pos| ((13 + pos + 1) as u64).max(4096))
                .unwrap_or(256 * 1024);
            return Some(CarvedSignature {
                ext: "gif",
                category: "photo",
                sub_type: format!("GIF Görsel ({w}×{h})"),
                estimated_size: sz,
                product_name: None,
                publisher: None,
                suggested_name: None,
                rsrc_offset: None,
            });
        }
    }

    // 10. BMP (valid BITMAPFILEHEADER + BITMAPINFOHEADER)
    if slice.starts_with(b"BM") && slice[6..10] == [0, 0, 0, 0] {
        let bf_size = u64::from(u32::from_le_bytes(slice[2..6].try_into().ok()?));
        let data_off = u32::from_le_bytes(slice[10..14].try_into().ok()?);
        let dib_sz = u32::from_le_bytes(slice[14..18].try_into().ok()?);
        if (54..=64 * 1024 * 1024).contains(&bf_size)
            && (26..=4096).contains(&data_off)
            && matches!(dib_sz, 12 | 40 | 52 | 56 | 108 | 124)
        {
            let w = i32::from_le_bytes(slice[18..22].try_into().ok()?).unsigned_abs();
            let h = i32::from_le_bytes(slice[22..26].try_into().ok()?).unsigned_abs();
            if (1..=32768).contains(&w) && (1..=32768).contains(&h) {
                return Some(CarvedSignature {
                    ext: "bmp",
                    category: "photo",
                    sub_type: format!("BMP Görsel ({w}×{h})"),
                    estimated_size: bf_size.max(4096),
                    product_name: None,
                    publisher: None,
                    suggested_name: None,
                    rsrc_offset: None,
                });
            }
        }
    }

    // 11. RIFF (WEBP / AVI / WAV exact chunk size via media_repair::find_riff_size)
    if slice.starts_with(b"RIFF") && slice.len() >= 16 {
        let riff_sz = media_repair::find_riff_size(slice)
            .map(|n| n as u64)
            .unwrap_or_else(|| {
                u64::from(u32::from_le_bytes(slice[4..8].try_into().unwrap_or([0; 4])))
                    .saturating_add(8)
            });
        if (4096..=MAX_RESTORE_BYTES).contains(&riff_sz) {
            match &slice[8..12] {
                b"WEBP" => {
                    return Some(CarvedSignature {
                        ext: "webp",
                        category: "photo",
                        sub_type: "WebP Görsel".into(),
                        estimated_size: riff_sz,
                        product_name: None,
                        publisher: None,
                        suggested_name: None,
                        rsrc_offset: None,
                    });
                }
                b"AVI " => {
                    return Some(CarvedSignature {
                        ext: "avi",
                        category: "video_audio",
                        sub_type: "AVI Video".into(),
                        estimated_size: riff_sz,
                        product_name: None,
                        publisher: None,
                        suggested_name: None,
                        rsrc_offset: None,
                    });
                }
                b"WAVE" => {
                    return Some(CarvedSignature {
                        ext: "wav",
                        category: "video_audio",
                        sub_type: "WAV Ses Kaydı".into(),
                        estimated_size: riff_sz,
                        product_name: None,
                        publisher: None,
                        suggested_name: None,
                        rsrc_offset: None,
                    });
                }
                _ => {}
            }
        }
    }

    // 12. MP4 / MOV (`ftyp`) via media_repair::find_mp4_size box walker
    if &slice[4..8] == b"ftyp" && slice[8..12].iter().all(|b| b.is_ascii_alphanumeric()) {
        let box0 = u32::from_be_bytes(slice[0..4].try_into().ok()?) as usize;
        if (16..=128).contains(&box0) {
            let sz = media_repair::find_mp4_size(slice)
                .map(|n| (n as u64).clamp(4096, MAX_RESTORE_BYTES))
                .unwrap_or(8 * 1024 * 1024);
            let is_mov = &slice[8..12] == b"qt  ";
            return Some(CarvedSignature {
                ext: if is_mov { "mov" } else { "mp4" },
                category: "video_audio",
                sub_type: if is_mov {
                    "QuickTime Video (.mov)".into()
                } else {
                    "MP4 / MOV Video".into()
                },
                estimated_size: sz,
                product_name: None,
                publisher: None,
                suggested_name: None,
                rsrc_offset: None,
            });
        }
    }

    // 13. SQLite 3 (exact page_size * page_count)
    if slice.starts_with(b"SQLite format 3\0") {
        let page_raw = u16::from_be_bytes([slice[16], slice[17]]);
        let page_sz = if page_raw == 1 {
            65536u64
        } else {
            u64::from(page_raw)
        };
        let pages = u64::from(u32::from_be_bytes(slice[28..32].try_into().ok()?));
        let total = page_sz.saturating_mul(pages).clamp(4096, MAX_RESTORE_BYTES);
        return Some(CarvedSignature {
            ext: "sqlite3",
            category: "database_code",
            sub_type: "SQLite 3 Veritabanı".into(),
            estimated_size: total,
            product_name: None,
            publisher: None,
            suggested_name: None,
            rsrc_offset: None,
        });
    }

    None
}

fn header_matches_ext(ext: &str, bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let lower = ext.trim_start_matches('.').to_ascii_lowercase();
    match lower.as_str() {
        "exe" | "dll" | "sys" => analyze_pe_binary(bytes).is_some(),
        "pdf" => bytes.starts_with(b"%PDF-"),
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1A\n"),
        "jpg" | "jpeg" => bytes.starts_with(b"\xFF\xD8\xFF"),
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "bmp" => bytes.starts_with(b"BM"),
        "webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        "avi" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"AVI ",
        "mp4" | "mov" | "m4v" => bytes.len() >= 8 && &bytes[4..8] == b"ftyp",
        "zip" | "docx" | "xlsx" | "pptx" | "apk" => bytes.starts_with(b"PK\x03\x04"),
        "msi" | "doc" | "xls" | "ppt" => bytes.starts_with(b"\xD0\xCF\x11\xE0"),
        "7z" => bytes.starts_with(b"7z\xBC\xAF\x27\x1C"),
        "rar" => bytes.starts_with(b"Rar!\x1A\x07"),
        "sqlite" | "sqlite3" | "db" => bytes.starts_with(b"SQLite format 3\0"),
        _ => true,
    }
}

pub fn classify_buffer_ai(
    bytes: &[u8],
    ext_hint: &str,
    source: &str,
    cluster_free: bool,
    is_ssd: bool,
) -> (String, u32, bool, String) {
    let pe = analyze_pe_binary(bytes);
    let (cat, _sub) = category_for_file(ext_hint, "", bytes, pe.as_ref());
    if !bytes.is_empty() && (bytes.iter().all(|&b| b == 0) || bytes.iter().all(|&b| b == 0xFF)) {
        let msg = if is_ssd {
            "Sektörler SSD TRIM tarafından sıfırlandı; fiziksel içerik kalıcı olarak silinmiş."
        } else {
            "Sektör içeriği tamamen sıfır (üzerine yazılmış veya boş alan)."
        };
        return (cat.to_owned(), 0, true, msg.to_owned());
    }

    let ent = shannon_entropy(bytes);
    let hdr_ok = header_matches_ext(ext_hint, bytes);
    let mut conf: i32 = match source {
        "recycle_bin" => 100,
        "mft_resident" => 99,
        "mft_nonresident" => {
            if cluster_free {
                88
            } else {
                34
            }
        }
        "carved" => {
            if hdr_ok {
                84
            } else {
                45
            }
        }
        _ => 60,
    };
    if is_ssd && source != "recycle_bin" && source != "mft_resident" {
        conf -= 18;
    }
    if !hdr_ok && !bytes.is_empty() {
        conf = (conf - 30).max(12);
    }
    let confidence = conf.clamp(5, 100) as u32;
    let summary = match source {
        "recycle_bin" => {
            "Geri Dönüşüm Kutusu içinde eksiksiz ($I/$R); %100 bütünlükle geri alınabilir.".into()
        }
        "mft_resident" => {
            "Küçük dosya doğrudan $MFT kaydı içinde tam korunmuş; küme ezilmesi riski yok.".into()
        }
        "carved" => format!(
            "Sektör yapısı ve imzası doğrulandı (Entropi: {ent:.2} bit/B, Bütünlük: %{confidence})."
        ),
        _ if !cluster_free => format!(
            "Veri kümesinin bir kısmı yeni dosyalarca üzerine yazılmış (Entropi: {ent:.2} bit/B)."
        ),
        _ => format!(
            "Kümeler henüz boş alanda korunuyor (Entropi: {ent:.2} bit/B); farklı sürücüye kurtarın."
        ),
    };
    (cat.to_owned(), confidence, false, summary)
}

type MediaInspectTuple = (Option<String>, bool, Option<String>, Option<(u32, u32)>);

fn inspect_media_bytes(
    ext: &str,
    bytes: &[u8],
    total_size: u64,
    trusted_intact_source: bool,
) -> MediaInspectTuple {
    if !media_repair::is_supported_media_ext(ext) {
        return (None, true, None, None);
    }
    if bytes.is_empty() || bytes.iter().all(|&b| b == 0) || bytes.iter().all(|&b| b == 0xFF) {
        return (
            Some("unrecoverable".into()),
            false,
            Some("Dosya verisi boş veya TRIM ile sıfırlanmış; açılamaz.".into()),
            None,
        );
    }
    let rep = media_repair::inspect_media(ext, bytes);
    let dims = rep.width.zip(rep.height);
    // Large OS-preserved files (e.g. >8 MiB MP4 in Recycle Bin or VSS) whose tail `moov`
    // sits past our head inspection buffer are still intact when container magic is valid.
    if trusted_intact_source
        && total_size > bytes.len() as u64
        && !rep.openable
        && (header_matches_ext(ext, bytes)
            || bytes.starts_with(b"\x1A\x45\xDF\xA3")
            || bytes.starts_with(b"RIFF"))
    {
        return (
            Some("intact".into()),
            true,
            Some("Geri Dönüşüm / VSS kaynağında eksiksiz korunmuş medya dosyası.".into()),
            dims,
        );
    }
    let note = (!rep.note_tr.is_empty()).then_some(rep.note_tr);
    (Some(rep.status), rep.openable, note, dims)
}

fn sanitize_filename(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.' || *c == ' ')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
        .trim_matches('.')
        .chars()
        .take(48)
        .collect()
}

fn sanitize_dest_filename(raw: &str) -> String {
    let base = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("kurtarilan_dosya.bin");
    let cleaned: String = base
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "kurtarilan_dosya.bin".into()
    } else {
        trimmed.chars().take(120).collect()
    }
}

fn unique_dest_path(dest_dir: &Path, filename: &str) -> PathBuf {
    let initial = dest_dir.join(filename);
    if !initial.exists() {
        return initial;
    }
    let p = Path::new(filename);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("kurtarilan");
    let ext = p.extension().and_then(|s| s.to_str());
    for n in 2..=9999u32 {
        let candidate = match ext {
            Some(e) if !e.is_empty() => dest_dir.join(format!("{stem}_{n}.{e}")),
            _ => dest_dir.join(format!("{stem}_{n}")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    dest_dir.join(format!("{}_{}", uuid::Uuid::new_v4().simple(), filename))
}

pub fn semantic_name_from_bytes(bytes: &[u8], ext: &str, index: usize) -> String {
    if let Some(pe) = analyze_pe_binary(bytes) {
        if let Some(orig) = pe.original_filename {
            let clean = sanitize_filename(&orig);
            if clean.len() >= 4 {
                if clean.to_ascii_lowercase().ends_with(&format!(".{ext}")) {
                    return clean;
                }
                return format!("{clean}_{index:03}.{ext}");
            }
        }
        if let Some(prod) = pe.product_name {
            let clean = sanitize_filename(&prod);
            if clean.len() >= 3 {
                return format!("{clean}_{index:03}.{ext}");
            }
        }
    }
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(8192)]);
    if let Some(pos) = text.find("/Title (") {
        if let Some(end) = text[pos + 8..].find(')') {
            let clean = sanitize_filename(&text[pos + 8..pos + 8 + end]);
            if clean.len() >= 3 {
                return format!("{clean}_{index:03}.{ext}");
            }
        }
    }
    let prefix = match category_for_extension(ext, bytes) {
        "exe_setup" | "game_portable" => "Kurtarilan_Uygulama",
        "document" => "Kurtarilan_Belge",
        "photo" => "Kurtarilan_Foto",
        "video_audio" => "Kurtarilan_Medya",
        "archive" => "Kurtarilan_Arsiv",
        "database_code" => "Kurtarilan_Veritabani",
        "system_lib" => "Kurtarilan_Sistem",
        _ => "Kurtarilan_Dosya",
    };
    format!("{prefix}_{index:03}.{ext}")
}

fn extension_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn drive_letter_of(path: &Path) -> Option<char> {
    let s = path.to_string_lossy();
    let trimmed = s.strip_prefix("\\\\?\\").unwrap_or(&s);
    let mut chars = trimmed.chars();
    let c0 = chars.next()?;
    let c1 = chars.next()?;
    (c0.is_ascii_alphabetic() && c1 == ':').then(|| c0.to_ascii_uppercase())
}

fn read_item_bytes_readonly(
    source_letter: &str,
    item: &Value,
    max_bytes: usize,
) -> Result<(Vec<u8>, u64), String> {
    let source = item["source"].as_str().unwrap_or("");
    let reported_size = item["size_bytes"].as_u64().unwrap_or(0);
    match source {
        "recycle_bin" | "vss_shadow" | "installer_cache" => {
            let src_str = item["locator"]["r_path"]
                .as_str()
                .or_else(|| item["locator"]["src_path"].as_str())
                .ok_or("Kaynak dosya yolu eksik.")?;
            let src_file = PathBuf::from(src_str);
            let is_vss = source == "vss_shadow";
            if !src_file.exists() || (!is_vss && platform::reparse(&src_file)) {
                return Err("Kaynak dosya artık disk üzerinde erişilebilir değil.".into());
            }
            let file_len = std::fs::metadata(&src_file)
                .map(|m| m.len())
                .unwrap_or(reported_size);
            let f = File::open(&src_file).map_err(|e| e.to_string())?;
            let mut buf = Vec::with_capacity((file_len as usize).min(max_bytes));
            f.take(max_bytes as u64)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            Ok((buf, file_len))
        }
        "mft_resident" => {
            let hex = item["locator"]["resident_hex"]
                .as_str()
                .ok_or("MFT resident verisi bulunamadı.")?;
            let mut data = decode_hex(hex)?;
            let total = data.len() as u64;
            if data.len() > max_bytes {
                data.truncate(max_bytes);
            }
            Ok((data, total))
        }
        "mft_nonresident" | "carved" => {
            let src_char = source_letter
                .chars()
                .next()
                .map(|c| c.to_ascii_uppercase())
                .ok_or("Kaynak bölüm harfi geçersiz.")?;
            #[cfg(windows)]
            {
                let data = win::read_raw_bytes_from_volume(src_char, item, max_bytes)?;
                let total = reported_size.max(data.len() as u64);
                Ok((data, total))
            }
            #[cfg(not(windows))]
            {
                let _ = src_char;
                Err("Ham sektör önizlemesi yalnız Windows'ta desteklenir.".into())
            }
        }
        _ => Err("Bu kurtarma kaynağı için doğrudan okuma desteklenmiyor.".into()),
    }
}

fn sniff_image_mime(bytes: &[u8], ext: &str) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1A\n") {
        "image/png"
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        match ext {
            "png" => "image/png",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            "webp" => "image/webp",
            _ => "image/jpeg",
        }
    }
}

fn is_previewable_text_ext(ext: &str) -> bool {
    matches!(
        ext,
        "txt"
            | "md"
            | "log"
            | "json"
            | "xml"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "conf"
            | "csv"
            | "tsv"
            | "sql"
            | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "vue"
            | "svelte"
            | "astro"
            | "py"
            | "cs"
            | "cpp"
            | "c"
            | "h"
            | "hpp"
            | "java"
            | "kt"
            | "kts"
            | "go"
            | "rb"
            | "php"
            | "swift"
            | "prisma"
            | "graphql"
            | "gql"
            | "proto"
            | "tf"
            | "hcl"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "bat"
            | "cmd"
            | "ps1"
            | "sh"
            | "env"
            | "svg"
            | "rtf"
    )
}

fn is_previewable_dev_name(path_or_name: &str) -> bool {
    let fname = file_name_lower(path_or_name);
    is_env_filename(&fname)
        || matches!(
            fname.as_str(),
            ".gitignore"
                | ".dockerignore"
                | ".npmrc"
                | "dockerfile"
                | "docker-compose.yml"
                | "docker-compose.yaml"
                | "makefile"
        )
}

fn is_benign_env_key(key: &str) -> bool {
    matches!(
        key,
        "NODE_ENV"
            | "PORT"
            | "HOST"
            | "DEBUG"
            | "TZ"
            | "LANG"
            | "LOG_LEVEL"
            | "ENVIRONMENT"
            | "APP_ENV"
    )
}

fn is_sensitive_key_name(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "secret",
        "token",
        "password",
        "passwd",
        "private_key",
        "database_url",
        "connection_string",
        "client_secret",
        "access_key",
        "auth_token",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Masks secret values inside read-only text previews without altering restored file bytes.
pub fn mask_sensitive_lines(text: &str, is_env_file: bool) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut masked = 0usize;
    let mut in_pem = false;

    for chunk in text.split_inclusive('\n') {
        let (line, nl) = if let Some(stripped) = chunk.strip_suffix("\r\n") {
            (stripped, "\r\n")
        } else if let Some(stripped) = chunk.strip_suffix('\n') {
            (stripped, "\n")
        } else {
            (chunk, "")
        };
        let trimmed = line.trim();
        let upper = trimmed.to_ascii_uppercase();

        if upper.contains("-----BEGIN") && upper.contains("PRIVATE KEY-----") {
            in_pem = true;
            out.push_str(line);
            out.push_str(nl);
            continue;
        }
        if in_pem {
            if upper.contains("-----END") && upper.contains("PRIVATE KEY-----") {
                in_pem = false;
                out.push_str(line);
            } else {
                masked += 1;
                out.push_str("••••••••");
            }
            out.push_str(nl);
            continue;
        }

        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            out.push_str(line);
            out.push_str(nl);
            continue;
        }

        let sep_idx = if is_env_file {
            line.find('=')
        } else {
            line.find(['=', ':'])
        };

        let mut replaced = false;
        if let Some(idx) = sep_idx {
            let (lhs, rhs_with_sep) = line.split_at(idx);
            let sep = &rhs_with_sep[..1];
            let rhs = &rhs_with_sep[1..];
            let key_raw = lhs.trim();
            let key_clean = key_raw
                .strip_prefix("export ")
                .unwrap_or(key_raw)
                .trim()
                .trim_matches(['"', '\''])
                .trim();
            let val_trim = rhs.trim();
            let has_non_empty_val = !val_trim.is_empty()
                && !matches!(val_trim, "{" | "[" | "null" | "\"\"" | "''" | "|");
            let should_mask = if !key_clean.is_empty() && has_non_empty_val {
                if is_env_file {
                    !is_benign_env_key(&key_clean.to_ascii_uppercase())
                } else {
                    is_sensitive_key_name(key_clean)
                }
            } else {
                false
            };
            if should_mask {
                let trailing_comma = if rhs.trim_end().ends_with(',') {
                    ","
                } else {
                    ""
                };
                let space_after = if sep == ":" && rhs.starts_with(' ') {
                    " "
                } else {
                    ""
                };
                out.push_str(lhs);
                out.push_str(sep);
                out.push_str(space_after);
                out.push_str("••••••••");
                out.push_str(trailing_comma);
                masked += 1;
                replaced = true;
            }
        }
        if !replaced {
            out.push_str(line);
        }
        out.push_str(nl);
    }

    (out, masked)
}

fn decode_utf8_preview(bytes: &[u8], max_bytes: usize) -> Option<String> {
    let slice = if bytes.len() > max_bytes {
        &bytes[..max_bytes]
    } else {
        bytes
    };
    if slice.contains(&0) {
        return None;
    }
    let text = match std::str::from_utf8(slice) {
        Ok(s) => s,
        Err(e) => {
            let valid_up_to = e.valid_up_to();
            if valid_up_to == 0 || (e.error_len().is_some() && valid_up_to + 4 < slice.len()) {
                return None;
            }
            std::str::from_utf8(&slice[..valid_up_to]).ok()?
        }
    };
    let bad_ctrl = text
        .chars()
        .filter(|&c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        .count();
    if !text.is_empty() && bad_ctrl * 20 > text.chars().count().max(1) {
        return None;
    }
    Some(text.strip_prefix('\u{FEFF}').unwrap_or(text).to_owned())
}

/// Read-only item preview (zero disk writes).
/// Supports images (<= 4 MiB, with automatic `media_repair::repair_media`!), UTF-8 text (<= 16 KiB),
/// PE/EXE/MSI binary metadata, and graceful `unavailable` responses.
pub fn preview_item(job: &Value, item_id: &str) -> Result<Value, String> {
    let item = job["recovery_items"]
        .as_array()
        .and_then(|arr| arr.iter().find(|i| i["id"].as_str() == Some(item_id)))
        .ok_or_else(|| "Önizlenecek kurtarma kaydı bulunamadı.".to_owned())?;

    let source_letter = item["source_letter"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| job["letter"].as_str())
        .unwrap_or("");
    let source = item["source"].as_str().unwrap_or("");
    let ext = item["extension"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_start_matches('.').to_ascii_lowercase())
        .unwrap_or_else(|| extension_of(item["name"].as_str().unwrap_or("")));
    let reported_size = item["size_bytes"].as_u64().unwrap_or(0);

    if item["trim_zeroed"] == true {
        return Ok(json!({
            "kind": "unavailable",
            "reason": "Bu dosyanın sektörleri SSD TRIM tarafından sıfırlandığı için önizleme yapılamaz.",
            "repaired": false,
            "repair_note": null,
            "media_status": "unrecoverable"
        }));
    }
    if matches!(source, "usn_journal" | "indx_slack") || item["locator"]["metadata_only"] == true {
        return Ok(json!({
            "kind": "unavailable",
            "reason": "Bu kayıt yalnızca NTFS işlem/dizin günlüğü izidir; içerik önizlemesi için ham veri mevcut değil."
        }));
    }

    // 1. PE / EXE / MSI / DLL binary metadata preview
    if matches!(
        ext.as_str(),
        "exe" | "msi" | "msix" | "dll" | "sys" | "ocx" | "drv"
    ) || matches!(
        item["category"].as_str(),
        Some("exe_setup" | "game_portable" | "system_lib")
    ) {
        let mut product_name = item["product_name"].as_str().map(str::to_owned);
        let mut publisher = item["publisher"].as_str().map(str::to_owned);
        let mut sub_type = item["sub_type"].as_str().unwrap_or("").to_owned();
        if (product_name.is_none() || publisher.is_none()) && ext == "exe" {
            if let Ok((hdr, _)) = read_item_bytes_readonly(source_letter, item, 64 * 1024) {
                if let Some(pe) = analyze_pe_binary(&hdr) {
                    if product_name.is_none() {
                        product_name = pe.product_name.clone();
                    }
                    if publisher.is_none() {
                        publisher = pe.company_name.clone();
                    }
                    if sub_type.is_empty() {
                        sub_type = pe_subtype_label(&pe);
                    }
                }
            }
        }
        return Ok(json!({
            "kind": "binary",
            "metadata": {
                "product_name": product_name,
                "publisher": publisher,
                "sub_type": sub_type,
                "size_bytes": reported_size,
                "source": source,
                "status_label": item["status_label"].as_str().unwrap_or(""),
            }
        }));
    }

    // 2. Image preview (PNG / JPG / GIF / BMP / WEBP <= 4 MiB) with automatic media repair!
    if matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
    ) {
        if reported_size > MAX_PREVIEW_IMAGE_BYTES as u64 {
            return Ok(json!({
                "kind": "unavailable",
                "reason": "Görsel boyutu 4 MiB önizleme sınırını aşıyor; kurtararak görüntüleyebilirsiniz."
            }));
        }
        let (raw, actual_size) =
            match read_item_bytes_readonly(source_letter, item, MAX_PREVIEW_IMAGE_BYTES + 1) {
                Ok(pair) => pair,
                Err(reason) => {
                    return Ok(json!({
                        "kind": "unavailable",
                        "reason": reason
                    }));
                }
            };
        if raw.len() > MAX_PREVIEW_IMAGE_BYTES {
            return Ok(json!({
                "kind": "unavailable",
                "reason": "Görsel boyutu 4 MiB önizleme sınırını aşıyor; kurtararak görüntüleyebilirsiniz."
            }));
        }
        if raw.is_empty() || raw.iter().all(|&b| b == 0) || raw.iter().all(|&b| b == 0xFF) {
            return Ok(json!({
                "kind": "unavailable",
                "reason": "Görsel sektörü boş veya TRIM ile sıfırlanmış.",
                "repaired": false,
                "repair_note": "Görsel verisi kurtarılamayacak derecede boş.",
                "media_status": "unrecoverable"
            }));
        }
        let (repaired_bytes, report) = media_repair::repair_media(&ext, &raw);
        if !report.openable || repaired_bytes.is_empty() {
            return Ok(json!({
                "kind": "unavailable",
                "reason": if report.note_tr.is_empty() {
                    "Görsel başlığı veya veri akışı onarılamayacak kadar bozuk.".to_owned()
                } else {
                    report.note_tr.clone()
                },
                "repaired": false,
                "repair_note": (!report.note_tr.is_empty()).then_some(report.note_tr),
                "media_status": report.status
            }));
        }
        let mime = sniff_image_mime(&repaired_bytes, &ext);
        let data_url = format!("data:{mime};base64,{}", encode_base64(&repaired_bytes));
        let out_size = if report.repaired {
            repaired_bytes.len() as u64
        } else {
            actual_size.max(repaired_bytes.len() as u64)
        };
        return Ok(json!({
            "kind": "image",
            "mime": mime,
            "data_url": data_url,
            "size_bytes": out_size,
            "repaired": report.repaired,
            "repair_note": (!report.note_tr.is_empty()).then_some(report.note_tr),
            "media_status": report.status,
            "width": report.width,
            "height": report.height
        }));
    }

    // 3. Video status preview (diagnose playability & auto-repair capability read-only)
    if matches!(ext.as_str(), "mp4" | "mov" | "m4v" | "avi" | "mkv" | "webm") {
        let mut reason = item["repair_note"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let mut media_status = item["media_status"].as_str().map(str::to_owned);
        let mut repaired = media_status.as_deref() == Some("repaired");
        if reason.is_none() {
            if let Ok((raw, total)) =
                read_item_bytes_readonly(source_letter, item, MAX_PREVIEW_IMAGE_BYTES)
            {
                let trusted = matches!(source, "recycle_bin" | "vss_shadow");
                let (st, openable, note, _) = inspect_media_bytes(&ext, &raw, total, trusted);
                repaired = st.as_deref() == Some("repaired");
                media_status = st;
                reason = note.or_else(|| {
                    Some(if openable {
                        "Video kurtarıldığında otomatik onarım kontrolünden geçirilerek kaydedilecek."
                            .into()
                    } else {
                        "Video akışı açılamayacak derecede hasarlı görünüyor.".into()
                    })
                });
            }
        }
        return Ok(json!({
            "kind": "unavailable",
            "reason": reason.unwrap_or_else(|| "Video dosyaları satır içi oynatılmaz; kurtardığınızda otomatik onarım uygulanır.".into()),
            "repaired": repaired,
            "repair_note": item["repair_note"],
            "media_status": media_status
        }));
    }

    // 4. UTF-8 Text / Code / Config preview (<= 16 KiB)
    let is_known_binary = matches!(
        ext.as_str(),
        "pdf"
            | "zip"
            | "rar"
            | "7z"
            | "tar"
            | "gz"
            | "iso"
            | "docx"
            | "xlsx"
            | "pptx"
            | "doc"
            | "xls"
            | "ppt"
            | "sqlite"
            | "sqlite3"
            | "db"
            | "mdb"
            | "mp3"
            | "wav"
            | "flac"
            | "ogg"
            | "psd"
            | "ico"
            | "pak"
    );
    let item_name = item["name"].as_str().unwrap_or("");
    let orig_path = item["original_path"].as_str().unwrap_or("");
    let dev_text_name = is_previewable_dev_name(item_name) || is_previewable_dev_name(orig_path);
    if is_previewable_text_ext(&ext) || dev_text_name || !is_known_binary {
        match read_item_bytes_readonly(source_letter, item, MAX_PREVIEW_TEXT_BYTES + 1) {
            Ok((raw, actual_size)) => {
                if !raw.is_empty()
                    && !(raw.iter().all(|&b| b == 0) || raw.iter().all(|&b| b == 0xFF))
                {
                    if let Some(text) = decode_utf8_preview(&raw, MAX_PREVIEW_TEXT_BYTES) {
                        let truncated = actual_size > MAX_PREVIEW_TEXT_BYTES as u64
                            || raw.len() > MAX_PREVIEW_TEXT_BYTES;
                        let is_env = is_env_filename(item_name) || is_env_filename(orig_path);
                        let (masked_text, masked_count) = mask_sensitive_lines(&text, is_env);
                        if masked_count > 0 {
                            return Ok(json!({
                                "kind": "text",
                                "text": masked_text,
                                "raw_text": text,
                                "secrets_masked": masked_count,
                                "truncated": truncated,
                                "size_bytes": actual_size.max(raw.len() as u64)
                            }));
                        }
                        return Ok(json!({
                            "kind": "text",
                            "text": text,
                            "truncated": truncated,
                            "size_bytes": actual_size.max(raw.len() as u64)
                        }));
                    }
                }
            }
            Err(reason) => {
                if is_previewable_text_ext(&ext) || dev_text_name {
                    return Ok(json!({
                        "kind": "unavailable",
                        "reason": reason
                    }));
                }
            }
        }
    }

    Ok(json!({
        "kind": "unavailable",
        "reason": if ext.is_empty() {
            "Bu dosya türü için satır içi önizleme desteklenmiyor; güvenli şekilde kurtarıp açabilirsiniz.".to_owned()
        } else {
            format!("Bu dosya türü (.{ext}) için satır içi önizleme desteklenmiyor; güvenli şekilde kurtarıp açabilirsiniz.")
        }
    }))
}

pub fn restore_item(source_letter: &str, item: &Value, dest_path: &Path) -> Result<Value, String> {
    if item["trim_zeroed"] == true {
        return Err(
            "Bu dosyanın sektörleri SSD TRIM tarafından sıfırlandığı için içerik kurtarılamaz."
                .into(),
        );
    }
    let src_char = source_letter
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase())
        .ok_or("Kaynak bölüm harfi geçersiz.")?;
    let source = item["source"].as_str().unwrap_or("");
    let parent = dest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or("Geçersiz hedef klasör.")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let canon_parent = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    let dest_check = canon_parent.join(dest_path.file_name().ok_or("Hedef dosya adı eksik.")?);
    if !matches!(source, "recycle_bin" | "vss_shadow" | "installer_cache")
        && drive_letter_of(&dest_check) == Some(src_char)
    {
        return Err(
            "Silinen sektörlerin üzerine yazılmasını önlemek için kurtarılan dosyayı FARKLI bir diske veya USB belleğe kaydedin."
                .into(),
        );
    }

    let tmp = dest_path.with_extension(format!(
        "{}.sentinel.tmp",
        dest_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("bin")
    ));
    let mut hasher = Sha256::new();
    let mut written_bytes: u64 = match source {
        "recycle_bin" | "vss_shadow" | "installer_cache" => {
            let src_str = item["locator"]["r_path"]
                .as_str()
                .or_else(|| item["locator"]["src_path"].as_str())
                .ok_or("Kaynak kurtarma dosya yolu eksik.")?;
            let src_file = PathBuf::from(src_str);
            let is_vss = source == "vss_shadow";
            if !src_file.exists() || (!is_vss && platform::reparse(&src_file)) {
                return Err(
                    "Kaynak dosya (Geri Dönüşüm / VSS / MSI Önbelleği) artık erişilebilir değil."
                        .into(),
                );
            }
            let mut rf = File::open(&src_file).map_err(|e| e.to_string())?;
            let mut wf = File::create(&tmp).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 256 * 1024];
            let mut total = 0u64;
            loop {
                let n = rf.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                wf.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                hasher.update(&buf[..n]);
                total += n as u64;
            }
            wf.sync_all().map_err(|e| e.to_string())?;
            total
        }
        "usn_journal" | "indx_slack" => {
            return Err(
                "Bu kayıt yalnızca $UsnJrnl / $I30 dizin günlüğü izidir; dosya içeriğini kurtarmak için 'Derin Sektör İmza Kazıma' çalıştırın."
                    .into(),
            );
        }
        "mft_resident" => {
            let hex = item["locator"]["resident_hex"]
                .as_str()
                .ok_or("MFT resident verisi bulunamadı.")?;
            let data = decode_hex(hex)?;
            let mut wf = File::create(&tmp).map_err(|e| e.to_string())?;
            wf.write_all(&data).map_err(|e| e.to_string())?;
            wf.sync_all().map_err(|e| e.to_string())?;
            hasher.update(&data);
            data.len() as u64
        }
        "mft_nonresident" | "carved" => {
            #[cfg(windows)]
            {
                win::restore_raw_from_volume(src_char, item, &tmp, &mut hasher)?
            }
            #[cfg(not(windows))]
            {
                let _ = (&src_char, &tmp);
                return Err("Ham disk kümesi kurtarma yalnız Windows'ta desteklenir.".into());
            }
        }
        _ => return Err("Desteklenmeyen kurtarma kaynağı.".into()),
    };

    // Automatic media repair pass on recovered photos & videos (<= 64 MiB)
    let ext = item["extension"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_start_matches('.').to_ascii_lowercase())
        .unwrap_or_else(|| {
            dest_path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
        });
    let mut repaired = false;
    let mut repair_note: Option<String> = None;
    let mut media_status: Option<String> = None;
    let mut final_sha256 = format!("{:x}", hasher.finalize());

    if media_repair::is_supported_media_ext(&ext)
        && written_bytes > 0
        && written_bytes <= MAX_MEDIA_REPAIR_BYTES
    {
        if let Ok(raw) = std::fs::read(&tmp) {
            let (fixed, rep) = media_repair::repair_media(&ext, &raw);
            media_status = Some(rep.status.clone());
            if !rep.note_tr.is_empty() {
                repair_note = Some(rep.note_tr.clone());
            }
            if rep.repaired && rep.openable && !fixed.is_empty() {
                let mut wf = File::create(&tmp).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp);
                    e.to_string()
                })?;
                wf.write_all(&fixed).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp);
                    e.to_string()
                })?;
                wf.sync_all().map_err(|e| {
                    let _ = std::fs::remove_file(&tmp);
                    e.to_string()
                })?;
                let mut new_hasher = Sha256::new();
                new_hasher.update(&fixed);
                final_sha256 = format!("{:x}", new_hasher.finalize());
                written_bytes = fixed.len() as u64;
                repaired = true;
            }
        }
    }

    std::fs::rename(&tmp, dest_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(json!({
        "saved": true,
        "path": dest_path.to_string_lossy(),
        "bytes": written_bytes,
        "sha256": final_sha256,
        "repaired": repaired,
        "repair_note": repair_note,
        "media_status": media_status
    }))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Batch-restores multiple recovered items into `dest_dir` with anti-overwrite enforcement,
/// collision-safe filenames, automatic photo/video repair, and post-write SHA-256 verification.
pub fn restore_batch(job: &Value, item_ids: &[String], dest_dir: &Path) -> Result<Value, String> {
    if item_ids.is_empty() {
        return Err("Toplu kurtarma için en az bir dosya seçin.".into());
    }
    std::fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let canon_dest = dest_dir
        .canonicalize()
        .unwrap_or_else(|_| dest_dir.to_path_buf());
    let dest_drive = drive_letter_of(&canon_dest);

    let mut by_id: HashMap<&str, &Value> = HashMap::new();
    if let Some(arr) = job["recovery_items"].as_array() {
        for item in arr {
            if let Some(id) = item["id"].as_str() {
                by_id.insert(id, item);
            }
        }
    }

    let mut unique_ids = Vec::with_capacity(item_ids.len());
    let mut seen = HashSet::new();
    for id in item_ids {
        if seen.insert(id.as_str()) {
            unique_ids.push(id.as_str());
        }
    }

    // Pre-flight anti-overwrite check BEFORE writing any file:
    // Refuse if destination drive matches source drive for any raw/MFT/carved item.
    for &id in &unique_ids {
        let Some(&item) = by_id.get(id) else {
            continue;
        };
        let source = item["source"].as_str().unwrap_or("");
        if item["trim_zeroed"] == true
            || matches!(source, "usn_journal" | "indx_slack")
            || item["locator"]["metadata_only"] == true
        {
            continue;
        }
        let item_letter = item["source_letter"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| job["letter"].as_str())
            .unwrap_or("");
        let src_char = item_letter.chars().next().map(|c| c.to_ascii_uppercase());
        if !matches!(source, "recycle_bin" | "vss_shadow" | "installer_cache")
            && src_char.is_some()
            && dest_drive == src_char
        {
            return Err(
                "Silinen sektörlerin üzerine yazılmasını önlemek için kurtarma klasörü olarak FARKLI bir disk veya USB bellek seçin."
                    .into(),
            );
        }
    }

    let mut restored = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut repaired_count = 0usize;
    let mut paths: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for &id in &unique_ids {
        let Some(&item) = by_id.get(id) else {
            failed += 1;
            errors.push(format!("{id}: Kurtarma kaydı bulunamadı."));
            continue;
        };
        let source = item["source"].as_str().unwrap_or("");
        if item["trim_zeroed"] == true
            || matches!(source, "usn_journal" | "indx_slack")
            || item["locator"]["metadata_only"] == true
        {
            skipped += 1;
            continue;
        }
        let item_letter = item["source_letter"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| job["letter"].as_str())
            .unwrap_or("");
        let safe_name = sanitize_dest_filename(item["name"].as_str().unwrap_or("dosya.bin"));
        let target = unique_dest_path(dest_dir, &safe_name);
        match restore_item(item_letter, item, &target) {
            Ok(res) => {
                let expected_sha = res["sha256"].as_str().unwrap_or("");
                match sha256_file(&target) {
                    Ok(actual_sha) if !expected_sha.is_empty() && actual_sha == expected_sha => {
                        restored += 1;
                        if res["repaired"] == true {
                            repaired_count += 1;
                        }
                        paths.push(target.to_string_lossy().into_owned());
                    }
                    Ok(_) => {
                        let _ = std::fs::remove_file(&target);
                        failed += 1;
                        errors.push(format!("{safe_name}: SHA-256 doğrulaması başarısız."));
                    }
                    Err(e) => {
                        failed += 1;
                        errors.push(format!("{safe_name}: Doğrulama okuma hatası: {e}"));
                    }
                }
            }
            Err(e) => {
                failed += 1;
                errors.push(format!("{safe_name}: {e}"));
            }
        }
    }

    let repair_note = if repaired_count > 0 {
        Some(format!(
            "{repaired_count} bozuk/eksik fotoğraf veya video otomatik onarılarak açılabilir hale getirildi."
        ))
    } else {
        None
    };

    Ok(json!({
        "saved": restored > 0,
        "cancelled": false,
        "restored": restored,
        "failed": failed,
        "skipped": skipped,
        "dest_dir": dest_dir.to_string_lossy(),
        "paths": paths,
        "errors": errors,
        "repaired": repaired_count > 0,
        "repaired_count": repaired_count,
        "repair_note": repair_note,
    }))
}

#[allow(clippy::too_many_arguments)]
fn build_recycle_file_item(
    r_path: &Path,
    original_path: String,
    fallback_name: &str,
    size_hint: u64,
    deleted_at: Option<String>,
    letter: &str,
    is_ssd: bool,
    head_buf: &mut [u8],
) -> RecoveredItem {
    let actual_size = std::fs::metadata(r_path)
        .map(|m| m.len())
        .unwrap_or(size_hint);
    let mut n = 0usize;
    let mut pe = None;
    if let Ok(mut f) = File::open(r_path) {
        n = f.read(head_buf).unwrap_or(0);
        pe = analyze_pe_binary(&head_buf[..n]);
        if let Some(ref mut p_info) = pe {
            if let Some((rsrc_off, rsrc_sz)) = p_info.rsrc_offset {
                if rsrc_off >= n as u64
                    && rsrc_off < actual_size
                    && f.seek(SeekFrom::Start(rsrc_off)).is_ok()
                {
                    let read_cap = (rsrc_sz as usize).clamp(4096, 256 * 1024);
                    let mut rsrc_buf = vec![0u8; read_cap];
                    let got = f.read(&mut rsrc_buf).unwrap_or(0);
                    extract_pe_metadata(&rsrc_buf[..got], p_info);
                }
            }
        }
    }
    let ext = extension_of(&original_path);
    let sample = &head_buf[..n];
    let (_, mut conf, trim_zeroed, mut ai_summary) =
        classify_buffer_ai(sample, &ext, "recycle_bin", true, is_ssd);
    let (cat, mut sub_type) = category_for_file(&ext, &original_path, sample, pe.as_ref());

    // Media inspection on Recycle Bin photo/video
    let (media_status, openable, repair_note, dims) = if media_repair::is_supported_media_ext(&ext)
    {
        let inspect_slice_owned: Vec<u8> =
            if actual_size <= MAX_MEDIA_INSPECT_BYTES as u64 && actual_size > n as u64 {
                std::fs::read(r_path).unwrap_or_else(|_| sample.to_vec())
            } else if actual_size > MAX_MEDIA_INSPECT_BYTES as u64 {
                let mut big = Vec::with_capacity(MAX_MEDIA_INSPECT_BYTES);
                if let Ok(f) = File::open(r_path) {
                    let _ = f.take(MAX_MEDIA_INSPECT_BYTES as u64).read_to_end(&mut big);
                }
                if big.is_empty() {
                    sample.to_vec()
                } else {
                    big
                }
            } else {
                sample.to_vec()
            };
        inspect_media_bytes(&ext, &inspect_slice_owned, actual_size, true)
    } else {
        (None, true, None, None)
    };
    if let Some((w, h)) = dims {
        if !sub_type.contains('×') {
            sub_type = format!("{sub_type} ({w}×{h})");
        }
    }
    if !openable {
        conf = conf.min(15);
    }
    if let Some(ref note) = repair_note {
        if media_status.as_deref() != Some("intact") {
            ai_summary = format!("{note} {ai_summary}");
        }
    }

    let name = Path::new(&original_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(fallback_name)
        .to_owned();
    let status_label = match media_status.as_deref() {
        Some("repaired") => "Geri Dönüşüm · Otomatik Onarılacak".into(),
        Some("thumbnail_rescued") => "Geri Dönüşüm · Önizleme Kurtarılabilir".into(),
        Some("unrecoverable") => "Bozuk Medya (Açılamaz)".into(),
        _ => "Geri Dönüşüm Kutusu (%100 Sağlam)".into(),
    };

    RecoveredItem {
        id: format!("rb-{}", uuid::Uuid::new_v4().simple()),
        name,
        original_path,
        extension: ext,
        category: cat.to_owned(),
        sub_type,
        product_name: pe.as_ref().and_then(|p| p.product_name.clone()),
        publisher: pe.as_ref().and_then(|p| p.company_name.clone()),
        size_bytes: actual_size,
        deleted_at,
        source: "recycle_bin".into(),
        confidence: conf,
        status_label,
        entropy: shannon_entropy(&sample[..n.min(4096)]),
        trim_zeroed,
        ai_summary,
        source_letter: letter.to_owned(),
        locator: json!({"r_path": r_path.to_string_lossy()}),
        media_status,
        openable,
        repair_note,
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_recycled_dir(
    dir: &Path,
    orig_base: &str,
    depth: usize,
    apps_only: bool,
    deleted_at: &Option<String>,
    letter: &str,
    is_ssd: bool,
    head_buf: &mut [u8],
    out: &mut Vec<RecoveredItem>,
    per_dir_count: &mut usize,
    max_total: usize,
) {
    if depth > 3 || *per_dir_count >= 120 || out.len() >= max_total || platform::reparse(dir) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let trimmed_base = orig_base.trim_end_matches('\\');
    for entry in entries.flatten() {
        if *per_dir_count >= 120 || out.len() >= max_total {
            break;
        }
        let p = entry.path();
        if platform::reparse(&p) {
            continue;
        }
        let Some(fname) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let child_orig = format!("{trimmed_base}\\{fname}");
        if p.is_dir() {
            walk_recycled_dir(
                &p,
                &child_orig,
                depth + 1,
                apps_only,
                deleted_at,
                letter,
                is_ssd,
                head_buf,
                out,
                per_dir_count,
                max_total,
            );
        } else if p.is_file() {
            let ext = extension_of(fname);
            if apps_only && !matches!(ext.as_str(), "exe" | "msi" | "msix") {
                continue;
            }
            if is_junk_noise_path(&child_orig, &ext) {
                continue;
            }
            if matches!(ext.as_str(), "exe" | "msi" | "msix")
                && forensics::is_noise_app_exe(&child_orig, None)
            {
                continue;
            }
            *per_dir_count += 1;
            let sz = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(build_recycle_file_item(
                &p,
                child_orig,
                fname,
                sz,
                deleted_at.clone(),
                letter,
                is_ssd,
                head_buf,
            ));
        }
    }
}

/// Deep `$Recycle.Bin` scanner across all readable SID directories, handling BOTH deleted files
/// and deleted application/user directories (`$R...` folders).
pub fn scan_recycle_bin_dir(
    root: &Path,
    letter: &str,
    is_ssd: bool,
    apps_only: bool,
    cancel_fn: &dyn Fn() -> bool,
) -> Vec<RecoveredItem> {
    let mut items = Vec::new();
    let rdir = root.join("$Recycle.Bin");
    let Ok(sid_dirs) = std::fs::read_dir(&rdir) else {
        return items;
    };
    let max_total = if apps_only { 150 } else { 500 };
    let mut head_buf = vec![0u8; 64 * 1024];
    for sid_entry in sid_dirs.flatten() {
        if cancel_fn() || items.len() >= max_total {
            break;
        }
        let sid_path = sid_entry.path();
        if !sid_path.is_dir() || platform::reparse(&sid_path) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&sid_path) else {
            continue;
        };
        for entry in entries.flatten() {
            if cancel_fn() || items.len() >= max_total {
                break;
            }
            let p = entry.path();
            let Some(fname) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if fname.len() < 3 || !(fname.starts_with("$I") || fname.starts_with("$i")) {
                continue;
            }
            let suffix = &fname[2..];
            let r_upper = sid_path.join(format!("$R{suffix}"));
            let r_path = if r_upper.exists() {
                r_upper
            } else {
                let r_lower = sid_path.join(format!("$r{suffix}"));
                if r_lower.exists() {
                    r_lower
                } else {
                    continue;
                }
            };
            if platform::reparse(&r_path) {
                continue;
            }
            let Ok(i_bytes) = std::fs::read(&p) else {
                continue;
            };
            let Some((size_bytes, deleted_at, original_path)) = parse_recycle_i_bytes(&i_bytes)
            else {
                continue;
            };
            let item_letter = forensics::extract_drive_letter(&original_path)
                .unwrap_or_else(|| letter.to_ascii_uppercase());

            if r_path.is_file() {
                let ext = extension_of(&original_path);
                if apps_only && !matches!(ext.as_str(), "exe" | "msi" | "msix") {
                    continue;
                }
                items.push(build_recycle_file_item(
                    &r_path,
                    original_path,
                    fname,
                    size_bytes,
                    deleted_at,
                    &item_letter,
                    is_ssd,
                    &mut head_buf,
                ));
            } else if r_path.is_dir() {
                let mut per_dir = 0usize;
                walk_recycled_dir(
                    &r_path,
                    &original_path,
                    0,
                    apps_only,
                    &deleted_at,
                    &item_letter,
                    is_ssd,
                    &mut head_buf,
                    &mut items,
                    &mut per_dir,
                    max_total,
                );
            }
        }
    }
    items
}

fn scan_recycle_bin(core: &Core, root: &Path, letter: &str, is_ssd: bool) -> Vec<RecoveredItem> {
    let cancel_fn = || core.job_cancel.load(Ordering::SeqCst) || core.stop.load(Ordering::SeqCst);
    scan_recycle_bin_dir(root, letter, is_ssd, false, &cancel_fn)
}

fn recovery_source_rank(source: &str, trim_zeroed: bool) -> u8 {
    if trim_zeroed {
        return 0;
    }
    match source {
        "vss_shadow" => 5,
        "recycle_bin" => 4,
        "installer_cache" => 3,
        "mft_resident" => 2,
        "mft_nonresident" | "carved" => 1,
        _ => 0,
    }
}

fn is_setup_like_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let fname = lower.rsplit('\\').next().unwrap_or(&lower);
    lower.contains("\\downloads\\")
        || lower.contains("\\windows\\installer\\")
        || fname.contains("setup")
        || fname.contains("installer")
        || fname.ends_with(".msi")
        || fname.ends_with(".msix")
}

/// Correlates, noise-filters, deduplicates, categorizes, and ranks all deleted apps & games
/// from recoverable binaries, multi-source Windows forensic traces, and surviving AppData folders.
pub fn build_deleted_app_bundles(
    selected_letter: &str,
    items: &[RecoveredItem],
    traces: &[ForensicAppTrace],
    appdata_entries: &[AppDataEntry],
    vss_by_path: &HashMap<String, String>,
) -> Vec<DeletedAppBundle> {
    struct Acc {
        bundle: DeletedAppBundle,
        best_rank: u8,
        is_game_hint: bool,
        has_real_exe_path: bool,
        appdata_seen: HashSet<String>,
    }

    let selected_upper = selected_letter
        .trim()
        .trim_end_matches(':')
        .to_ascii_uppercase();
    let mut accs: Vec<Acc> = Vec::new();
    let mut idx_by_canon: HashMap<String, usize> = HashMap::new();

    // Index AppData folders by canonical app key for instant correlation
    let mut appdata_by_key: HashMap<String, Vec<&AppDataEntry>> = HashMap::new();
    for entry in appdata_entries {
        let k = forensics::canonical_app_key(&entry.folder_name);
        if k.len() >= 2 {
            appdata_by_key.entry(k).or_default().push(entry);
        }
    }

    let attach_appdata = |acc: &mut Acc, canon_key: &str| {
        if let Some(entries) = appdata_by_key.get(canon_key) {
            for e in entries {
                let pl = e.path.to_ascii_lowercase();
                if acc.appdata_seen.insert(pl) {
                    acc.bundle.appdata_paths.push(e.path.clone());
                    acc.bundle.leftover_bytes =
                        acc.bundle.leftover_bytes.saturating_add(e.size_bytes);
                    if !acc.bundle.sources.iter().any(|s| s == &e.scope) {
                        acc.bundle.sources.push(e.scope.clone());
                    }
                }
            }
        }
    };

    // 1. Index directly recoverable EXE/MSI/MSIX binaries (VSS, Recycle Bin, MSI/Downloads Cache, $MFT, Carved)
    for item in items {
        let is_app_item = matches!(item.category.as_str(), "exe_setup" | "game_portable")
            || matches!(item.extension.as_str(), "exe" | "msi" | "msix");
        if !is_app_item {
            continue;
        }
        if forensics::is_noise_app_exe(&item.original_path, item.product_name.as_deref()) {
            continue;
        }

        let raw_name = item
            .product_name
            .as_deref()
            .filter(|p| p.trim().len() >= 2)
            .unwrap_or(&item.name);
        let canon = forensics::canonical_app_key(raw_name);
        let canon = if canon.len() < 2 {
            forensics::canonical_app_key(&item.original_path)
        } else {
            canon
        };
        if canon.len() < 2 {
            continue;
        }

        let src_label = match item.source.as_str() {
            "vss_shadow" => "VSS Gölge Kopya",
            "installer_cache" => {
                if item.status_label.contains("İndirilenler") {
                    "İndirilenler Kurulumu"
                } else {
                    "Windows MSI Önbelleği"
                }
            }
            "recycle_bin" => "Geri Dönüşüm Kutusu",
            "carved" => "Derin PE Kazıma",
            "usn_journal" => "$UsnJrnl Günlüğü",
            _ => "NTFS $MFT",
        };
        let rank = recovery_source_rank(&item.source, item.trim_zeroed);
        let is_restorable = rank > 0;
        let is_full_restore = rank >= 4;
        let is_game = item.category == "game_portable";
        let real_exe = !is_setup_like_path(&item.original_path)
            && item.original_path.to_ascii_lowercase().ends_with(".exe");
        let item_drive = forensics::extract_drive_letter(&item.original_path)
            .unwrap_or_else(|| item.source_letter.to_ascii_uppercase());
        let display_name = forensics::clean_app_display_name(raw_name, &item.original_path);

        if let Some(&idx) = idx_by_canon.get(&canon) {
            let acc = &mut accs[idx];
            if !acc.bundle.sources.iter().any(|s| s == src_label) {
                acc.bundle.sources.push(src_label.into());
            }
            if acc.bundle.publisher.is_none() && item.publisher.is_some() {
                acc.bundle.publisher = item.publisher.clone();
            }
            if is_game {
                acc.is_game_hint = true;
            }
            if real_exe && !acc.has_real_exe_path {
                acc.bundle.exe_path = item.original_path.clone();
                acc.bundle.install_dir = forensics::extract_install_dir(&item.original_path);
                acc.bundle.drive_letter = item_drive;
                acc.has_real_exe_path = true;
            }
            if rank > acc.best_rank {
                acc.best_rank = rank;
                acc.bundle.installer_found = true;
                acc.bundle.recoverable_item_id = Some(item.id.clone());
                if item.source == "vss_shadow" {
                    acc.bundle.vss_recoverable = true;
                }
                acc.bundle.confidence = acc.bundle.confidence.max(if is_full_restore {
                    100
                } else {
                    item.confidence
                });
            }
            continue;
        }

        let install_dir = forensics::extract_install_dir(&item.original_path);
        let category =
            forensics::classify_app_category(&canon, &item.original_path, &display_name, is_game);
        let winget_hint = forensics::winget_command_for_app(&canon, &display_name);
        let recovery_status = if is_full_restore {
            "full_restore"
        } else if is_restorable {
            "installer_found"
        } else {
            "reinstall_only"
        };

        let mut acc = Acc {
            bundle: DeletedAppBundle {
                name: display_name,
                publisher: item.publisher.clone(),
                exe_path: item.original_path.clone(),
                installer_found: is_restorable,
                recoverable_item_id: is_restorable.then(|| item.id.clone()),
                appdata_paths: Vec::new(),
                winget_hint,
                confidence: if is_full_restore {
                    100
                } else {
                    item.confidence
                },
                sources: vec![src_label.into()],
                run_count: None,
                last_run_at: item.deleted_at.clone(),
                vss_recoverable: item.source == "vss_shadow",
                category: category.to_owned(),
                recovery_status: recovery_status.to_owned(),
                leftover_bytes: 0,
                install_dir,
                drive_letter: item_drive,
            },
            best_rank: rank,
            is_game_hint: is_game,
            has_real_exe_path: real_exe,
            appdata_seen: HashSet::new(),
        };
        attach_appdata(&mut acc, &canon);
        idx_by_canon.insert(canon, accs.len());
        accs.push(acc);
    }

    // 2. Fuse all Windows forensic traces (FirewallRules, GameConfigStore, Prefetch, ShimCache,
    //    UserAssist, BAM, Orphaned Uninstall/App Paths, Dead .lnk, AppData/ProgramData, MuiCache)
    for tr in traces {
        if forensics::is_noise_app_exe(&tr.exe_path, Some(&tr.name)) {
            continue;
        }
        let canon = {
            let k1 = forensics::canonical_app_key(&tr.name);
            if k1.len() >= 2 {
                k1
            } else {
                forensics::canonical_app_key(&tr.exe_path)
            }
        };
        if canon.len() < 2 {
            continue;
        }

        let vss_id = vss_by_path.get(&tr.exe_path.to_ascii_lowercase()).cloned();
        let tr_real_exe = !is_setup_like_path(&tr.exe_path)
            && forensics::extract_drive_letter(&tr.exe_path).is_some()
            && tr.exe_path.to_ascii_lowercase().ends_with(".exe");
        let display_name = forensics::clean_app_display_name(&tr.name, &tr.exe_path);

        if let Some(&idx) = idx_by_canon.get(&canon) {
            let acc = &mut accs[idx];
            for s in &tr.sources {
                if !acc.bundle.sources.iter().any(|cur| cur == s) {
                    acc.bundle.sources.push(s.clone());
                }
            }
            if acc.bundle.publisher.is_none() && tr.publisher.is_some() {
                acc.bundle.publisher = tr.publisher.clone();
            }
            if let Some(rc) = tr.run_count {
                acc.bundle.run_count = Some(acc.bundle.run_count.unwrap_or(0).max(rc));
            }
            if let Some(ref ts) = tr.last_run_at {
                if acc.bundle.last_run_at.as_deref().unwrap_or("") < ts.as_str() {
                    acc.bundle.last_run_at = Some(ts.clone());
                }
            }
            if tr.is_game {
                acc.is_game_hint = true;
            }
            // Prefer real installed exe_path (e.g. `Spotify.exe`) over setup installer (`SpotifySetup.exe`)
            if tr_real_exe && !acc.has_real_exe_path {
                acc.bundle.exe_path = tr.exe_path.clone();
                acc.bundle.install_dir = tr
                    .install_dir
                    .clone()
                    .or_else(|| forensics::extract_install_dir(&tr.exe_path));
                acc.bundle.drive_letter = tr.drive_letter.clone();
                acc.has_real_exe_path = true;
            } else if acc.bundle.install_dir.is_none() && tr.install_dir.is_some() {
                acc.bundle.install_dir = tr.install_dir.clone();
            }
            for ap in &tr.appdata_paths {
                if acc.appdata_seen.insert(ap.to_ascii_lowercase()) {
                    acc.bundle.appdata_paths.push(ap.clone());
                    acc.bundle.leftover_bytes =
                        acc.bundle.leftover_bytes.saturating_add(tr.leftover_bytes);
                }
            }
            if let Some(vid) = vss_id {
                if acc.best_rank < 5 {
                    acc.best_rank = 5;
                    acc.bundle.vss_recoverable = true;
                    acc.bundle.installer_found = true;
                    acc.bundle.recoverable_item_id = Some(vid);
                    acc.bundle.confidence = 100;
                }
            }
            // Boost confidence for multi-source corroboration
            let multi_conf = (65 + (acc.bundle.sources.len() as u32 * 7)).min(96);
            acc.bundle.confidence = acc.bundle.confidence.max(multi_conf);
            continue;
        }

        let has_vss = vss_id.is_some();
        let rank = if has_vss { 5 } else { 0 };
        let base_conf = if has_vss {
            100
        } else {
            (62 + (tr.sources.len() as u32 * 8)).min(95)
        };
        let category =
            forensics::classify_app_category(&canon, &tr.exe_path, &display_name, tr.is_game);
        let winget_hint = forensics::winget_command_for_app(&canon, &display_name);
        let mut appdata_seen = HashSet::new();
        let mut appdata_paths = Vec::new();
        for ap in &tr.appdata_paths {
            if appdata_seen.insert(ap.to_ascii_lowercase()) {
                appdata_paths.push(ap.clone());
            }
        }
        let mut acc = Acc {
            bundle: DeletedAppBundle {
                name: display_name,
                publisher: tr.publisher.clone(),
                exe_path: tr.exe_path.clone(),
                installer_found: has_vss,
                recoverable_item_id: vss_id,
                appdata_paths,
                winget_hint,
                confidence: base_conf,
                sources: tr.sources.clone(),
                run_count: tr.run_count,
                last_run_at: tr.last_run_at.clone(),
                vss_recoverable: has_vss,
                category: category.to_owned(),
                recovery_status: if has_vss {
                    "full_restore".into()
                } else {
                    "reinstall_only".into()
                },
                leftover_bytes: tr.leftover_bytes,
                install_dir: tr
                    .install_dir
                    .clone()
                    .or_else(|| forensics::extract_install_dir(&tr.exe_path)),
                drive_letter: if tr.drive_letter.is_empty() {
                    selected_upper.clone()
                } else {
                    tr.drive_letter.to_ascii_uppercase()
                },
            },
            best_rank: rank,
            is_game_hint: tr.is_game,
            has_real_exe_path: tr_real_exe,
            appdata_seen,
        };
        attach_appdata(&mut acc, &canon);
        idx_by_canon.insert(canon, accs.len());
        accs.push(acc);
    }

    // Finalize category, recovery_status, and sort cleanly
    let mut bundles: Vec<DeletedAppBundle> = accs
        .into_iter()
        .map(|mut acc| {
            let canon = forensics::canonical_app_key(&acc.bundle.name);
            acc.bundle.category = forensics::classify_app_category(
                &canon,
                &acc.bundle.exe_path,
                &acc.bundle.name,
                acc.is_game_hint,
            )
            .to_owned();
            acc.bundle.recovery_status = if acc.bundle.vss_recoverable || acc.best_rank >= 4 {
                "full_restore".into()
            } else if acc.bundle.installer_found && acc.bundle.recoverable_item_id.is_some() {
                "installer_found".into()
            } else {
                "reinstall_only".into()
            };
            acc.bundle.winget_hint = forensics::winget_command_for_app(&canon, &acc.bundle.name);
            if acc.bundle.drive_letter.is_empty() {
                acc.bundle.drive_letter = forensics::extract_drive_letter(&acc.bundle.exe_path)
                    .unwrap_or_else(|| selected_upper.clone());
            }
            acc.bundle
        })
        .collect();

    bundles.sort_by(|a, b| {
        let a_full = a.recovery_status == "full_restore";
        let b_full = b.recovery_status == "full_restore";
        let a_sel = a.drive_letter.eq_ignore_ascii_case(&selected_upper);
        let b_sel = b.drive_letter.eq_ignore_ascii_case(&selected_upper);
        let a_inst = a.recovery_status == "installer_found";
        let b_inst = b.recovery_status == "installer_found";
        b_full
            .cmp(&a_full)
            .then_with(|| b_sel.cmp(&a_sel))
            .then_with(|| b_inst.cmp(&a_inst))
            .then_with(|| b.sources.len().cmp(&a.sources.len()))
            .then_with(|| b.run_count.unwrap_or(0).cmp(&a.run_count.unwrap_or(0)))
            .then_with(|| (b.leftover_bytes > 0).cmp(&(a.leftover_bytes > 0)))
            .then_with(|| b.confidence.cmp(&a.confidence))
    });
    bundles.truncate(MAX_APPS);
    bundles
}

pub fn run(core: &Core, request: &JobRequest, disk: &Disk, root: &Path) -> Result<(), String> {
    let mode = request.options["mode"].as_str().unwrap_or("quick");
    if mode == "deep" && !platform::is_admin() {
        return Err("Derin ham sektör imza kazıma taraması Yönetici yetkisi gerektirir.".into());
    }
    let is_ssd = disk.kind != "HDD";
    let mut items = Vec::new();
    let mut mft_scanned = 0u64;
    let mut dir_map: HashMap<u64, (u64, String)> = HashMap::new();
    let mut usn_deleted_count = 0usize;
    let mut indx_ghost_count = 0usize;

    phase(
        core,
        "1/5 · Geri Dönüşüm Kutusu (dosya & klasörler), VSS Gölge Kopyaları ve adli kaynaklar taranıyor",
    );
    items.extend(scan_recycle_bin(core, root, &request.letter, is_ssd));

    // Also inspect SystemDrive $Recycle.Bin for deleted apps/installers if user scanned a secondary drive
    #[cfg(windows)]
    {
        let sys_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let sys_letter = sys_drive.trim_end_matches(':').to_ascii_uppercase();
        if !sys_letter.eq_ignore_ascii_case(&request.letter) {
            let sys_root = PathBuf::from(format!("{sys_letter}:\\"));
            let cancel_fn =
                || core.job_cancel.load(Ordering::SeqCst) || core.stop.load(Ordering::SeqCst);
            items.extend(scan_recycle_bin_dir(
                &sys_root,
                &sys_letter,
                true,
                true,
                &cancel_fn,
            ));
        }
    }

    let mut forensic = forensics::collect_forensic_sweep(&request.letter);
    let mut vss_by_path: HashMap<String, String> = HashMap::new();
    let mut head_probe = vec![0u8; 64 * 1024];

    for vf in &forensic.vss_files {
        let ext = extension_of(&vf.original_path);
        let n = File::open(&vf.vss_path)
            .and_then(|mut f| f.read(&mut head_probe))
            .unwrap_or(0);
        let sample = &head_probe[..n];
        let pe = analyze_pe_binary(sample);
        let (cat, mut sub_type) = category_for_file(&ext, &vf.original_path, sample, pe.as_ref());
        let (media_status, openable, repair_note, dims) =
            if media_repair::is_supported_media_ext(&ext) {
                let media_buf = if vf.size_bytes <= MAX_MEDIA_INSPECT_BYTES as u64 {
                    std::fs::read(&vf.vss_path).unwrap_or_else(|_| sample.to_vec())
                } else {
                    sample.to_vec()
                };
                inspect_media_bytes(&ext, &media_buf, vf.size_bytes, true)
            } else {
                (None, true, None, None)
            };
        if let Some((w, h)) = dims {
            if !sub_type.contains('×') {
                sub_type = format!("{sub_type} ({w}×{h})");
            }
        }
        let name = Path::new(&vf.original_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("dosya")
            .to_owned();
        let id = format!("vss-{}", uuid::Uuid::new_v4().simple());
        vss_by_path.insert(vf.original_path.to_ascii_lowercase(), id.clone());
        items.push(RecoveredItem {
            id,
            name,
            original_path: vf.original_path.clone(),
            extension: ext,
            category: cat.to_owned(),
            sub_type,
            product_name: pe.as_ref().and_then(|p| p.product_name.clone()),
            publisher: pe.as_ref().and_then(|p| p.company_name.clone()),
            size_bytes: vf.size_bytes,
            deleted_at: vf.modified_at.clone(),
            source: "vss_shadow".into(),
            confidence: if openable { 100 } else { 15 },
            status_label: "VSS Gölge Kopya (%100 Sağlam · TRIM Korumalı)".into(),
            entropy: shannon_entropy(&sample[..n.min(4096)]),
            trim_zeroed: false,
            ai_summary: "Windows Volume Shadow Copy anlık görüntüsünde eksiksiz bulundu; SSD TRIM'den etkilenmeden %100 kurtarılabilir.".into(),
            source_letter: vf.drive_letter.clone(),
            locator: json!({"src_path": vf.vss_path}),
            media_status,
            openable,
            repair_note,
        });
    }
    for msi in &forensic.cached_msis {
        let fname = format!("{}.msi", sanitize_filename(&msi.product_name));
        let msi_letter = forensics::extract_drive_letter(&msi.msi_path)
            .unwrap_or_else(|| request.letter.clone());
        items.push(RecoveredItem {
            id: format!("msi-{}", uuid::Uuid::new_v4().simple()),
            name: fname,
            original_path: msi.msi_path.clone(),
            extension: "msi".into(),
            category: "exe_setup".into(),
            sub_type: "Windows Installer Önbelleği (.msi)".into(),
            product_name: Some(msi.product_name.clone()),
            publisher: msi.publisher.clone(),
            size_bytes: msi.size_bytes,
            deleted_at: None,
            source: "installer_cache".into(),
            confidence: 100,
            status_label: "MSI Kurulum Önbelleği (%100 Sağlam)".into(),
            entropy: 6.0,
            trim_zeroed: false,
            ai_summary:
                "C:\\Windows\\Installer gizli kurulum önbelleğinden doğrudan yeniden kurulabilir."
                    .into(),
            source_letter: msi_letter,
            locator: json!({"src_path": msi.msi_path}),
            media_status: None,
            openable: true,
            repair_note: None,
        });
    }
    lock(&core.job)["progress"] = json!(15);

    #[cfg(windows)]
    {
        if platform::is_admin()
            && disk
                .volumes
                .iter()
                .any(|v| v["letter"] == request.letter && v["filesystem"] == "NTFS")
        {
            phase(
                core,
                "2/5 · NTFS $MFT tam kapsam dizin ağacı ve $UsnJrnl:$J silinme günlüğü okunuyor",
            );
            let max_records = if mode == "deep" { 1_200_000 } else { 350_000 };
            if let Ok((mft_items, scanned, dirs)) = win::scan_ntfs_mft(
                core,
                &request.letter,
                mode == "apps_only",
                mode == "dev",
                is_ssd,
                max_records,
            ) {
                items.extend(mft_items);
                mft_scanned = scanned;
                dir_map = dirs;
            }
            // Enrich & supplement with NTFS $UsnJrnl:$J deleted records (DFIR)
            let usn_records = forensics::read_usn_deleted_records(&request.letter, 400);
            usn_deleted_count = usn_records.len();
            let mut mft_ts_map: HashMap<u64, String> = HashMap::new();
            let mut existing_paths: HashSet<String> = items
                .iter()
                .map(|i| i.original_path.to_ascii_lowercase())
                .collect();
            for rec in &usn_records {
                if let Some(ref ts) = rec.deleted_at {
                    mft_ts_map.entry(rec.file_ref).or_insert_with(|| ts.clone());
                }
            }
            for item in &mut items {
                if let Some(mft_str) = item.id.strip_prefix("mft-") {
                    if let Ok(rec_idx) = mft_str.parse::<u64>() {
                        if let Some(usn_ts) = mft_ts_map.get(&rec_idx) {
                            item.deleted_at = Some(usn_ts.clone());
                        }
                    }
                }
            }
            let mut usn_added = 0usize;
            for rec in usn_records {
                if usn_added >= 80 {
                    break;
                }
                let ext = extension_of(&rec.file_name);
                let full_path =
                    reconstruct_mft_path(&request.letter, rec.parent_ref, &rec.file_name, &dir_map);
                if is_junk_noise_path(&full_path, &ext) {
                    continue;
                }
                if mode == "apps_only" && !matches!(ext.as_str(), "exe" | "msi" | "msix") {
                    continue;
                }
                if mode == "dev" && !is_dev_project_file(&full_path, &ext) {
                    continue;
                }
                if existing_paths.insert(full_path.to_ascii_lowercase()) {
                    let (cat, sub_type) = category_for_file(&ext, &full_path, &[], None);
                    if cat == "system_lib" || cat == "other" {
                        continue;
                    }
                    usn_added += 1;
                    items.push(RecoveredItem {
                        id: format!("usn-{}-{}", rec.file_ref, usn_added),
                        name: rec.file_name,
                        original_path: full_path,
                        extension: ext,
                        category: cat.to_owned(),
                        sub_type,
                        product_name: None,
                        publisher: None,
                        size_bytes: 0,
                        deleted_at: rec.deleted_at,
                        source: "usn_journal".into(),
                        confidence: 35,
                        status_label: "$UsnJrnl Silinme Kaydı (MFT Yeniden Kullanılmış)".into(),
                        entropy: 0.0,
                        trim_zeroed: false,
                        ai_summary: "NTFS $UsnJrnl:$J işlem günlüğünde silinme zamanı tespit edildi; içerik için Derin Sektör Kazıma önerilir.".into(),
                        source_letter: request.letter.clone(),
                        locator: json!({"metadata_only": true, "file_ref": rec.file_ref}),
                        media_status: None,
                        openable: true,
                        repair_note: None,
                    });
                }
            }
        }
        lock(&core.job)["progress"] = json!(45);

        if (mode == "deep" || mode == "apps_only") && platform::is_admin() {
            let size_mib = request.options["size_mib"].as_u64().unwrap_or(16_384);
            let budget_bytes = if size_mib == 0 {
                FULL_SCAN_CAP_BYTES
            } else if mode == "apps_only" {
                4 * 1024 * 1024 * 1024u64
            } else {
                size_mib * 1024 * 1024
            };
            phase(
                core,
                if mode == "apps_only" {
                    "3/5 · Boş kümelerde kayıp EXE/MSI kurulum imzaları aranıyor"
                } else {
                    "3/5 · Derin boş sektör imza kazıma ve medya onarım analizi (PE / Foto / Video / Belge / $I30)"
                },
            );
            if let Ok((carved, ghosts)) = win::scan_signature_carving(
                core,
                &request.letter,
                budget_bytes,
                mode == "apps_only",
                is_ssd,
                &dir_map,
            ) {
                indx_ghost_count = ghosts;
                items.extend(carved);
            }
        }
        lock(&core.job)["progress"] = json!(85);
    }

    cancelled(core)?;
    phase(
        core,
        "4/5 · 11 Kaynaklı Kayıp Uygulama Korelasyonu (Firewall, GameConfigStore, Prefetch, ShimCache, UserAssist, BAM, AppData, VSS)",
    );

    // Append MuiCache / AppCompatFlags traces (across ALL local drive letters!)
    #[cfg(windows)]
    {
        for (exe_path, friendly_name) in win::scan_deleted_app_registry() {
            if is_junk_noise_path(&exe_path, "exe")
                || forensics::is_noise_app_exe(&exe_path, Some(&friendly_name))
            {
                continue;
            }
            let dl = forensics::extract_drive_letter(&exe_path)
                .unwrap_or_else(|| request.letter.to_ascii_uppercase());
            let clean_name = forensics::clean_app_display_name(&friendly_name, &exe_path);
            *forensic
                .source_counts
                .entry("MuiCache / AppCompat".into())
                .or_insert(0) += 1;
            forensic.traces.push(ForensicAppTrace {
                install_dir: forensics::extract_install_dir(&exe_path),
                exe_path,
                name: clean_name,
                publisher: None,
                sources: vec!["MuiCache / AppCompat".into()],
                run_count: None,
                last_run_at: None,
                vss_path: None,
                drive_letter: dl,
                is_game: false,
                appdata_paths: Vec::new(),
                leftover_bytes: 0,
            });
        }
    }

    // Correlate live Downloads installers ONLY when they match a detected deleted app!
    let mut detected_keys: HashSet<String> = forensic
        .traces
        .iter()
        .map(|t| forensics::canonical_app_key(&t.name))
        .filter(|k| k.len() >= 2)
        .collect();
    for item in &items {
        if matches!(item.category.as_str(), "exe_setup" | "game_portable")
            || matches!(item.extension.as_str(), "exe" | "msi" | "msix")
        {
            let k =
                forensics::canonical_app_key(item.product_name.as_deref().unwrap_or(&item.name));
            if k.len() >= 2 {
                detected_keys.insert(k);
            }
        }
    }
    for dl_inst in &forensic.downloads_installers {
        let k = forensics::canonical_app_key(&dl_inst.file_name);
        if k.len() >= 2 && detected_keys.contains(&k) {
            let ext = extension_of(&dl_inst.file_name);
            let n = File::open(&dl_inst.path)
                .and_then(|mut f| f.read(&mut head_probe))
                .unwrap_or(0);
            let pe = analyze_pe_binary(&head_probe[..n]);
            let (_, sub_type) =
                category_for_file(&ext, &dl_inst.path, &head_probe[..n], pe.as_ref());
            items.push(RecoveredItem {
                id: format!("dl-{}", uuid::Uuid::new_v4().simple()),
                name: dl_inst.file_name.clone(),
                original_path: dl_inst.path.clone(),
                extension: ext,
                category: "exe_setup".into(),
                sub_type,
                product_name: pe.as_ref().and_then(|p| p.product_name.clone()),
                publisher: pe.as_ref().and_then(|p| p.company_name.clone()),
                size_bytes: dl_inst.size_bytes,
                deleted_at: dl_inst.modified_at.clone(),
                source: "installer_cache".into(),
                confidence: 100,
                status_label: "İndirilenler Kurulum Dosyası (%100 Sağlam)".into(),
                entropy: 6.5,
                trim_zeroed: false,
                ai_summary: "İndirilenler klasöründe bu silinmiş uygulamanın orijinal kurulum dosyası mevcut.".into(),
                source_letter: dl_inst.drive_letter.clone(),
                locator: json!({"src_path": dl_inst.path}),
                media_status: None,
                openable: true,
                repair_note: None,
            });
        }
    }

    let apps = build_deleted_app_bundles(
        &request.letter,
        &items,
        &forensic.traces,
        &forensic.appdata_entries,
        &vss_by_path,
    );

    phase(
        core,
        "5/5 · Sonuçlar önem, açılabilirlik ve bütünlüğe göre sıralanıyor",
    );
    if mode == "apps_only" {
        items.retain(|i| matches!(i.category.as_str(), "exe_setup" | "game_portable"));
    } else if mode == "dev" {
        items.retain(|i| {
            i.category == "database_code"
                || is_dev_project_file(&i.original_path, &i.extension)
                || is_dev_project_file(&i.name, &i.extension)
        });
    }
    items.sort_by(|a, b| {
        let a_vss = a.source == "vss_shadow";
        let b_vss = b.source == "vss_shadow";
        let a_env =
            mode == "dev" && (is_env_filename(&a.name) || is_env_filename(&a.original_path));
        let b_env =
            mode == "dev" && (is_env_filename(&b.name) || is_env_filename(&b.original_path));
        let a_app = matches!(a.category.as_str(), "exe_setup" | "game_portable");
        let b_app = matches!(b.category.as_str(), "exe_setup" | "game_portable");
        let a_sys = a.category == "system_lib";
        let b_sys = b.category == "system_lib";
        b.openable
            .cmp(&a.openable)
            .then_with(|| b_vss.cmp(&a_vss))
            .then_with(|| b_env.cmp(&a_env))
            .then_with(|| a_sys.cmp(&b_sys))
            .then_with(|| b_app.cmp(&a_app))
            .then_with(|| b.confidence.cmp(&a.confidence))
            .then_with(|| b.deleted_at.cmp(&a.deleted_at))
    });
    items.truncate(MAX_RESULTS);

    let dev_files_found = items
        .iter()
        .filter(|i| {
            i.category == "database_code"
                || is_dev_project_file(&i.original_path, &i.extension)
                || is_dev_project_file(&i.name, &i.extension)
        })
        .count();
    let high_conf = items
        .iter()
        .filter(|i| i.openable && i.confidence >= 75 && i.category != "system_lib")
        .count();
    let exe_found = items
        .iter()
        .filter(|i| matches!(i.category.as_str(), "exe_setup" | "game_portable") && !i.trim_zeroed)
        .count();
    let vss_found = items.iter().filter(|i| i.source == "vss_shadow").count();
    let trim_count = items.iter().filter(|i| i.trim_zeroed).count();
    let media_repairable = items
        .iter()
        .filter(|i| {
            matches!(
                i.media_status.as_deref(),
                Some("repaired" | "thumbnail_rescued")
            )
        })
        .count();
    let media_unrecoverable = items.iter().filter(|i| !i.openable).count();

    let mut job = lock(&core.job);
    job["progress"] = json!(100);
    job["recovery_items"] = json!(items);
    job["deleted_apps"] = json!(apps);
    job["repair_summary"] = json!({
        "mode": mode,
        "total_items": items.len(),
        "high_confidence": high_conf,
        "exe_found": exe_found,
        "mft_scanned": mft_scanned,
        "vss_shadows": forensic.vss_shadows_found,
        "vss_found": vss_found,
        "usn_deleted": usn_deleted_count,
        "indx_ghosts": indx_ghost_count,
        "forensic_sources": forensic.source_counts,
        "trim_zeroed": trim_count,
        "media_repairable": media_repairable,
        "media_unrecoverable": media_unrecoverable,
        "deleted_apps": apps.len(),
        "dev_files_found": dev_files_found,
        "scope": "11 kaynaklı adli tespit ve medya onarım motoru (VSS, Geri Dönüşüm, $MFT, $UsnJrnl, $I30, PE/Medya Kazıma, Prefetch, GameConfigStore, Güvenlik Duvarı, ShimCache, UserAssist, BAM, AppData, .lnk, Yetim Kayıt)."
    });
    Ok(())
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_MORE_DATA, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetDiskFreeSpaceW, ReadFile, SetFilePointerEx, FILE_BEGIN,
        FILE_FLAG_NO_BUFFERING, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
    };
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const GENERIC_READ: u32 = 0x8000_0000;
    const FSCTL_GET_VOLUME_BITMAP: u32 = 0x0009_006F;

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

    struct PageBuf {
        ptr: *mut u8,
        len: usize,
    }
    impl PageBuf {
        fn alloc(len: usize) -> Result<Self, String> {
            let ptr = unsafe { VirtualAlloc(null(), len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) }
                as *mut u8;
            if ptr.is_null() {
                return Err("Doğrudan disk okuma tamponu ayrılamadı.".into());
            }
            Ok(Self { ptr, len })
        }
        fn as_mut(&mut self) -> &mut [u8] {
            unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
        }
    }
    impl Drop for PageBuf {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                unsafe {
                    VirtualFree(self.ptr.cast(), 0, MEM_RELEASE);
                }
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn open_vol(letter: &str) -> Result<WinHandle, String> {
        let w = wide(&format!("\\\\.\\{letter}:"));
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_NO_BUFFERING,
                null_mut(),
            )
        };
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return Err(format!(
                "Bölüm ham erişimle açılamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(WinHandle(h))
    }

    fn read_at(h: HANDLE, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
        let mut pos = 0i64;
        if unsafe { SetFilePointerEx(h, offset as i64, &mut pos, FILE_BEGIN) } == 0 {
            return Err(format!(
                "Konumlanamadı: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut got = 0u32;
        if unsafe {
            ReadFile(
                h,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut got,
                null_mut(),
            )
        } == 0
        {
            return Err(format!("Okuma hatası: {}", std::io::Error::last_os_error()));
        }
        Ok(got as usize)
    }

    fn read_runs_into_vec(
        vh: HANDLE,
        runs: &[(i64, u64)],
        cluster_bytes: u64,
        max_bytes: usize,
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(max_bytes.min(256 * 1024));
        let chunk_cap = (512 * 1024usize)
            .min(max_bytes.div_ceil(4096) * 4096)
            .max(4096);
        let Ok(mut pbuf) = PageBuf::alloc(chunk_cap) else {
            return out;
        };
        let mut remaining = max_bytes;
        for &(lcn, cnt) in runs {
            if remaining == 0 {
                break;
            }
            if lcn < 0 || cnt == 0 {
                continue;
            }
            let mut run_rem = (cnt.saturating_mul(cluster_bytes) as usize).min(remaining);
            let mut run_off = (lcn as u64).saturating_mul(cluster_bytes);
            while run_rem > 0 {
                let want = run_rem.min(chunk_cap);
                let aligned = want.div_ceil(4096) * 4096;
                let Ok(got) = read_at(vh, run_off, &mut pbuf.as_mut()[..aligned]) else {
                    return out;
                };
                let take = got.min(want);
                if take == 0 {
                    return out;
                }
                out.extend_from_slice(&pbuf.as_mut()[..take]);
                run_off += take as u64;
                run_rem -= take;
                remaining -= take;
            }
        }
        out
    }

    pub fn read_raw_bytes_from_volume(
        letter: char,
        item: &Value,
        max_bytes: usize,
    ) -> Result<Vec<u8>, String> {
        let vh = open_vol(&letter.to_string())?;
        let reported = item["size_bytes"]
            .as_u64()
            .unwrap_or(max_bytes as u64)
            .min(max_bytes as u64) as usize;
        if reported == 0 {
            return Ok(Vec::new());
        }
        let chunk_cap = 1024 * 1024usize;
        let mut buf = PageBuf::alloc(chunk_cap)?;
        let mut out = Vec::with_capacity(reported);

        if item["source"] == "carved" {
            let offset = item["locator"]["offset_bytes"]
                .as_u64()
                .ok_or("Kazınmış veri ofseti eksik.")?;
            let mut cur_off = offset;
            let mut remaining = reported;
            while remaining > 0 {
                let aligned_off = (cur_off / 4096) * 4096;
                let skip = (cur_off - aligned_off) as usize;
                let want = remaining.min(chunk_cap - skip);
                let aligned_len = (want + skip).div_ceil(4096) * 4096;
                let got = read_at(vh.0, aligned_off, &mut buf.as_mut()[..aligned_len])?;
                if got <= skip {
                    break;
                }
                let take = (got - skip).min(want);
                if take == 0 {
                    break;
                }
                out.extend_from_slice(&buf.as_mut()[skip..skip + take]);
                cur_off += take as u64;
                remaining -= take;
            }
        } else {
            let cluster_bytes = item["locator"]["cluster_bytes"]
                .as_u64()
                .unwrap_or(4096)
                .max(4096);
            let runs_json = item["locator"]["runs"]
                .as_array()
                .ok_or("MFT küme haritası eksik.")?;
            let runs: Vec<(i64, u64)> = runs_json
                .iter()
                .filter_map(|r| {
                    let arr = r.as_array()?;
                    Some((arr.first()?.as_i64()?, arr.get(1)?.as_u64()?))
                })
                .collect();
            out = read_runs_into_vec(vh.0, &runs, cluster_bytes, reported);
        }
        Ok(out)
    }

    fn query_full_volume_bitmap(vh: HANDLE) -> Option<Vec<u8>> {
        let mut bits = Vec::new();
        let mut chunk = vec![0u8; 16 + 1024 * 1024];
        let mut start_lcn = 0i64;
        let max_bytes = 64 * 1024 * 1024usize;
        loop {
            let mut ret = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    vh,
                    FSCTL_GET_VOLUME_BITMAP,
                    (&start_lcn as *const i64).cast(),
                    8,
                    chunk.as_mut_ptr().cast(),
                    chunk.len() as u32,
                    &mut ret,
                    null_mut(),
                )
            };
            let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
            let hdr_sz = 16usize;
            if (ok == 0 && err != ERROR_MORE_DATA) || (ret as usize) <= hdr_sz {
                break;
            }
            let copied = (ret as usize).min(chunk.len()) - hdr_sz;
            if copied == 0 {
                break;
            }
            bits.extend_from_slice(&chunk[hdr_sz..hdr_sz + copied]);
            if ok != 0 || bits.len() >= max_bytes {
                break;
            }
            start_lcn = bits.len() as i64 * 8;
        }
        if bits.is_empty() {
            None
        } else {
            Some(bits)
        }
    }

    fn cluster_is_free(bitmap: Option<&[u8]>, lcn: i64) -> bool {
        let Some(bits) = bitmap else {
            return true;
        };
        if lcn < 0 {
            return false;
        }
        let idx = lcn as usize;
        let byte_i = idx / 8;
        if byte_i >= bits.len() {
            return false;
        }
        (bits[byte_i] >> (idx % 8)) & 1 == 0
    }

    fn window_has_free_clusters(bitmap: Option<&[u8]>, start_lcn: i64, count: u64) -> bool {
        let Some(bits) = bitmap else {
            return true;
        };
        if start_lcn < 0 {
            return false;
        }
        let start_byte = (start_lcn as usize) / 8;
        if start_byte >= bits.len() {
            return false;
        }
        let end_byte = ((start_lcn as usize + count as usize).div_ceil(8)).min(bits.len());
        bits[start_byte..end_byte].iter().any(|&b| b != 0xFF)
    }

    fn probe_pe_rsrc_at_volume_offset(
        vh: HANDLE,
        base_vol_off: u64,
        rsrc_off: u64,
        rsrc_sz: u64,
        info: &mut PeIntelligence,
    ) {
        if rsrc_off >= MAX_RESTORE_BYTES {
            return;
        }
        let target = base_vol_off.saturating_add(rsrc_off);
        let aligned = (target / 4096) * 4096;
        let skip = (target - aligned) as usize;
        let read_len = ((rsrc_sz as usize).clamp(4096, 128 * 1024) + skip).div_ceil(4096) * 4096;
        let Ok(mut pbuf) = PageBuf::alloc(read_len.min(256 * 1024)) else {
            return;
        };
        if let Ok(got) = read_at(vh, aligned, pbuf.as_mut()) {
            if got > skip {
                extract_pe_metadata(&pbuf.as_mut()[skip..got], info);
            }
        }
    }

    fn probe_carved_media_window(vh: HANDLE, abs_off: u64, max_len: usize) -> Option<Vec<u8>> {
        let aligned_off = (abs_off / 4096) * 4096;
        let skip = (abs_off - aligned_off) as usize;
        let aligned_len = (max_len + skip).div_ceil(4096) * 4096;
        let mut pbuf = PageBuf::alloc(aligned_len).ok()?;
        let got = read_at(vh, aligned_off, pbuf.as_mut()).ok()?;
        if got <= skip {
            return None;
        }
        let end = (skip + max_len).min(got);
        Some(pbuf.as_mut()[skip..end].to_vec())
    }

    fn candidate_priority(name: &str, full_path: &str, size: u64) -> i32 {
        let ext = extension_of(name);
        let lower_path = full_path.to_ascii_lowercase();
        let mut score = if is_env_filename(name) {
            115
        } else {
            match ext.as_str() {
                "exe" | "msi" | "msix" | "apk" => 120,
                "pdf" | "docx" | "xlsx" | "pptx" | "doc" | "xls" => 80,
                "jpg" | "jpeg" | "png" | "mp4" | "mkv" | "mov" | "zip" | "rar" | "7z"
                | "sqlite" | "sqlite3" | "db" | "duckdb" | "sql" => 75,
                "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "json" | "toml" | "yaml"
                | "yml" | "prisma" | "env" => 65,
                "dll" | "sys" | "bat" | "cmd" | "ps1" => -40,
                _ => 30,
            }
        };
        if is_game_path(&lower_path) {
            score += 40;
        }
        if lower_path.contains("\\users\\")
            || lower_path.contains("\\downloads\\")
            || lower_path.contains("\\desktop\\")
            || lower_path.contains("\\documents\\")
        {
            score += 35;
        }
        if size >= 128 * 1024 {
            score += 20;
        }
        score
    }

    struct MftCandidate {
        parsed: ParsedMftDeleted,
        full_path: String,
        priority: i32,
    }

    pub type MftScanResult = (Vec<RecoveredItem>, u64, HashMap<u64, (u64, String)>);

    pub fn scan_ntfs_mft(
        core: &Core,
        letter: &str,
        apps_only: bool,
        dev_only: bool,
        is_ssd: bool,
        max_records: usize,
    ) -> Result<MftScanResult, String> {
        let vh = open_vol(letter)?;
        let mut bpb = PageBuf::alloc(4096)?;
        read_at(vh.0, 0, bpb.as_mut())?;
        let slice = &bpb.as_mut()[..512];
        if &slice[3..11] != b"NTFS    " {
            return Err("Bölüm geçerli bir NTFS başlığı taşımıyor.".into());
        }
        let bps = u64::from(u16::from_le_bytes([slice[11], slice[12]])).max(512);
        let spc = u64::from(slice[13]).max(1);
        let cluster_bytes = bps * spc;
        let mft_lcn = u64::from_le_bytes(slice[48..56].try_into().map_err(|_| "BPB hatası")?);
        let mft_start_byte = mft_lcn.saturating_mul(cluster_bytes);

        let mut rec0 = PageBuf::alloc(4096)?;
        let mft_runs = match read_at(vh.0, mft_start_byte, rec0.as_mut()) {
            Ok(n) if n >= 1024 => extract_mft_runs(&rec0.as_mut()[..1024], cluster_bytes)
                .unwrap_or_else(|| {
                    vec![(
                        mft_lcn as i64,
                        (max_records as u64 * 1024).div_ceil(cluster_bytes),
                    )]
                }),
            _ => vec![(
                mft_lcn as i64,
                (max_records as u64 * 1024).div_ceil(cluster_bytes),
            )],
        };

        let bitmap = query_full_volume_bitmap(vh.0);
        let chunk_bytes = 1024 * 1024usize;
        let mut buf = PageBuf::alloc(chunk_bytes)?;
        let mut dirs: HashMap<u64, (u64, String)> = HashMap::new();
        let mut raw_deleted: Vec<ParsedMftDeleted> = Vec::new();
        let mut scanned = 0u64;
        let mut total_read = 0u64;
        let mut last_ui = Instant::now();

        'extents: for (lcn, cluster_cnt) in mft_runs {
            if lcn < 0 || cluster_cnt == 0 {
                continue;
            }
            let run_bytes = cluster_cnt.saturating_mul(cluster_bytes);
            let mut off_in_run = 0u64;
            while off_in_run < run_bytes && (scanned as usize) < max_records {
                if cancelled(core).is_err() {
                    break 'extents;
                }
                let rem = (run_bytes - off_in_run) as usize;
                let wanted = rem.min(chunk_bytes);
                let aligned_wanted = wanted.div_ceil(4096) * 4096;
                let disk_off = (lcn as u64) * cluster_bytes + off_in_run;
                let Ok(got) = read_at(vh.0, disk_off, &mut buf.as_mut()[..aligned_wanted]) else {
                    break;
                };
                let valid = got.min(wanted);
                if valid < 1024 {
                    break;
                }
                total_read = total_read.saturating_add(valid as u64);
                let recs = valid / 1024;
                for r in 0..recs {
                    let rec_idx = scanned + r as u64;
                    let rec = &buf.as_mut()[r * 1024..(r + 1) * 1024];
                    if let Some(entry) = parse_mft_entry(rec, rec_idx, cluster_bytes) {
                        match entry {
                            MftEntry::Directory {
                                record_index,
                                parent_record,
                                name,
                            } => {
                                if dirs.len() < MAX_MFT_DIRS {
                                    dirs.insert(record_index, (parent_record, name));
                                }
                            }
                            MftEntry::DeletedFile(f) => {
                                if raw_deleted.len() < MAX_MFT_COLLECT * 3 {
                                    raw_deleted.push(f);
                                }
                            }
                        }
                    }
                }
                scanned += recs as u64;
                off_in_run += valid as u64;
                if last_ui.elapsed().as_millis() >= 200 {
                    last_ui = Instant::now();
                    let mut job = lock(&core.job);
                    job["read_bytes"] = json!(total_read);
                    job["progress"] =
                        json!(15 + ((scanned * 25) / max_records.max(1) as u64).min(25));
                    job["phase"] = json!(format!(
                        "2/5 · $MFT taranıyor ({} kayıt · {} silinen aday)",
                        scanned,
                        raw_deleted.len()
                    ));
                }
            }
        }

        let mut candidates: Vec<MftCandidate> = raw_deleted
            .into_iter()
            .filter_map(|parsed| {
                let full_path =
                    reconstruct_mft_path(letter, parsed.parent_record, &parsed.name, &dirs);
                let ext = extension_of(&parsed.name);
                if is_junk_noise_path(&full_path, &ext) {
                    return None;
                }
                if apps_only && !matches!(ext.as_str(), "exe" | "msi" | "msix") {
                    return None;
                }
                if dev_only && !is_dev_project_file(&full_path, &ext) {
                    return None;
                }
                let priority = candidate_priority(&parsed.name, &full_path, parsed.real_size);
                Some(MftCandidate {
                    parsed,
                    full_path,
                    priority,
                })
            })
            .collect();

        candidates.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| b.parsed.deleted_at.cmp(&a.parsed.deleted_at))
        });
        candidates.truncate(MAX_MFT_PROBE);

        let mut probe = PageBuf::alloc(64 * 1024)?;
        let mut out = Vec::with_capacity(candidates.len());
        let mut media_probes = 0usize;
        for cand in candidates {
            if cancelled(core).is_err() {
                break;
            }
            let parsed = cand.parsed;
            let ext = extension_of(&parsed.name);
            let (src_kind, sample_slice, locator, is_free, intact_pct, mut pe) =
                if let Some(res_bytes) = &parsed.resident_bytes {
                    (
                        "mft_resident",
                        res_bytes.clone(),
                        json!({"resident_hex": encode_hex(res_bytes)}),
                        true,
                        100u32,
                        analyze_pe_binary(res_bytes),
                    )
                } else if let Some((lcn0, _)) = parsed.runs.first().copied() {
                    let mut checked = 0u64;
                    let mut free_c = 0u64;
                    for &(rlcn, rcnt) in &parsed.runs {
                        for off in 0..rcnt.min(64) {
                            if checked >= 64 {
                                break;
                            }
                            checked += 1;
                            if cluster_is_free(bitmap.as_deref(), rlcn + off as i64) {
                                free_c += 1;
                            }
                        }
                        if checked >= 64 {
                            break;
                        }
                    }
                    let ratio = (free_c * 100)
                        .checked_div(checked)
                        .map_or(100, |r| r as u32);
                    let free_ok = ratio >= 75;
                    let first_vol_off = (lcn0 as u64) * cluster_bytes;
                    let got_probe = read_at(vh.0, first_vol_off, probe.as_mut()).unwrap_or(0);
                    total_read = total_read.saturating_add(got_probe as u64);
                    let mut pe_info = analyze_pe_binary(&probe.as_mut()[..got_probe]);
                    if let Some(ref mut p_info) = pe_info {
                        if let Some((rsrc_off, rsrc_sz)) = p_info.rsrc_offset {
                            if rsrc_off >= got_probe as u64 {
                                probe_pe_rsrc_at_volume_offset(
                                    vh.0,
                                    first_vol_off,
                                    rsrc_off,
                                    rsrc_sz,
                                    p_info,
                                );
                            }
                        }
                    }
                    (
                        "mft_nonresident",
                        probe.as_mut()[..got_probe.min(8192)].to_vec(),
                        json!({"cluster_bytes": cluster_bytes, "runs": parsed.runs}),
                        free_ok,
                        ratio,
                        pe_info,
                    )
                } else {
                    continue;
                };

            let (_, mut conf, trim_zeroed, mut ai_summary) =
                classify_buffer_ai(&sample_slice, &ext, src_kind, is_free, is_ssd);
            if src_kind == "mft_nonresident" && intact_pct < 75 && !trim_zeroed {
                conf = conf.min(40 + intact_pct / 3);
                ai_summary = format!(
                    "Küme bütünlüğü %{intact_pct}; dosyanın bir bölümü yeni verilerle çakışmış olabilir."
                );
            }
            let (refined_cat, mut sub_type) =
                category_for_file(&ext, &cand.full_path, &sample_slice, pe.as_ref());
            let cat = refined_cat.to_owned();
            if let Some(ref prod) = pe.as_ref().and_then(|p| p.product_name.clone()) {
                ai_summary = format!("{sub_type} · Ürün: {prod}. {ai_summary}");
            }

            // Media inspect for MFT-recovered photos & videos
            let (media_status, openable, repair_note) = if trim_zeroed {
                if media_repair::is_supported_media_ext(&ext) {
                    (
                        Some("unrecoverable".into()),
                        false,
                        Some("Sektörler SSD TRIM tarafından sıfırlandı; açılamaz.".into()),
                    )
                } else {
                    (None, false, None)
                }
            } else if media_repair::is_supported_media_ext(&ext) {
                let media_buf = if src_kind == "mft_nonresident"
                    && parsed.real_size > sample_slice.len() as u64
                    && media_probes < 120
                {
                    media_probes += 1;
                    let cap = (parsed.real_size as usize).min(2 * 1024 * 1024);
                    read_runs_into_vec(vh.0, &parsed.runs, cluster_bytes, cap)
                } else {
                    sample_slice.clone()
                };
                let (st, op, note, dims) =
                    inspect_media_bytes(&ext, &media_buf, parsed.real_size, false);
                if let Some((w, h)) = dims {
                    if !sub_type.contains('×') {
                        sub_type = format!("{sub_type} ({w}×{h})");
                    }
                }
                if !op {
                    conf = conf.min(10);
                }
                if let Some(ref n) = note {
                    ai_summary = format!("{n} {ai_summary}");
                }
                (st, op, note)
            } else {
                (None, true, None)
            };

            let status_label = if trim_zeroed {
                "TRIM Sıfırlanmış".into()
            } else {
                match media_status.as_deref() {
                    Some("repaired") => "Otomatik Onarılacak ($MFT)".into(),
                    Some("thumbnail_rescued") => "Önizleme Kurtarılabilir ($MFT)".into(),
                    Some("unrecoverable") => "Bozuk Medya (Açılamaz)".into(),
                    _ if conf >= 80 => "Yüksek Bütünlük ($MFT)".into(),
                    _ => "Kısmi Riskli ($MFT)".into(),
                }
            };

            out.push(RecoveredItem {
                id: format!("mft-{}", parsed.record_index),
                name: parsed.name,
                original_path: cand.full_path,
                extension: ext,
                category: cat,
                sub_type,
                product_name: pe.as_mut().and_then(|p| p.product_name.take()),
                publisher: pe.as_mut().and_then(|p| p.company_name.take()),
                size_bytes: parsed.real_size,
                deleted_at: parsed.deleted_at,
                source: src_kind.into(),
                confidence: conf,
                status_label,
                entropy: shannon_entropy(&sample_slice),
                trim_zeroed,
                ai_summary,
                source_letter: letter.to_owned(),
                locator,
                media_status,
                openable,
                repair_note,
            });
        }
        lock(&core.job)["read_bytes"] = json!(total_read);
        Ok((out, scanned, dirs))
    }

    pub fn scan_signature_carving(
        core: &Core,
        letter: &str,
        budget_bytes: u64,
        apps_only: bool,
        is_ssd: bool,
        dirs: &HashMap<u64, (u64, String)>,
    ) -> Result<(Vec<RecoveredItem>, usize), String> {
        let vh = open_vol(letter)?;
        let root_w = wide(&format!("{letter}:\\"));
        let (mut spc, mut bps, mut fc, mut tc) = (0u32, 0u32, 0u32, 0u32);
        let ok =
            unsafe { GetDiskFreeSpaceW(root_w.as_ptr(), &mut spc, &mut bps, &mut fc, &mut tc) };
        let (cluster_bytes, total_clusters) = if ok != 0 && spc > 0 && bps > 0 && tc > 0 {
            (u64::from(spc) * u64::from(bps), u64::from(tc))
        } else {
            (4096, (budget_bytes / 4096).max(1))
        };
        let free_bytes_total = u64::from(fc).saturating_mul(cluster_bytes).max(1);
        let target_scan_bytes = budget_bytes.min(free_bytes_total).max(4096);

        let bitmap = query_full_volume_bitmap(vh.0);
        let chunk_size = 2 * 1024 * 1024usize;
        let clusters_per_chunk = (chunk_size as u64 / cluster_bytes).max(1);
        let mut buf = PageBuf::alloc(chunk_size)?;
        let mut out = Vec::new();
        let mut sys_count = 0usize;
        let mut exe_count = 0usize;
        let mut indx_ghosts = 0usize;
        let mut media_extra_probes = 0usize;
        let mut seen_ghosts: HashSet<String> = HashSet::new();
        let mut cur_lcn = 0i64;
        let mut scanned_free_bytes = 0u64;
        let mut total_read = lock(&core.job)["read_bytes"].as_u64().unwrap_or(0);
        let mut skip_until_vol_off = 0u64;
        let started_at = Instant::now();
        let mut last_ui = Instant::now();

        while (cur_lcn as u64) < total_clusters
            && scanned_free_bytes < target_scan_bytes
            && out.len() < MAX_CARVED_ITEMS
        {
            if cancelled(core).is_err() {
                break;
            }
            if !window_has_free_clusters(bitmap.as_deref(), cur_lcn, clusters_per_chunk) {
                cur_lcn += clusters_per_chunk as i64;
                continue;
            }
            let vol_off = (cur_lcn as u64) * cluster_bytes;
            if vol_off + (chunk_size as u64) <= skip_until_vol_off {
                cur_lcn += clusters_per_chunk as i64;
                continue;
            }
            let Ok(got) = read_at(vh.0, vol_off, buf.as_mut()) else {
                cur_lcn += clusters_per_chunk as i64;
                continue;
            };
            if got < 4096 {
                break;
            }
            total_read = total_read.saturating_add(got as u64);
            let slice = &buf.as_mut()[..got];
            let step = cluster_bytes.max(4096) as usize;
            let mut offset = 0usize;

            while offset + 4096 <= slice.len()
                && out.len() < MAX_CARVED_ITEMS
                && scanned_free_bytes < target_scan_bytes
            {
                let c_lcn = cur_lcn + ((offset as u64) / cluster_bytes) as i64;
                if !cluster_is_free(bitmap.as_deref(), c_lcn) {
                    offset += step;
                    continue;
                }
                scanned_free_bytes = scanned_free_bytes.saturating_add(step as u64);
                let abs_off = vol_off + offset as u64;
                if abs_off < skip_until_vol_off {
                    offset += step;
                    continue;
                }

                // Quick two-byte precheck before deep structural inspection.
                let b0 = slice[offset];
                let b1 = slice[offset + 1];
                if b0 == b'I'
                    && b1 == b'N'
                    && &slice[offset..offset + 4] == b"INDX"
                    && indx_ghosts < 40
                {
                    for ghost in forensics::parse_indx_slack(&slice[offset..offset + 4096]) {
                        if indx_ghosts >= 40 || out.len() >= MAX_CARVED_ITEMS {
                            break;
                        }
                        let ext = extension_of(&ghost.file_name);
                        let full_path =
                            reconstruct_mft_path(letter, ghost.parent_ref, &ghost.file_name, dirs);
                        if is_junk_noise_path(&full_path, &ext) {
                            continue;
                        }
                        let (cat, sub_type) = category_for_file(&ext, &full_path, &[], None);
                        if cat == "system_lib" || cat == "other" {
                            continue;
                        }
                        if apps_only && !matches!(cat, "exe_setup" | "game_portable") {
                            continue;
                        }
                        if seen_ghosts.insert(full_path.to_ascii_lowercase()) {
                            indx_ghosts += 1;
                            out.push(RecoveredItem {
                                id: format!("indx-{abs_off}-{indx_ghosts}"),
                                name: ghost.file_name,
                                original_path: full_path,
                                extension: ext,
                                category: cat.to_owned(),
                                sub_type,
                                product_name: None,
                                publisher: None,
                                size_bytes: ghost.real_size,
                                deleted_at: ghost.modified_at,
                                source: "indx_slack".into(),
                                confidence: 40,
                                status_label: "$I30 Dizin Slack Hayalet Kaydı".into(),
                                entropy: 0.0,
                                trim_zeroed: false,
                                ai_summary: "NTFS $I30 (INDX) dizin tamponunun slack alanında eski kayıt tespit edildi (NTFS $I30 analizi).".into(),
                                source_letter: letter.to_owned(),
                                locator: json!({"metadata_only": true, "offset_bytes": abs_off}),
                                media_status: None,
                                openable: true,
                                repair_note: None,
                            });
                        }
                    }
                    offset += step;
                    continue;
                }
                let maybe_sig = matches!(
                    (b0, b1),
                    (b'M', b'Z')
                        | (0xD0, 0xCF)
                        | (b'P', b'K')
                        | (b'7', b'z')
                        | (b'R', b'a')
                        | (b'%', b'P')
                        | (0x89, b'P')
                        | (0xFF, 0xD8)
                        | (b'G', b'I')
                        | (b'B', b'M')
                        | (b'R', b'I')
                        | (b'S', b'Q')
                ) || (offset + 8 <= slice.len()
                    && &slice[offset + 4..offset + 8] == b"ftyp");

                if maybe_sig {
                    if let Some(mut sig) = detect_carved_signature(&slice[offset..]) {
                        if let Some((rsrc_off, rsrc_sz)) = sig.rsrc_offset {
                            if sig.product_name.is_none() || sig.original_filename_missing() {
                                if let Some(mut pe) = analyze_pe_binary(
                                    &slice[offset..slice.len().min(offset + 65536)],
                                ) {
                                    if rsrc_off >= (slice.len() - offset) as u64 {
                                        probe_pe_rsrc_at_volume_offset(
                                            vh.0, abs_off, rsrc_off, rsrc_sz, &mut pe,
                                        );
                                        if let Some(orig) = pe.original_filename.clone() {
                                            let clean = sanitize_filename(&orig);
                                            if clean.len() >= 4 {
                                                sig.suggested_name = Some(clean);
                                            }
                                        } else if let Some(prod) = pe.product_name.clone() {
                                            let clean = sanitize_filename(&prod);
                                            if clean.len() >= 3 {
                                                sig.suggested_name =
                                                    Some(format!("{clean}.{}", sig.ext));
                                            }
                                        }
                                        sig.product_name = pe.product_name;
                                        sig.publisher = pe.company_name;
                                        if let Some(inst) = pe.installer_kind {
                                            sig.sub_type = format!(
                                                "{} {inst}",
                                                if pe.is_64bit { "64-bit" } else { "32-bit" }
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        if apps_only && !matches!(sig.category, "exe_setup" | "game_portable") {
                            offset += step;
                            continue;
                        }
                        if sig.category == "system_lib" {
                            if sys_count >= MAX_SYSTEM_CARVED {
                                offset += step;
                                continue;
                            }
                            sys_count += 1;
                        }
                        if matches!(sig.category, "exe_setup" | "game_portable") {
                            exe_count += 1;
                        }

                        // For carved photos/videos near the tail of the 2 MiB window, read a clean
                        // 4 MiB window directly from volume so true end & moov/IEND/EOI aren't missed!
                        let avail_in_chunk = slice.len() - offset;
                        let mut media_owned: Option<Vec<u8>> = None;
                        if media_repair::is_supported_media_ext(sig.ext)
                            && (sig.estimated_size as usize >= avail_in_chunk
                                || matches!(sig.ext, "mp4" | "mov" | "jpg"))
                            && media_extra_probes < 100
                        {
                            let want_probe = (sig.estimated_size as usize)
                                .clamp(avail_in_chunk, 4 * 1024 * 1024);
                            if want_probe > avail_in_chunk {
                                media_extra_probes += 1;
                                if let Some(extra) =
                                    probe_carved_media_window(vh.0, abs_off, want_probe)
                                {
                                    // Re-run accurate boundary detection on the full probed window
                                    let refined = match sig.ext {
                                        "jpg" => media_repair::find_jpeg_end(&extra),
                                        "png" => media_repair::find_png_end(&extra),
                                        "mp4" | "mov" => media_repair::find_mp4_size(&extra),
                                        "webp" | "avi" | "wav" => {
                                            media_repair::find_riff_size(&extra)
                                        }
                                        _ => None,
                                    };
                                    if let Some(exact_sz) = refined {
                                        sig.estimated_size =
                                            (exact_sz as u64).clamp(4096, MAX_RESTORE_BYTES);
                                    }
                                    media_owned = Some(extra);
                                }
                            }
                        }

                        let idx = out.len() + 1;
                        let sample_end = (offset + 8192).min(slice.len());
                        let sample_bytes = &slice[offset..sample_end];
                        let name = sig.suggested_name.clone().unwrap_or_else(|| {
                            semantic_name_from_bytes(sample_bytes, sig.ext, idx)
                        });
                        let (_, mut conf, trim_zeroed, mut ai_summary) =
                            classify_buffer_ai(sample_bytes, sig.ext, "carved", true, is_ssd);
                        if let Some(ref pub_name) = sig.publisher {
                            ai_summary =
                                format!("{} · Yayıncı: {pub_name}. {ai_summary}", sig.sub_type);
                        } else {
                            ai_summary = format!("{}. {ai_summary}", sig.sub_type);
                        }

                        let (media_status, openable, repair_note) =
                            if media_repair::is_supported_media_ext(sig.ext) {
                                let inspect_slice = if let Some(ref full_buf) = media_owned {
                                    let end = (sig.estimated_size as usize).min(full_buf.len());
                                    &full_buf[..end]
                                } else {
                                    let end =
                                        (offset + sig.estimated_size as usize).min(slice.len());
                                    &slice[offset..end]
                                };
                                let (st, op, note, dims) = inspect_media_bytes(
                                    sig.ext,
                                    inspect_slice,
                                    sig.estimated_size,
                                    false,
                                );
                                if let Some((w, h)) = dims {
                                    if !sig.sub_type.contains('×') {
                                        sig.sub_type = format!("{} ({w}×{h})", sig.sub_type);
                                    }
                                }
                                if !op {
                                    conf = conf.min(10);
                                }
                                if let Some(ref n) = note {
                                    ai_summary = format!("{n} {ai_summary}");
                                }
                                (st, op, note)
                            } else {
                                (None, true, None)
                            };

                        let status_label = match media_status.as_deref() {
                            Some("intact") => "Derin Kazıma · Sağlam Medya".into(),
                            Some("repaired") => "Otomatik Onarılacak (Derin Kazıma)".into(),
                            Some("thumbnail_rescued") => "Önizleme Kurtarılabilir (EXIF)".into(),
                            Some("unrecoverable") => "Bozuk Medya (Açılamaz)".into(),
                            _ => "Derin İmza Kazıma (Yapı Doğrulandı)".into(),
                        };

                        let aligned_sz = sig.estimated_size.div_ceil(4096) * 4096;
                        skip_until_vol_off = abs_off.saturating_add(aligned_sz);

                        out.push(RecoveredItem {
                            id: format!("carv-{abs_off}"),
                            name: name.clone(),
                            original_path: format!("{letter}:\\{name}"),
                            extension: sig.ext.into(),
                            category: sig.category.into(),
                            sub_type: sig.sub_type,
                            product_name: sig.product_name,
                            publisher: sig.publisher,
                            size_bytes: sig.estimated_size,
                            deleted_at: None,
                            source: "carved".into(),
                            confidence: conf,
                            status_label,
                            entropy: shannon_entropy(sample_bytes),
                            trim_zeroed,
                            ai_summary,
                            source_letter: letter.to_owned(),
                            locator: json!({"offset_bytes": abs_off, "size_bytes": sig.estimated_size}),
                            media_status,
                            openable,
                            repair_note,
                        });
                        offset += aligned_sz.min(slice.len() as u64) as usize;
                        continue;
                    }
                }
                offset += step;
            }

            if last_ui.elapsed().as_millis() >= 200 {
                last_ui = Instant::now();
                let secs = started_at.elapsed().as_secs_f64().max(0.1);
                let mb_s = (total_read as f64 / 1_000_000.0) / secs;
                let scanned_gib = scanned_free_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                let target_gib = target_scan_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                let mut job = lock(&core.job);
                job["read_bytes"] = json!(total_read);
                job["progress"] =
                    json!(45 + ((scanned_free_bytes * 38) / target_scan_bytes).min(38));
                job["phase"] = json!(format!(
                    "3/5 · Derin kazıma: {scanned_gib:.2} / {target_gib:.2} GiB · {mb_s:.0} MB/sn · {exe_count} EXE · {indx_ghosts} $I30 · {} dosya",
                    out.len()
                ));
            }
            cur_lcn += clusters_per_chunk as i64;
        }
        lock(&core.job)["read_bytes"] = json!(total_read);
        Ok((out, indx_ghosts))
    }

    /// Scans MuiCache & Program Compatibility Assistant across ALL local drive letters.
    pub fn scan_deleted_app_registry() -> Vec<(String, String)> {
        let mut results = Vec::new();
        let mut seen = HashSet::new();
        let subkeys = [
            "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\Shell\\MuiCache",
            "Software\\Microsoft\\Windows NT\\CurrentVersion\\AppCompatFlags\\Compatibility Assistant\\Store",
        ];
        for sub_str in subkeys {
            let sub = wide(sub_str);
            let mut hkey: HKEY = null_mut();
            let rc =
                unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut hkey) };
            if rc != 0 {
                continue;
            }
            let mut idx = 0u32;
            loop {
                let mut name_buf = [0u16; 512];
                let mut name_len = name_buf.len() as u32;
                let mut data_buf = [0u8; 512];
                let mut data_len = data_buf.len() as u32;
                let mut val_type = 0u32;
                let rc_enum = unsafe {
                    RegEnumValueW(
                        hkey,
                        idx,
                        name_buf.as_mut_ptr(),
                        &mut name_len,
                        null_mut(),
                        &mut val_type,
                        data_buf.as_mut_ptr(),
                        &mut data_len,
                    )
                };
                if rc_enum != 0 {
                    break;
                }
                idx += 1;
                let val_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
                if forensics::extract_drive_letter(&val_name).is_none() {
                    continue;
                }
                let exe_path = val_name
                    .split(".FriendlyAppName")
                    .next()
                    .unwrap_or(&val_name)
                    .split(".ApplicationCompany")
                    .next()
                    .unwrap_or(&val_name)
                    .to_owned();
                if !exe_path.to_ascii_lowercase().ends_with(".exe") {
                    continue;
                }
                if !Path::new(&exe_path).exists() && seen.insert(exe_path.to_ascii_lowercase()) {
                    let friendly = if val_type == 1 && !val_name.contains(".ApplicationCompany") {
                        decode_utf16le(&data_buf[..data_len as usize])
                    } else {
                        String::new()
                    };
                    results.push((exe_path, friendly));
                    if results.len() >= MAX_APPS * 2 {
                        break;
                    }
                }
            }
            unsafe {
                RegCloseKey(hkey);
            }
            if results.len() >= MAX_APPS * 2 {
                break;
            }
        }
        results
    }

    pub fn restore_raw_from_volume(
        letter: char,
        item: &Value,
        tmp_path: &Path,
        hasher: &mut Sha256,
    ) -> Result<u64, String> {
        let vh = open_vol(&letter.to_string())?;
        let mut wf = File::create(tmp_path).map_err(|e| e.to_string())?;
        let mut remaining = item["size_bytes"]
            .as_u64()
            .unwrap_or(0)
            .min(MAX_RESTORE_BYTES);
        let mut written = 0u64;
        let chunk_cap = 4 * 1024 * 1024usize;
        let mut buf = PageBuf::alloc(chunk_cap)?;

        if item["source"] == "carved" {
            let offset = item["locator"]["offset_bytes"]
                .as_u64()
                .ok_or("Kazınmış veri ofseti eksik.")?;
            let mut cur_off = offset;
            while remaining > 0 {
                let aligned_off = (cur_off / 4096) * 4096;
                let skip = (cur_off - aligned_off) as usize;
                let want_bytes = (remaining as usize).min(chunk_cap - skip);
                let aligned_len = (want_bytes + skip).div_ceil(4096) * 4096;
                let got = read_at(vh.0, aligned_off, &mut buf.as_mut()[..aligned_len])?;
                if got <= skip {
                    break;
                }
                let take = (got - skip).min(want_bytes);
                if take == 0 {
                    break;
                }
                let slice = &buf.as_mut()[skip..skip + take];
                wf.write_all(slice).map_err(|e| e.to_string())?;
                hasher.update(slice);
                written += slice.len() as u64;
                cur_off += slice.len() as u64;
                remaining = remaining.saturating_sub(slice.len() as u64);
            }
        } else {
            let cluster_bytes = item["locator"]["cluster_bytes"]
                .as_u64()
                .unwrap_or(4096)
                .max(4096);
            let runs = item["locator"]["runs"]
                .as_array()
                .ok_or("MFT küme haritası eksik.")?;
            for run in runs {
                if remaining == 0 {
                    break;
                }
                let arr = run.as_array().ok_or("Geçersiz küme kaydı.")?;
                let lcn = arr[0].as_i64().unwrap_or(-1);
                let cnt = arr[1].as_u64().unwrap_or(0);
                if lcn < 0 || cnt == 0 {
                    continue;
                }
                let mut run_rem = cnt.saturating_mul(cluster_bytes).min(remaining);
                let mut run_off = (lcn as u64) * cluster_bytes;
                while run_rem > 0 {
                    let want = (run_rem as usize).min(chunk_cap);
                    let aligned_len = want.div_ceil(4096) * 4096;
                    let got = read_at(vh.0, run_off, &mut buf.as_mut()[..aligned_len])?;
                    let take = got.min(want);
                    if take == 0 {
                        break;
                    }
                    let slice = &buf.as_mut()[..take];
                    wf.write_all(slice).map_err(|e| e.to_string())?;
                    hasher.update(slice);
                    written += slice.len() as u64;
                    run_off += slice.len() as u64;
                    run_rem = run_rem.saturating_sub(slice.len() as u64);
                    remaining = remaining.saturating_sub(slice.len() as u64);
                }
            }
        }
        wf.sync_all().map_err(|e| e.to_string())?;
        Ok(written)
    }
}

impl CarvedSignature {
    #[cfg(windows)]
    fn original_filename_missing(&self) -> bool {
        self.suggested_name.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_version_kv(out: &mut Vec<u8>, key: &str, val: &str) {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        let key_u16: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
        let val_u16: Vec<u16> = val.encode_utf16().chain(Some(0)).collect();
        let start = out.len();
        out.extend_from_slice(&0u16.to_le_bytes()); // wLength placeholder
        out.extend_from_slice(&(val_u16.len() as u16).to_le_bytes()); // wValueLength
        out.extend_from_slice(&1u16.to_le_bytes()); // wType = 1 (text)
        for u in key_u16 {
            out.extend_from_slice(&u.to_le_bytes());
        }
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        for u in val_u16 {
            out.extend_from_slice(&u.to_le_bytes());
        }
        let len = (out.len() - start) as u16;
        out[start..start + 2].copy_from_slice(&len.to_le_bytes());
    }

    fn build_test_pe(is_dll: bool, nsis: bool) -> Vec<u8> {
        let mut pe = vec![0u8; 8192];
        pe[0..2].copy_from_slice(b"MZ");
        pe[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        pe[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes()); // x64
        pe[coff + 2..coff + 4].copy_from_slice(&2u16.to_le_bytes()); // 2 sections
        pe[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes()); // PE32+ opt header
        let chars: u16 = if is_dll { 0x2002 } else { 0x0002 };
        pe[coff + 18..coff + 20].copy_from_slice(&chars.to_le_bytes());
        let opt = coff + 20;
        pe[opt..opt + 2].copy_from_slice(&0x020Bu16.to_le_bytes()); // PE32+
        pe[opt + 36..opt + 40].copy_from_slice(&512u32.to_le_bytes()); // FileAlignment
        pe[opt + 68..opt + 70].copy_from_slice(&2u16.to_le_bytes()); // GUI subsystem

        let sec_tbl = opt + 240;
        pe[sec_tbl..sec_tbl + 5].copy_from_slice(b".text");
        pe[sec_tbl + 16..sec_tbl + 20].copy_from_slice(&1024u32.to_le_bytes());
        pe[sec_tbl + 20..sec_tbl + 24].copy_from_slice(&1024u32.to_le_bytes());

        let s2 = sec_tbl + 40;
        pe[s2..s2 + 5].copy_from_slice(b".rsrc");
        pe[s2 + 16..s2 + 20].copy_from_slice(&2048u32.to_le_bytes());
        pe[s2 + 20..s2 + 24].copy_from_slice(&2048u32.to_le_bytes());

        let mut rsrc = Vec::new();
        push_version_kv(&mut rsrc, "OriginalFilename", "RufusSetup.exe");
        push_version_kv(&mut rsrc, "ProductName", "Rufus Kurulum");
        push_version_kv(&mut rsrc, "CompanyName", "Akeo Consulting");
        pe[2048..2048 + rsrc.len()].copy_from_slice(&rsrc);

        if nsis {
            // Valid NSIS FirstHeader at overlay 4096: flags(4) + magic(16) + header_len(4) + archive_len(4)
            let ov = 4096;
            pe[ov + 4..ov + 20].copy_from_slice(b"\xEF\xBE\xAD\xDENullsoftInst");
            pe[ov + 24..ov + 28].copy_from_slice(&1_500_000u32.to_le_bytes());
        }
        pe
    }

    fn build_minimal_png() -> Vec<u8> {
        // Valid 1x1 PNG (IHDR + IDAT + IEND)
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1A\n");
        // IHDR
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
        png.extend_from_slice(&[0x90, 0x77, 0x53, 0xDE]);
        // IDAT (zlib minimal valid payload for 1x1 RGB)
        let idat: &[u8] = &[
            0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00,
        ];
        png.extend_from_slice(&(idat.len() as u32).to_be_bytes());
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(idat);
        png.extend_from_slice(&[0x18, 0xDD, 0x8D, 0xB4]);
        // IEND
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IEND\xAE\x42\x60\x82");
        png
    }

    fn build_truncated_jpeg() -> Vec<u8> {
        // Valid SOI + APP0 + DQT + SOF0 + DHT + SOS + scan data WITHOUT trailing FF D9
        let mut jpg = Vec::new();
        jpg.extend_from_slice(&[0xFF, 0xD8]); // SOI
        jpg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]); // APP0 len=16
        jpg.extend_from_slice(b"JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00");
        jpg.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]); // DQT len=67
        jpg.extend_from_slice(&[16u8; 64]);
        jpg.extend_from_slice(&[
            0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x08, 0x00, 0x08, 0x01, 0x01, 0x11, 0x00,
        ]); // SOF0 8x8
        jpg.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x14, 0x00]); // DHT
        jpg.extend_from_slice(&[0u8; 17]);
        jpg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]); // SOS
        jpg.extend_from_slice(&[0x55, 0xAA, 0x33, 0x77, 0x11, 0x22]); // Truncated entropy scan (missing FF D9!)
        jpg
    }

    #[test]
    fn recycle_bin_i_parser_supports_v1_and_v2_headers() {
        let path_u16: Vec<u16> = "D:\\Belgeler\\Rapor.pdf".encode_utf16().collect();
        let mut v2 = Vec::new();
        v2.extend_from_slice(&2u64.to_le_bytes());
        v2.extend_from_slice(&4096u64.to_le_bytes());
        v2.extend_from_slice(
            &(WINDOWS_EPOCH_OFFSET_100NS + 1_700_000_000 * 10_000_000).to_le_bytes(),
        );
        v2.extend_from_slice(&(path_u16.len() as u32).to_le_bytes());
        for w in &path_u16 {
            v2.extend_from_slice(&w.to_le_bytes());
        }
        v2.extend_from_slice(&0u16.to_le_bytes());

        let (sz, dt, path) = parse_recycle_i_bytes(&v2).unwrap();
        assert_eq!(sz, 4096);
        assert!(dt.unwrap().starts_with("2023-11-14"));
        assert_eq!(path, "D:\\Belgeler\\Rapor.pdf");

        let mut v1 = Vec::new();
        v1.extend_from_slice(&1u64.to_le_bytes());
        v1.extend_from_slice(&1234u64.to_le_bytes());
        v1.extend_from_slice(
            &(WINDOWS_EPOCH_OFFSET_100NS + 1_700_000_000 * 10_000_000).to_le_bytes(),
        );
        for w in &path_u16 {
            v1.extend_from_slice(&w.to_le_bytes());
        }
        assert_eq!(parse_recycle_i_bytes(&v1).unwrap().0, 1234);
    }

    #[test]
    fn deep_recycle_bin_scan_recovers_deleted_app_folders_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let sid_dir = tmp.path().join("$Recycle.Bin").join("S-1-5-21-999");
        std::fs::create_dir_all(&sid_dir).unwrap();

        // Build $I metadata pointing to a deleted game folder D:\Games\HollowKnight
        let orig_dir = "D:\\Games\\HollowKnight";
        let path_u16: Vec<u16> = orig_dir.encode_utf16().collect();
        let mut i_bytes = Vec::new();
        i_bytes.extend_from_slice(&2u64.to_le_bytes());
        i_bytes.extend_from_slice(&8192u64.to_le_bytes());
        i_bytes.extend_from_slice(
            &(WINDOWS_EPOCH_OFFSET_100NS + 1_740_000_000 * 10_000_000).to_le_bytes(),
        );
        i_bytes.extend_from_slice(&(path_u16.len() as u32).to_le_bytes());
        for w in &path_u16 {
            i_bytes.extend_from_slice(&w.to_le_bytes());
        }
        i_bytes.extend_from_slice(&0u16.to_le_bytes());
        std::fs::write(sid_dir.join("$IGAME01"), &i_bytes).unwrap();

        let r_dir = sid_dir.join("$RGAME01");
        std::fs::create_dir_all(&r_dir).unwrap();
        std::fs::write(r_dir.join("HollowKnight.exe"), build_test_pe(false, false)).unwrap();

        let items = scan_recycle_bin_dir(tmp.path(), "D", false, false, &|| false);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "HollowKnight.exe");
        assert_eq!(
            items[0].original_path,
            "D:\\Games\\HollowKnight\\HollowKnight.exe"
        );
        assert_eq!(items[0].source, "recycle_bin");
        assert_eq!(items[0].confidence, 100);
    }

    #[test]
    fn deduplication_merges_setup_exe_and_appdata_into_single_deleted_app_bundle() {
        let items = vec![RecoveredItem {
            id: "rb-spotify-setup".into(),
            name: "SpotifySetup.exe".into(),
            original_path: "C:\\Users\\Ali\\Downloads\\SpotifySetup.exe".into(),
            extension: "exe".into(),
            category: "exe_setup".into(),
            sub_type: "64-bit Masaüstü Uygulaması".into(),
            product_name: Some("Spotify".into()),
            publisher: Some("Spotify AB".into()),
            size_bytes: 1_800_000,
            deleted_at: Some("2026-05-01T12:00:00+00:00".into()),
            source: "recycle_bin".into(),
            confidence: 100,
            status_label: "Geri Dönüşüm Kutusu (%100 Sağlam)".into(),
            entropy: 6.8,
            trim_zeroed: false,
            ai_summary: "Sağlam".into(),
            source_letter: "C".into(),
            locator: json!({"r_path": "C:\\$Recycle.Bin\\$R1.exe"}),
            media_status: None,
            openable: true,
            repair_note: None,
        }];

        let traces = vec![
            ForensicAppTrace {
                exe_path: "C:\\Users\\Ali\\AppData\\Roaming\\Spotify\\Spotify.exe".into(),
                name: "Spotify".into(),
                publisher: None,
                sources: vec!["Prefetch".into(), "Güvenlik Duvarı".into()],
                run_count: Some(42),
                last_run_at: Some("2026-05-02T14:00:00+00:00".into()),
                vss_path: None,
                drive_letter: "C".into(),
                is_game: false,
                install_dir: Some("C:\\Users\\Ali\\AppData\\Roaming\\Spotify".into()),
                appdata_paths: Vec::new(),
                leftover_bytes: 0,
            },
            // Noise trace must be ruthlessly filtered out
            ForensicAppTrace {
                exe_path: "C:\\Windows\\System32\\svchost.exe".into(),
                name: "svchost".into(),
                publisher: None,
                sources: vec!["Prefetch".into()],
                run_count: Some(500),
                last_run_at: None,
                vss_path: None,
                drive_letter: "C".into(),
                is_game: false,
                install_dir: None,
                appdata_paths: Vec::new(),
                leftover_bytes: 0,
            },
            // GameConfigStore game on D:
            ForensicAppTrace {
                exe_path: "D:\\Games\\Hades\\x64\\Hades.exe".into(),
                name: "Hades".into(),
                publisher: Some("Supergiant Games".into()),
                sources: vec!["GameConfigStore".into()],
                run_count: Some(15),
                last_run_at: Some("2026-05-03T18:00:00+00:00".into()),
                vss_path: None,
                drive_letter: "D".into(),
                is_game: true,
                install_dir: Some("D:\\Games\\Hades".into()),
                appdata_paths: Vec::new(),
                leftover_bytes: 0,
            },
        ];

        let appdata = vec![AppDataEntry {
            folder_name: "Spotify".into(),
            path: "C:\\Users\\Ali\\AppData\\Roaming\\Spotify".into(),
            size_bytes: 45_000_000,
            scope: "AppData Kalıntısı".into(),
            is_installed: false,
            has_registry_key: true,
            modified_at: None,
        }];

        let bundles = build_deleted_app_bundles("D", &items, &traces, &appdata, &HashMap::new());
        assert_eq!(
            bundles.len(),
            2,
            "Expected Spotify (merged) + Hades; svchost dropped"
        );

        let spotify = bundles.iter().find(|b| b.name == "Spotify").unwrap();
        assert_eq!(spotify.recovery_status, "full_restore");
        assert_eq!(spotify.category, "Uygulama");
        assert_eq!(
            spotify.exe_path,
            "C:\\Users\\Ali\\AppData\\Roaming\\Spotify\\Spotify.exe"
        );
        assert_eq!(
            spotify.recoverable_item_id.as_deref(),
            Some("rb-spotify-setup")
        );
        assert_eq!(spotify.leftover_bytes, 45_000_000);
        assert_eq!(spotify.run_count, Some(42));
        assert_eq!(spotify.drive_letter, "C");
        assert_eq!(
            spotify.winget_hint,
            "winget install --id Spotify.Spotify -e"
        );
        assert!(spotify.sources.iter().any(|s| s == "Geri Dönüşüm Kutusu"));
        assert!(spotify.sources.iter().any(|s| s == "Prefetch"));
        assert!(spotify.sources.iter().any(|s| s == "Güvenlik Duvarı"));
        assert!(spotify.sources.iter().any(|s| s == "AppData Kalıntısı"));

        let hades = bundles.iter().find(|b| b.name == "Hades").unwrap();
        assert_eq!(hades.category, "Oyun");
        assert_eq!(hades.recovery_status, "reinstall_only");
        assert_eq!(hades.drive_letter, "D");
    }

    #[test]
    fn preview_item_handles_text_image_binary_and_unavailable_readonly() {
        let tmp = tempfile::tempdir().unwrap();
        let png_path = tmp.path().join("photo.png");
        std::fs::write(&png_path, build_minimal_png()).unwrap();

        let trunc_jpg_path = tmp.path().join("broken.jpg");
        std::fs::write(&trunc_jpg_path, build_truncated_jpeg()).unwrap();

        let big_text = "Merhaba SSD Sentinel! ".repeat(1000); // > 16 KiB
        let txt_path = tmp.path().join("notes.txt");
        std::fs::write(&txt_path, &big_text).unwrap();

        let job = json!({
            "letter": "D",
            "recovery_items": [
                {
                    "id": "img-1",
                    "name": "photo.png",
                    "extension": "png",
                    "category": "photo",
                    "size_bytes": 70,
                    "source": "recycle_bin",
                    "trim_zeroed": false,
                    "source_letter": "D",
                    "locator": { "r_path": png_path.to_string_lossy() }
                },
                {
                    "id": "img-trunc",
                    "name": "broken.jpg",
                    "extension": "jpg",
                    "category": "photo",
                    "size_bytes": 200,
                    "source": "recycle_bin",
                    "trim_zeroed": false,
                    "source_letter": "D",
                    "locator": { "r_path": trunc_jpg_path.to_string_lossy() }
                },
                {
                    "id": "txt-1",
                    "name": "notes.txt",
                    "extension": "txt",
                    "category": "document",
                    "size_bytes": big_text.len(),
                    "source": "recycle_bin",
                    "trim_zeroed": false,
                    "source_letter": "D",
                    "locator": { "r_path": txt_path.to_string_lossy() }
                },
                {
                    "id": "bin-1",
                    "name": "RufusSetup.exe",
                    "extension": "exe",
                    "category": "exe_setup",
                    "sub_type": "64-bit NSIS Kurulum Paketi",
                    "product_name": "Rufus",
                    "publisher": "Akeo Consulting",
                    "status_label": "Geri Dönüşüm Kutusu",
                    "size_bytes": 1400000,
                    "source": "recycle_bin",
                    "trim_zeroed": false,
                    "source_letter": "D",
                    "locator": { "r_path": "nonexistent.exe" }
                },
                {
                    "id": "trim-1",
                    "name": "gone.jpg",
                    "extension": "jpg",
                    "category": "photo",
                    "size_bytes": 4096,
                    "source": "carved",
                    "trim_zeroed": true,
                    "source_letter": "D",
                    "locator": { "offset_bytes": 4096 }
                },
                {
                    "id": "usn-1",
                    "name": "ghost.pdf",
                    "extension": "pdf",
                    "category": "document",
                    "size_bytes": 0,
                    "source": "usn_journal",
                    "trim_zeroed": false,
                    "source_letter": "D",
                    "locator": { "metadata_only": true }
                }
            ]
        });

        let img_res = preview_item(&job, "img-1").unwrap();
        assert_eq!(img_res["kind"], "image");
        assert_eq!(img_res["mime"], "image/png");
        assert!(img_res["data_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));

        // Truncated JPEG is actively repaired in preview_item into an openable image!
        let trunc_res = preview_item(&job, "img-trunc").unwrap();
        assert_eq!(trunc_res["kind"], "image");
        assert_eq!(trunc_res["mime"], "image/jpeg");
        assert_eq!(trunc_res["repaired"], true);

        let txt_res = preview_item(&job, "txt-1").unwrap();
        assert_eq!(txt_res["kind"], "text");
        assert_eq!(txt_res["truncated"], true);
        assert!(txt_res["text"]
            .as_str()
            .unwrap()
            .starts_with("Merhaba SSD Sentinel!"));

        let bin_res = preview_item(&job, "bin-1").unwrap();
        assert_eq!(bin_res["kind"], "binary");
        assert_eq!(bin_res["metadata"]["product_name"], "Rufus");
        assert_eq!(bin_res["metadata"]["publisher"], "Akeo Consulting");

        let trim_res = preview_item(&job, "trim-1").unwrap();
        assert_eq!(trim_res["kind"], "unavailable");

        let usn_res = preview_item(&job, "usn-1").unwrap();
        assert_eq!(usn_res["kind"], "unavailable");

        assert!(preview_item(&job, "does-not-exist").is_err());
    }

    #[test]
    fn restore_batch_enforces_anti_overwrite_collision_safety_and_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("src");
        let dst_dir = tmp.path().join("dst");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();

        let same_letter = drive_letter_of(dst_dir.as_path()).unwrap_or('C');
        let other_letter = if same_letter == 'D' { 'Z' } else { 'D' };

        // 1. Same-drive anti-overwrite refusal for mft_resident
        let blocked_job = json!({
            "letter": same_letter.to_string(),
            "recovery_items": [
                {
                    "id": "mft-1",
                    "name": "secret.txt",
                    "extension": "txt",
                    "source": "mft_resident",
                    "trim_zeroed": false,
                    "source_letter": same_letter.to_string(),
                    "locator": { "resident_hex": encode_hex(b"sensitive") }
                }
            ]
        });
        assert!(restore_batch(&blocked_job, &["mft-1".into()], &dst_dir).is_err());
        assert!(!dst_dir.join("secret.txt").exists());

        // 2. Multi-file restore with duplicate filename collision, skipped TRIM item, and auto-repaired JPEG
        let jpg_src = src_dir.join("$RJPG.jpg");
        std::fs::write(&jpg_src, build_truncated_jpeg()).unwrap();

        let ok_job = json!({
            "letter": other_letter.to_string(),
            "recovery_items": [
                {
                    "id": "mft-a",
                    "name": "notes.txt",
                    "extension": "txt",
                    "source": "mft_resident",
                    "trim_zeroed": false,
                    "source_letter": other_letter.to_string(),
                    "locator": { "resident_hex": encode_hex(b"first file") }
                },
                {
                    "id": "mft-b",
                    "name": "notes.txt",
                    "extension": "txt",
                    "source": "mft_resident",
                    "trim_zeroed": false,
                    "source_letter": other_letter.to_string(),
                    "locator": { "resident_hex": encode_hex(b"second file") }
                },
                {
                    "id": "rb-jpg",
                    "name": "holiday.jpg",
                    "extension": "jpg",
                    "source": "recycle_bin",
                    "trim_zeroed": false,
                    "source_letter": other_letter.to_string(),
                    "locator": { "r_path": jpg_src.to_string_lossy() }
                },
                {
                    "id": "trim-skip",
                    "name": "zeroed.bin",
                    "extension": "bin",
                    "source": "carved",
                    "trim_zeroed": true,
                    "source_letter": other_letter.to_string(),
                    "locator": { "offset_bytes": 4096 }
                }
            ]
        });

        let res = restore_batch(
            &ok_job,
            &[
                "mft-a".into(),
                "mft-b".into(),
                "rb-jpg".into(),
                "trim-skip".into(),
            ],
            &dst_dir,
        )
        .unwrap();
        assert_eq!(res["saved"], true);
        assert_eq!(res["restored"], 3);
        assert_eq!(res["skipped"], 1);
        assert_eq!(res["failed"], 0);
        assert_eq!(res["repaired"], true);
        assert_eq!(res["repaired_count"], 1);
        assert_eq!(
            std::fs::read(dst_dir.join("notes.txt")).unwrap(),
            b"first file"
        );
        assert_eq!(
            std::fs::read(dst_dir.join("notes_2.txt")).unwrap(),
            b"second file"
        );
        let saved_jpg = std::fs::read(dst_dir.join("holiday.jpg")).unwrap();
        assert!(
            saved_jpg.ends_with(&[0xFF, 0xD9]),
            "Repaired JPEG must end with EOI FF D9"
        );
    }

    #[test]
    fn ntfs_data_runs_and_deleted_mft_record_parse_resident_and_nonresident() {
        let run_bytes = [0x11, 0x04, 0x64, 0x11, 0x02, 0xEC, 0x00];
        let runs = decode_ntfs_data_runs(&run_bytes);
        assert_eq!(runs, vec![(100, 4), (80, 2)]);

        let mut rec = vec![0u8; 1024];
        rec[0..4].copy_from_slice(b"FILE");
        rec[4..6].copy_from_slice(&48u16.to_le_bytes());
        rec[6..8].copy_from_slice(&3u16.to_le_bytes());
        rec[20..22].copy_from_slice(&56u16.to_le_bytes());
        rec[22..24].copy_from_slice(&0u16.to_le_bytes());

        rec[48..50].copy_from_slice(&0x1234u16.to_le_bytes());
        rec[50..52].copy_from_slice(b"AB");
        rec[52..54].copy_from_slice(b"CD");
        rec[510..512].copy_from_slice(&0x1234u16.to_le_bytes());
        rec[1022..1024].copy_from_slice(&0x1234u16.to_le_bytes());

        let name_u16: Vec<u16> = "secret.txt".encode_utf16().collect();
        let fn_len = 24 + 66 + name_u16.len() * 2;
        rec[56..60].copy_from_slice(&0x30u32.to_le_bytes());
        rec[60..64].copy_from_slice(&(fn_len as u32).to_le_bytes());
        rec[56 + 20..56 + 22].copy_from_slice(&24u16.to_le_bytes());
        rec[56 + 24..56 + 32].copy_from_slice(&(420u64 | (1u64 << 48)).to_le_bytes());
        rec[56 + 24 + 64] = name_u16.len() as u8;
        rec[56 + 24 + 65] = 1;
        for (i, w) in name_u16.iter().enumerate() {
            rec[56 + 24 + 66 + i * 2..56 + 24 + 68 + i * 2].copy_from_slice(&w.to_le_bytes());
        }

        let data_off = 56 + fn_len;
        let payload = b"hello world";
        let data_len = 24 + payload.len();
        rec[data_off..data_off + 4].copy_from_slice(&0x80u32.to_le_bytes());
        rec[data_off..data_off + 8][4..8].copy_from_slice(&(data_len as u32).to_le_bytes());
        rec[data_off + 16..data_off + 20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        rec[data_off + 20..data_off + 22].copy_from_slice(&24u16.to_le_bytes());
        rec[data_off + 24..data_off + 24 + payload.len()].copy_from_slice(payload);

        let end_off = data_off + data_len;
        rec[end_off..end_off + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());

        let parsed = parse_mft_record(&rec, 42, 4096).unwrap();
        assert_eq!(parsed.name, "secret.txt");
        assert_eq!(parsed.parent_record, 420);
        assert_eq!(parsed.resident_bytes.unwrap(), payload);

        let mut dirs = HashMap::new();
        dirs.insert(420, (5u64, "Belgeler".to_owned()));
        assert_eq!(
            reconstruct_mft_path("D", parsed.parent_record, &parsed.name, &dirs),
            "D:\\Belgeler\\secret.txt"
        );
        assert!(is_junk_noise_path(
            "C:\\Users\\Demo\\AppData\\Local\\Temp\\abc.tmp",
            "tmp"
        ));
        assert!(!is_junk_noise_path("D:\\Downloads\\Rufus.exe", "exe"));
    }

    #[test]
    fn pe_intelligence_extracts_utf16_metadata_and_separates_exe_from_dll() {
        let setup_exe = build_test_pe(false, true);
        let sig = detect_carved_signature(&setup_exe).unwrap();
        assert_eq!(sig.ext, "exe");
        assert_eq!(sig.category, "exe_setup");
        assert_eq!(sig.suggested_name.as_deref(), Some("RufusSetup.exe"));
        assert_eq!(sig.product_name.as_deref(), Some("Rufus Kurulum"));
        assert_eq!(sig.publisher.as_deref(), Some("Akeo Consulting"));
        assert_eq!(sig.estimated_size, 4096 + 1_500_000);
        assert!(sig.sub_type.contains("NSIS"));

        let lib_dll = build_test_pe(true, false);
        let dll_sig = detect_carved_signature(&lib_dll).unwrap();
        assert_eq!(dll_sig.ext, "dll");
        assert_eq!(dll_sig.category, "system_lib");
        assert!(dll_sig.sub_type.contains("DLL"));
    }

    #[test]
    fn carved_formats_distinguish_docx_7z_sqlite_and_png_dimensions() {
        let mut docx = Vec::from(&b"PK\x03\x04\x14\x00\x00\x00\x08\x00"[..]);
        docx.resize(26, 0);
        docx.extend_from_slice(&17u16.to_le_bytes()); // file name len
        docx.extend_from_slice(&0u16.to_le_bytes());
        docx.extend_from_slice(b"word/document.xml");
        docx.resize(128, 0x41);
        let docx_sig = detect_carved_signature(&docx).unwrap();
        assert_eq!(docx_sig.ext, "docx");
        assert_eq!(docx_sig.category, "document");

        let mut png = vec![0u8; 128];
        png[0..8].copy_from_slice(b"\x89PNG\r\n\x1A\n");
        png[8..12].copy_from_slice(&13u32.to_be_bytes());
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&1920u32.to_be_bytes());
        png[20..24].copy_from_slice(&1080u32.to_be_bytes());
        let png_sig = detect_carved_signature(&png).unwrap();
        assert_eq!(png_sig.ext, "png");
        assert_eq!(png_sig.category, "photo");
        assert!(png_sig.sub_type.contains("1920×1080"));

        let mut sql = vec![0u8; 128];
        sql[0..16].copy_from_slice(b"SQLite format 3\0");
        sql[16..18].copy_from_slice(&4096u16.to_be_bytes());
        sql[28..32].copy_from_slice(&25u32.to_be_bytes());
        let sql_sig = detect_carved_signature(&sql).unwrap();
        assert_eq!(sql_sig.ext, "sqlite3");
        assert_eq!(sql_sig.category, "database_code");
        assert_eq!(sql_sig.estimated_size, 4096 * 25);
    }

    #[test]
    fn offline_ai_detects_trim_zeroes_and_extracts_semantic_names() {
        let zeroes = vec![0u8; 4096];
        let (_cat, conf, trim, _msg) = classify_buffer_ai(&zeroes, "exe", "carved", true, true);
        assert!(trim);
        assert_eq!(conf, 0);

        let pdf = b"%PDF-1.7\n/Title (Fatura 2026) /Producer";
        let name = semantic_name_from_bytes(pdf, "pdf", 3);
        assert_eq!(name, "Fatura_2026_003.pdf");

        let (cat, conf2, trim2, _) = classify_buffer_ai(pdf, "pdf", "carved", true, false);
        assert_eq!(cat, "document");
        assert!(!trim2);
        assert!(conf2 >= 70);
    }

    #[test]
    fn restore_item_blocks_same_drive_overwrite_and_restores_cross_drive_or_recycle() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("src");
        let dst_dir = tmp.path().join("dst");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dst_dir).unwrap();

        let same_letter = drive_letter_of(dst_dir.as_path()).unwrap_or('C');
        let other_letter = if same_letter == 'D' { 'Z' } else { 'D' };

        let mft_res_item = json!({
            "source": "mft_resident",
            "trim_zeroed": false,
            "locator": { "resident_hex": encode_hex(b"kurtarilan icerik") }
        });
        assert!(restore_item(
            &same_letter.to_string(),
            &mft_res_item,
            &dst_dir.join("out.txt")
        )
        .is_err());
        let ok = restore_item(
            &other_letter.to_string(),
            &mft_res_item,
            &dst_dir.join("out.txt"),
        )
        .unwrap();
        assert_eq!(ok["saved"], true);
        assert_eq!(
            std::fs::read(dst_dir.join("out.txt")).unwrap(),
            b"kurtarilan icerik"
        );

        let r_file = src_dir.join("$RABC.txt");
        std::fs::write(&r_file, b"cop kutusu").unwrap();
        let rb_item = json!({
            "source": "recycle_bin",
            "trim_zeroed": false,
            "locator": { "r_path": r_file.to_string_lossy() }
        });
        let rb_ok = restore_item(
            &same_letter.to_string(),
            &rb_item,
            &dst_dir.join("from_rb.txt"),
        )
        .unwrap();
        assert_eq!(rb_ok["bytes"], 10);
        assert_eq!(
            std::fs::read(dst_dir.join("from_rb.txt")).unwrap(),
            b"cop kutusu"
        );
    }

    #[test]
    fn dev_recovery_classifies_dotfiles_and_masks_secret_previews_without_touching_disk() {
        assert_eq!(
            category_for_file("", "C:\\Dev\\App\\.env", b"", None).0,
            "database_code"
        );
        assert_eq!(
            category_for_file("local", "C:\\Dev\\App\\.env.local", b"", None).0,
            "database_code"
        );
        assert_eq!(
            category_for_file("", "C:\\Dev\\App\\Dockerfile", b"", None).0,
            "database_code"
        );
        assert_eq!(
            category_for_file("tsx", "C:\\Dev\\App\\src\\App.tsx", b"", None).0,
            "database_code"
        );
        assert_eq!(
            category_for_file("prisma", "C:\\Dev\\App\\prisma\\schema.prisma", b"", None).0,
            "database_code"
        );
        assert_eq!(
            category_for_file("duckdb", "C:\\Dev\\App\\dev.duckdb", b"", None).0,
            "database_code"
        );
        assert!(is_dev_project_file("C:\\Dev\\App\\.env", ""));
        assert!(is_dev_project_file("C:\\Dev\\App\\.gitignore", "gitignore"));
        assert!(!is_dev_project_file("C:\\Photos\\vacation.jpg", "jpg"));

        let env_raw = "NODE_ENV=development\nPORT=3000\nOPENAI_API_KEY=sk-proj-12345\nDATABASE_URL=postgres://user:pass@localhost/db\n";
        let (masked_env, count_env) = mask_sensitive_lines(env_raw, true);
        assert_eq!(count_env, 2);
        assert!(masked_env.contains("NODE_ENV=development"));
        assert!(masked_env.contains("PORT=3000"));
        assert!(masked_env.contains("OPENAI_API_KEY=••••••••"));
        assert!(masked_env.contains("DATABASE_URL=••••••••"));
        assert!(!masked_env.contains("sk-proj-12345"));

        let json_raw = "{\n  \"port\": 8080,\n  \"api_key\": \"top-secret\",\n  \"pem\": \"-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\"\n}\n";
        let (masked_json, count_json) = mask_sensitive_lines(json_raw, false);
        assert!(count_json >= 1);
        assert!(masked_json.contains("\"port\": 8080"));
        assert!(!masked_json.contains("top-secret"));

        let tmp = tempfile::tempdir().unwrap();
        let env_file = tmp.path().join(".env.local");
        std::fs::write(&env_file, env_raw).unwrap();
        let job = json!({
            "letter": "D",
            "recovery_items": [{
                "id": "dev-env-1",
                "name": ".env.local",
                "original_path": "D:\\Dev\\App\\.env.local",
                "extension": "local",
                "category": "database_code",
                "source": "recycle_bin",
                "size_bytes": env_raw.len(),
                "trim_zeroed": false,
                "source_letter": "D",
                "locator": { "r_path": env_file.to_string_lossy() }
            }]
        });
        let preview = preview_item(&job, "dev-env-1").unwrap();
        assert_eq!(preview["kind"], "text");
        assert_eq!(preview["secrets_masked"], 2);
        assert!(!preview["text"].as_str().unwrap().contains("sk-proj-12345"));
        assert!(preview["raw_text"]
            .as_str()
            .unwrap()
            .contains("sk-proj-12345"));
        assert_eq!(std::fs::read_to_string(&env_file).unwrap(), env_raw);

        assert!(crate::jobs::validate_options(
            "recovery",
            &json!({ "mode": "dev", "size_mib": "auto" })
        )
        .is_ok());
    }
}
