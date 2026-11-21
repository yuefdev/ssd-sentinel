//! Pure-Rust binary media boundary detection, structural inspection, and active repair
//! for recovered photos and videos (JPEG, PNG, GIF, BMP, WEBP, MP4/MOV/M4V, AVI, MKV/WEBM, WAV).

use serde::{Deserialize, Serialize};

const MAX_MP4_BOX_CAP: u64 = 512 * 1024 * 1024; // 512 MiB safety cap for carved box sizing
const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
const PNG_IEND_CHUNK: [u8; 12] = [
    0x00, 0x00, 0x00, 0x00, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82,
];

/// Structural verification and active-repair report for a recovered media buffer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRepairReport {
    /// `"intact"` | `"repaired"` | `"thumbnail_rescued"` | `"unrecoverable"`
    pub status: String,
    /// `true` ONLY when the inspected/repaired output is structurally valid and openable/playable.
    pub openable: bool,
    /// `true` if bytes must be / were modified or rebuilt to make the media openable.
    pub repaired: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Concise Turkish explanation of what was verified or repaired.
    pub note_tr: String,
}

impl MediaRepairReport {
    fn intact(width: Option<u32>, height: Option<u32>, note_tr: impl Into<String>) -> Self {
        Self {
            status: "intact".to_owned(),
            openable: true,
            repaired: false,
            width,
            height,
            note_tr: note_tr.into(),
        }
    }

    fn repaired(width: Option<u32>, height: Option<u32>, note_tr: impl Into<String>) -> Self {
        Self {
            status: "repaired".to_owned(),
            openable: true,
            repaired: true,
            width,
            height,
            note_tr: note_tr.into(),
        }
    }

    fn thumbnail_rescued(
        width: Option<u32>,
        height: Option<u32>,
        note_tr: impl Into<String>,
    ) -> Self {
        Self {
            status: "thumbnail_rescued".to_owned(),
            openable: true,
            repaired: true,
            width,
            height,
            note_tr: note_tr.into(),
        }
    }

    fn unrecoverable(width: Option<u32>, height: Option<u32>, note_tr: impl Into<String>) -> Self {
        Self {
            status: "unrecoverable".to_owned(),
            openable: false,
            repaired: false,
            width,
            height,
            note_tr: note_tr.into(),
        }
    }
}

fn normalize_ext(ext: &str) -> String {
    ext.trim().trim_start_matches('.').to_ascii_lowercase()
}

/// Returns `true` if the extension is handled by the deep structural verifier & active repairer.
pub fn is_supported_media_ext(ext: &str) -> bool {
    matches!(
        normalize_ext(ext).as_str(),
        "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "bmp"
            | "webp"
            | "mp4"
            | "mov"
            | "m4v"
            | "avi"
            | "mkv"
            | "webm"
            | "wav"
    )
}

fn is_uniform_or_empty(data: &[u8]) -> bool {
    if data.is_empty() {
        return true;
    }
    let first = data[0];
    data.iter().all(|&b| b == first)
}

fn has_nontrivial_bytes(data: &[u8], min_non_uniform: usize) -> bool {
    if data.len() < min_non_uniform {
        return false;
    }
    let mut non_zero = 0usize;
    let mut non_ff = 0usize;
    for &b in data.iter().take(65_536) {
        if b != 0x00 {
            non_zero += 1;
        }
        if b != 0xFF {
            non_ff += 1;
        }
        if non_zero >= min_non_uniform && non_ff >= min_non_uniform {
            return true;
        }
    }
    false
}

fn read_u16_be(data: &[u8], off: usize) -> Option<u16> {
    let bytes: [u8; 2] = data.get(off..off + 2)?.try_into().ok()?;
    Some(u16::from_be_bytes(bytes))
}

fn read_u32_be(data: &[u8], off: usize) -> Option<u32> {
    let bytes: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(bytes))
}

fn read_u64_be(data: &[u8], off: usize) -> Option<u64> {
    let bytes: [u8; 8] = data.get(off..off + 8)?.try_into().ok()?;
    Some(u64::from_be_bytes(bytes))
}

fn read_u16_le(data: &[u8], off: usize) -> Option<u16> {
    let bytes: [u8; 2] = data.get(off..off + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(bytes))
}

fn read_u32_le(data: &[u8], off: usize) -> Option<u32> {
    let bytes: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn read_i32_le(data: &[u8], off: usize) -> Option<i32> {
    let bytes: [u8; 4] = data.get(off..off + 4)?.try_into().ok()?;
    Some(i32::from_le_bytes(bytes))
}

// ============================================================================
// Pure-Rust IEEE 802.3 CRC-32 (used for PNG chunk verification and synthesis)
// ============================================================================

const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 {
            if (c & 1) != 0 {
                c = 0xEDB8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            j += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = CRC32_TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc
}

fn crc32(data: &[u8]) -> u32 {
    !crc32_update(!0u32, data)
}

fn crc32_two(a: &[u8], b: &[u8]) -> u32 {
    let c = crc32_update(!0u32, a);
    !crc32_update(c, b)
}

// ============================================================================
// 1. Accurate Carving Boundary Helpers
// ============================================================================

/// Walks JPEG marker segments using 2-byte big-endian segment lengths so embedded EXIF
/// thumbnails (`FF D9` inside `APP1`) NEVER prematurely truncate the main photo.
/// After `FF DA` (SOS), scans entropy-coded data skipping `FF 00` byte-stuffing and
/// `FF D0..=D7` restart markers until the real `FF D9` EOI (or returns best valid length
/// when truncated after a valid scan).
pub fn find_jpeg_end(data: &[u8]) -> Option<usize> {
    let parsed = parse_jpeg(data);
    if let Some(eoi) = parsed.eoi_end {
        return Some(eoi);
    }
    if parsed.has_sof && parsed.has_sos && parsed.best_valid_end > parsed.first_scan_start {
        return Some(parsed.best_valid_end);
    }
    None
}

/// Walks top-level ISO-BMFF boxes (`ftyp`, `free`, `skip`, `wide`, `mdat`, `moov`, `meta`,
/// `uuid`, `pdin`, `styp`, `moof`, `mfra`, etc.) supporting both 32-bit size and 64-bit
/// `largesize` (`size == 1`), returning the true end of all contiguous valid boxes.
pub fn find_mp4_size(data: &[u8]) -> Option<usize> {
    if data.len() < 16 {
        return None;
    }

    let mut pos = 0usize;
    let mut seen_ftyp = false;
    let mut seen_second_box = false;
    let mut best_end = 0usize;

    while pos + 8 <= data.len() {
        let size32 = read_u32_be(data, pos)? as u64;
        let fourcc: [u8; 4] = data[pos + 4..pos + 8].try_into().ok()?;

        if pos == 0 {
            if &fourcc == b"ftyp" {
                if !(12..=512).contains(&size32) {
                    return None;
                }
                if !is_valid_ftyp_brand(data, pos + 8) {
                    return None;
                }
                seen_ftyp = true;
            } else if matches!(&fourcc, b"wide" | b"free" | b"skip") && (8..=64).contains(&size32) {
                // QuickTime MOV occasionally places a leading `wide`/`free` box before `ftyp`/`mdat`.
            } else {
                return None;
            }
        } else if !is_known_mp4_top_box(&fourcc) {
            break;
        } else if &fourcc == b"ftyp" {
            if !(12..=512).contains(&size32) || !is_valid_ftyp_brand(data, pos + 8) {
                break;
            }
            seen_ftyp = true;
        }

        let (box_size, hdr_len) = if size32 == 1 {
            if pos + 16 > data.len() {
                break;
            }
            let sz64 = read_u64_be(data, pos + 8)?;
            if sz64 < 16 {
                break;
            }
            (sz64, 16usize)
        } else if size32 == 0 {
            // Extends to end of file/slice.
            let rem = (data.len() - pos) as u64;
            if rem < 8 {
                break;
            }
            if pos > 0 {
                seen_second_box = true;
            }
            best_end = data.len();
            break;
        } else {
            if size32 < 8 {
                break;
            }
            (size32, 8usize)
        };

        let _ = hdr_len;
        let Some(next_u64) = (pos as u64).checked_add(box_size) else {
            break;
        };

        if pos > 0 {
            seen_second_box = true;
        }

        if next_u64 <= data.len() as u64 {
            pos = next_u64 as usize;
            best_end = pos;
        } else {
            // Box extends past current slice (e.g. a multi-MiB `mdat` inside a 2 MiB carving window).
            if next_u64 <= MAX_MP4_BOX_CAP {
                best_end = next_u64 as usize;
            } else if seen_second_box {
                best_end = data.len();
            }
            break;
        }
    }

    if seen_ftyp && seen_second_box && best_end >= 24 {
        Some(best_end)
    } else {
        None
    }
}

/// Walks PNG chunks (`IHDR`..`IEND`) using chunk lengths instead of naive signature scanning.
pub fn find_png_end(data: &[u8]) -> Option<usize> {
    if data.len() < 33 || !data.starts_with(&PNG_SIGNATURE) {
        return None;
    }
    parse_png_ihdr(data)?;

    let mut pos = 8usize;
    let mut seen_idat = false;
    while pos + 12 <= data.len() {
        let chunk_len = read_u32_be(data, pos)? as usize;
        let ctype = &data[pos + 4..pos + 8];
        if !ctype.iter().all(|b| b.is_ascii_alphabetic()) {
            break;
        }
        let Some(chunk_end) = pos.checked_add(12).and_then(|v| v.checked_add(chunk_len)) else {
            break;
        };
        if ctype == b"IDAT" && (pos + 8 < data.len()) {
            seen_idat = true;
        }
        if chunk_end > data.len() {
            if seen_idat {
                return Some(data.len());
            }
            return None;
        }
        pos = chunk_end;
        if ctype == b"IEND" && chunk_len == 0 {
            return Some(pos);
        }
    }

    if seen_idat && pos > 33 {
        Some(pos)
    } else {
        None
    }
}

/// Reads RIFF size header (`+4..+8`) + 8 for AVI/WEBP/WAV, clamped safely to slice bounds.
pub fn find_riff_size(data: &[u8]) -> Option<usize> {
    if data.len() < 12 || &data[0..4] != b"RIFF" {
        return None;
    }
    let form = &data[8..12];
    if !matches!(form, b"WEBP" | b"AVI " | b"WAVE") {
        return None;
    }
    let payload_sz = read_u32_le(data, 4)? as usize;
    let total = payload_sz.checked_add(8)?;
    if total < 12 {
        return None;
    }
    Some(total.min(data.len()))
}

// ============================================================================
// 2. Public Media Inspection & Active Repair Entrypoints
// ============================================================================

/// Inspects a recovered media buffer without allocating repaired output copies and reports
/// whether it is intact, actively repairable, EXIF-thumbnail-rescuable, or truly unrecoverable.
pub fn inspect_media(ext: &str, data: &[u8]) -> MediaRepairReport {
    dispatch_media(ext, data, false).1
}

/// Inspects and actively repairs a recovered photo or video buffer into an openable/playable
/// byte vector whenever structurally possible.
pub fn repair_media(ext: &str, data: &[u8]) -> (Vec<u8>, MediaRepairReport) {
    let (repaired_opt, report) = dispatch_media(ext, data, true);
    match repaired_opt {
        Some(bytes) => (bytes, report),
        None => (data.to_vec(), report),
    }
}

fn dispatch_media(
    ext: &str,
    data: &[u8],
    build_output: bool,
) -> (Option<Vec<u8>>, MediaRepairReport) {
    if data.len() < 12 || is_uniform_or_empty(data) {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "Medya içeriği boş veya tamamen sıfırlanmış (TRIM/boş sektör); açılamaz.",
            ),
        );
    }

    let norm = normalize_ext(ext);
    let effective = if norm.is_empty() {
        detect_ext_by_magic(data).unwrap_or("")
    } else {
        norm.as_str()
    };

    match effective {
        "jpg" | "jpeg" => process_jpeg(data, build_output),
        "png" => process_png(data, build_output),
        "gif" => process_gif(data, build_output),
        "bmp" => process_bmp(data, build_output),
        "webp" => process_webp(data, build_output),
        "mp4" | "mov" | "m4v" => process_mp4(data, build_output),
        "avi" => process_avi(data, build_output),
        "mkv" | "webm" => process_mkv(data, build_output),
        "wav" => process_wav(data, build_output),
        "heic" | "heif" | "m4a" => (None, inspect_isobmff_generic(effective, data)),
        "tif" | "tiff" | "cr2" | "nef" => (None, inspect_tiff_like(data)),
        "ico" => (None, inspect_ico(data)),
        "svg" => (None, inspect_svg(data)),
        "psd" => (None, inspect_psd(data)),
        "mp3" => (None, inspect_mp3(data)),
        "flac" => (None, inspect_magic_simple(data, b"fLaC", "FLAC ses")),
        "ogg" => (None, inspect_magic_simple(data, b"OggS", "OGG ses")),
        "aac" => (None, inspect_aac(data)),
        "flv" => (None, inspect_magic_simple(data, b"FLV\x01", "FLV video")),
        "wmv" => (
            None,
            inspect_magic_simple(
                data,
                b"\x30\x26\xB2\x75\x8E\x66\xCF\x11\xA6\xD9\x00\xAA\x00\x62\xCE\x6C",
                "WMV/ASF video",
            ),
        ),
        _ => {
            if let Some(magic_ext) = detect_ext_by_magic(data) {
                dispatch_media(magic_ext, data, build_output)
            } else {
                (
                    None,
                    MediaRepairReport::unrecoverable(
                        None,
                        None,
                        "Tanınabilir medya başlığı bulunamadı; açılamaz.",
                    ),
                )
            }
        }
    }
}

fn detect_ext_by_magic(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if data.starts_with(&PNG_SIGNATURE) {
        return Some("png");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if data.starts_with(b"BM") && data.len() >= 54 {
        return Some("bmp");
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" {
        return match &data[8..12] {
            b"WEBP" => Some("webp"),
            b"AVI " => Some("avi"),
            b"WAVE" => Some("wav"),
            _ => None,
        };
    }
    if data.len() >= 12 && &data[4..8] == b"ftyp" {
        return Some("mp4");
    }
    if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("mkv");
    }
    None
}

// ============================================================================
// 3. JPEG Parser, Repairer, and EXIF Thumbnail Rescuer
// ============================================================================

#[derive(Default)]
struct JpegParseResult {
    width: Option<u32>,
    height: Option<u32>,
    has_sof: bool,
    has_dqt: bool,
    has_dht_or_dac: bool,
    has_sos: bool,
    first_scan_start: usize,
    eoi_end: Option<usize>,
    best_valid_end: usize,
    app_ranges: Vec<(usize, usize)>,
}

fn is_sof_marker(m: u8) -> bool {
    matches!(
        m,
        0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF
    )
}

fn is_post_scan_marker(m: u8) -> bool {
    matches!(m, 0xC4 | 0xCC | 0xDA | 0xDB | 0xDD | 0xE0..=0xEF | 0xFE)
}

fn parse_jpeg(data: &[u8]) -> JpegParseResult {
    let mut res = JpegParseResult::default();
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return res;
    }

    let mut pos = 2usize;
    let mut in_scan = false;

    'outer: while pos < data.len() {
        if !in_scan {
            // Expect 0xFF marker prefix (skip optional fill 0xFF bytes per ITU-T T.81).
            if data[pos] != 0xFF {
                break;
            }
            while pos < data.len() && data[pos] == 0xFF {
                pos += 1;
            }
            if pos >= data.len() {
                break;
            }
            let marker = data[pos];
            pos += 1;

            match marker {
                0x00 => break, // Stuffed byte outside entropy stream is invalid.
                0xD8 => continue,
                0xD9 => {
                    if res.has_sos {
                        res.eoi_end = Some(pos);
                        res.best_valid_end = pos;
                    }
                    break;
                }
                0xD0..=0xD7 | 0x01 => continue,
                _ => {
                    let Some(seg_len) = read_u16_be(data, pos).map(usize::from) else {
                        break;
                    };
                    if seg_len < 2 {
                        break;
                    }
                    let Some(seg_end) = pos.checked_add(seg_len) else {
                        break;
                    };
                    if seg_end > data.len() {
                        // Record truncated APP1/APP0 slice so EXIF thumbnail rescue can still inspect it.
                        if matches!(marker, 0xE0..=0xE2) && pos + 2 < data.len() {
                            res.app_ranges.push((pos + 2, data.len()));
                        }
                        break;
                    }
                    let payload = &data[pos + 2..seg_end];

                    if matches!(marker, 0xE0..=0xE2) {
                        res.app_ranges.push((pos + 2, seg_end));
                    } else if is_sof_marker(marker) {
                        if payload.len() >= 6 {
                            let precision = payload[0];
                            let h = u16::from_be_bytes([payload[1], payload[2]]) as u32;
                            let w = u16::from_be_bytes([payload[3], payload[4]]) as u32;
                            let num_comp = payload[5] as usize;
                            if matches!(precision, 8 | 12 | 16)
                                && w > 0
                                && h > 0
                                && (1..=4).contains(&num_comp)
                                && payload.len() >= 6 + 3 * num_comp
                            {
                                res.has_sof = true;
                                res.width = Some(w);
                                res.height = Some(h);
                            }
                        }
                    } else if marker == 0xDB {
                        if payload.len() >= 2 {
                            res.has_dqt = true;
                        }
                    } else if marker == 0xC4 || marker == 0xCC {
                        if payload.len() >= 2 {
                            res.has_dht_or_dac = true;
                        }
                    } else if marker == 0xDA {
                        // SOS header: 1 byte component count + 2*Ns bytes + 3 spectral selection bytes.
                        if !payload.is_empty() {
                            let ns = payload[0] as usize;
                            if (1..=4).contains(&ns) && payload.len() == 1 + 2 * ns + 3 {
                                if !res.has_sos {
                                    res.first_scan_start = seg_end;
                                }
                                res.has_sos = true;
                                pos = seg_end;
                                res.best_valid_end = pos;
                                in_scan = true;
                                continue;
                            }
                        }
                        break;
                    }

                    pos = seg_end;
                }
            }
        } else {
            // Entropy-coded scan segment after SOS.
            let mut zero_run = 0usize;
            while pos < data.len() {
                let b = data[pos];
                if b == 0x00 {
                    zero_run += 1;
                    if zero_run >= 512 {
                        // Hit a TRIM/zero-filled disk sector boundary inside truncated scan.
                        res.best_valid_end = pos + 1 - zero_run;
                        break 'outer;
                    }
                    pos += 1;
                    continue;
                }
                zero_run = 0;

                if b != 0xFF {
                    pos += 1;
                    continue;
                }

                let ff_start = pos;
                while pos < data.len() && data[pos] == 0xFF {
                    pos += 1;
                }
                if pos >= data.len() {
                    // Dangling 0xFF at EOF — keep valid data right before it.
                    res.best_valid_end = ff_start;
                    break 'outer;
                }
                let next = data[pos];
                match next {
                    0x00 => {
                        // Byte-stuffed literal 0xFF in entropy stream.
                        pos += 1;
                    }
                    0xD0..=0xD7 => {
                        // Restart marker RSTm inside entropy stream.
                        pos += 1;
                    }
                    0xD9 => {
                        // Real EOI marker!
                        res.eoi_end = Some(pos + 1);
                        res.best_valid_end = pos + 1;
                        break 'outer;
                    }
                    m if is_post_scan_marker(m) => {
                        // Progressive JPEG next scan/DHT/DRI/APP/COM — verify segment length fits
                        // before switching back to marker mode; otherwise treat as overwritten tail.
                        let seg_ok =
                            if let Some(seg_len) = read_u16_be(data, pos + 1).map(usize::from) {
                                seg_len >= 2 && pos + 1 + seg_len <= data.len()
                            } else {
                                false
                            };
                        if seg_ok {
                            res.best_valid_end = ff_start;
                            pos = ff_start;
                            in_scan = false;
                            continue 'outer;
                        } else {
                            res.best_valid_end = ff_start;
                            break 'outer;
                        }
                    }
                    _ => {
                        // Invalid unstuffed marker in entropy stream (overwritten tail sector).
                        res.best_valid_end = ff_start;
                        break 'outer;
                    }
                }
            }

            if pos >= data.len() {
                res.best_valid_end = data.len();
                break;
            }
        }
    }

    res
}

fn trim_jpeg_truncated_tail(data: &[u8], scan_start: usize, valid_end: usize) -> usize {
    let mut cut = valid_end.min(data.len());
    while cut > scan_start {
        let last = data[cut - 1];
        if last == 0x00 || last == 0xFF {
            cut -= 1;
        } else {
            break;
        }
    }
    cut
}

fn rescue_exif_thumbnail(
    data: &[u8],
    app_ranges: &[(usize, usize)],
) -> Option<(usize, usize, Option<u32>, Option<u32>)> {
    type ThumbCandidate = (u64, usize, usize, Option<u32>, Option<u32>);
    let mut best: Option<ThumbCandidate> = None;

    let scan_region =
        |region_start: usize, region_end: usize, best: &mut Option<ThumbCandidate>| {
            if region_end <= region_start + 32 {
                return;
            }
            let slice = &data[region_start..region_end];
            let mut i = 0usize;
            while i + 3 < slice.len() {
                if slice[i] == 0xFF
                    && slice[i + 1] == 0xD8
                    && slice[i + 2] == 0xFF
                    && matches!(slice[i + 3], 0xDB | 0xE0..=0xEF | 0xC0..=0xC4 | 0xFE)
                {
                    let abs_start = region_start + i;
                    let sub = parse_jpeg(&data[abs_start..region_end]);
                    if sub.has_sof && sub.has_dqt && sub.has_dht_or_dac && sub.has_sos {
                        if let Some(sub_eoi) = sub.eoi_end {
                            let scan_slice =
                                &data[abs_start + sub.first_scan_start..abs_start + sub_eoi - 2];
                            if has_nontrivial_bytes(scan_slice, 2) {
                                let pixels = u64::from(sub.width.unwrap_or(1))
                                    .saturating_mul(u64::from(sub.height.unwrap_or(1)));
                                let score = pixels
                                    .saturating_mul(1_000_000)
                                    .saturating_add(sub_eoi as u64);
                                if best.as_ref().is_none_or(|b| score > b.0) {
                                    *best = Some((
                                        score,
                                        abs_start,
                                        abs_start + sub_eoi,
                                        sub.width,
                                        sub.height,
                                    ));
                                }
                                i += sub_eoi;
                                continue;
                            }
                        }
                    }
                }
                i += 1;
            }
        };

    for &(s, e) in app_ranges {
        scan_region(s, e, &mut best);
    }

    // Fallback: if marker table broke early due to corrupt APP length, scan first 128 KiB past offset 4.
    if best.is_none() && data.len() > 64 {
        let cap = data.len().min(131_072);
        scan_region(4, cap, &mut best);
    }

    best.map(|(_, s, e, w, h)| (s, e, w, h))
}

fn process_jpeg(data: &[u8], build_output: bool) -> (Option<Vec<u8>>, MediaRepairReport) {
    let parsed = parse_jpeg(data);

    // 1. Check intact JPEG
    if parsed.has_sof && parsed.has_dqt && parsed.has_dht_or_dac && parsed.has_sos {
        if let Some(eoi_end) = parsed.eoi_end {
            let scan_payload = if eoi_end >= parsed.first_scan_start + 2 {
                &data[parsed.first_scan_start..eoi_end - 2]
            } else {
                &[]
            };
            if has_nontrivial_bytes(scan_payload, 2) {
                let out = if build_output {
                    Some(data[..eoi_end].to_vec())
                } else {
                    None
                };
                return (
                    out,
                    MediaRepairReport::intact(
                        parsed.width,
                        parsed.height,
                        "JPEG yapısı sağlam; doğrudan açılabilir.",
                    ),
                );
            }
        }

        // 2. Truncated or damaged-tail JPEG after valid SOS -> trim cleanly and append FF D9
        let cut = trim_jpeg_truncated_tail(data, parsed.first_scan_start, parsed.best_valid_end);
        if cut > parsed.first_scan_start {
            let scan_payload = &data[parsed.first_scan_start..cut];
            if has_nontrivial_bytes(scan_payload, 2) {
                let out = if build_output {
                    let mut repaired = Vec::with_capacity(cut + 2);
                    repaired.extend_from_slice(&data[..cut]);
                    repaired.push(0xFF);
                    repaired.push(0xD9);
                    Some(repaired)
                } else {
                    None
                };
                return (
                    out,
                    MediaRepairReport::repaired(
                        parsed.width,
                        parsed.height,
                        "Eksik JPEG sonu (EOI) onarıldı; kurtarılan bölüm açılabilir hale getirildi.",
                    ),
                );
            }
        }
    }

    // 3. Main image unusable -> try rescuing an embedded intact EXIF thumbnail
    if let Some((start, end, tw, th)) = rescue_exif_thumbnail(data, &parsed.app_ranges) {
        let out = if build_output {
            Some(data[start..end].to_vec())
        } else {
            None
        };
        return (
            out,
            MediaRepairReport::thumbnail_rescued(
                tw,
                th,
                "Ana görüntü hasarlıydı; gömülü EXIF önizleme fotoğrafı kurtarıldı.",
            ),
        );
    }

    (
        None,
        MediaRepairReport::unrecoverable(
            parsed.width,
            parsed.height,
            "JPEG başlığı veya görüntü verisi ağır hasarlı; açılamaz.",
        ),
    )
}

// ============================================================================
// 4. PNG Parser & Active Chunk/CRC-32/IEND Repairer
// ============================================================================

struct PngIhdr {
    width: u32,
    height: u32,
    color_type: u8,
    crc_valid: bool,
}

fn parse_png_ihdr(data: &[u8]) -> Option<PngIhdr> {
    if data.len() < 33 || !data.starts_with(&PNG_SIGNATURE) {
        return None;
    }
    let len = read_u32_be(data, 8)?;
    if len != 13 || &data[12..16] != b"IHDR" {
        return None;
    }
    let width = read_u32_be(data, 16)?;
    let height = read_u32_be(data, 20)?;
    if width == 0 || width > 65_535 || height == 0 || height > 65_535 {
        return None;
    }
    let bit_depth = data[24];
    let color_type = data[25];
    let compression = data[26];
    let filter = data[27];
    let interlace = data[28];

    let valid_pair = match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        4 => matches!(bit_depth, 8 | 16),
        6 => matches!(bit_depth, 8 | 16),
        _ => false,
    };
    if !valid_pair || compression != 0 || filter != 0 || interlace > 1 {
        return None;
    }

    let stored_crc = read_u32_be(data, 29)?;
    let calc_crc = crc32(&data[12..29]);

    Some(PngIhdr {
        width,
        height,
        color_type,
        crc_valid: stored_crc == calc_crc,
    })
}

fn is_valid_zlib_header(bytes: &[u8]) -> bool {
    if bytes.len() < 2 {
        return false;
    }
    let cmf = bytes[0];
    let flg = bytes[1];
    let cm = cmf & 0x0F;
    let cinfo = cmf >> 4;
    cm == 8 && cinfo <= 7 && (((u16::from(cmf) << 8) | u16::from(flg)) % 31 == 0)
}

fn push_png_chunk(out: &mut Vec<u8>, ctype: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(ctype);
    out.extend_from_slice(payload);
    let c = crc32_two(ctype, payload);
    out.extend_from_slice(&c.to_be_bytes());
}

fn process_png(data: &[u8], build_output: bool) -> (Option<Vec<u8>>, MediaRepairReport) {
    let Some(ihdr) = parse_png_ihdr(data) else {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "Geçerli PNG imzası veya IHDR başlığı bulunamadı; açılamaz.",
            ),
        );
    };

    let mut pos = 8usize;
    let mut kept_chunks: Vec<([u8; 4], &[u8])> = Vec::new();
    let mut has_plte = false;
    let mut first_idat_checked = false;
    let mut first_idat_zlib_ok = false;
    let mut idat_bytes_total = 0usize;
    let mut seen_valid_iend = false;
    let mut intact_end = 0usize;
    let mut any_crc_fixed = !ihdr.crc_valid;
    let mut truncated_chunk = false;

    while pos + 8 <= data.len() {
        let Some(chunk_len) = read_u32_be(data, pos).map(|v| v as usize) else {
            break;
        };
        let ctype: [u8; 4] = match data[pos + 4..pos + 8].try_into() {
            Ok(v) => v,
            Err(_) => break,
        };
        if !ctype.iter().all(|b| b.is_ascii_alphabetic()) {
            truncated_chunk = true;
            break;
        }

        let payload_start = pos + 8;
        let full_chunk_end = pos.checked_add(12).and_then(|v| v.checked_add(chunk_len));

        match full_chunk_end {
            Some(end) if end <= data.len() => {
                let payload = &data[payload_start..payload_start + chunk_len];
                let stored_crc = read_u32_be(data, end - 4).unwrap_or(0);
                let calc_crc = crc32_two(&ctype, payload);
                let crc_ok = stored_crc == calc_crc;

                if &ctype == b"IEND" {
                    if chunk_len == 0 {
                        if !crc_ok {
                            any_crc_fixed = true;
                        }
                        seen_valid_iend = true;
                        intact_end = end;
                    } else {
                        truncated_chunk = true;
                    }
                    break;
                }

                if &ctype == b"PLTE" {
                    if !payload.is_empty() && payload.len().is_multiple_of(3) {
                        has_plte = true;
                        if !crc_ok {
                            any_crc_fixed = true;
                        }
                        kept_chunks.push((ctype, payload));
                    }
                } else if &ctype == b"IDAT" {
                    if !payload.is_empty() {
                        if !first_idat_checked {
                            first_idat_checked = true;
                            first_idat_zlib_ok = is_valid_zlib_header(payload);
                        }
                        if first_idat_zlib_ok {
                            if !crc_ok {
                                any_crc_fixed = true;
                            }
                            idat_bytes_total += payload.len();
                            kept_chunks.push((ctype, payload));
                        } else {
                            break;
                        }
                    }
                } else if &ctype == b"IHDR"
                    || matches!(
                        &ctype,
                        b"tRNS" | b"gAMA" | b"cHRM" | b"sRGB" | b"iCCP" | b"bKGD" | b"pHYs"
                    )
                {
                    if !crc_ok {
                        any_crc_fixed = true;
                    }
                    kept_chunks.push((ctype, payload));
                } else if crc_ok {
                    kept_chunks.push((ctype, payload));
                }

                pos = end;
            }
            _ => {
                // Chunk is truncated before its declared end.
                truncated_chunk = true;
                if &ctype == b"IDAT" && payload_start < data.len() {
                    let mut avail = &data[payload_start..];
                    // Strip long trailing zero-sector padding if present (> 16 trailing zeros).
                    let trailing_zeros = avail.iter().rev().take_while(|&&b| b == 0).count();
                    if trailing_zeros >= 16 && avail.len() > trailing_zeros {
                        avail = &avail[..avail.len() - trailing_zeros];
                    }
                    if !first_idat_checked {
                        first_idat_zlib_ok = is_valid_zlib_header(avail);
                    }
                    if first_idat_zlib_ok && avail.len() >= 2 {
                        idat_bytes_total += avail.len();
                        kept_chunks.push((ctype, avail));
                    }
                }
                break;
            }
        }
    }

    let palette_ok = ihdr.color_type != 3 || has_plte;
    if !first_idat_zlib_ok || idat_bytes_total < 2 || !palette_ok {
        return (
            None,
            MediaRepairReport::unrecoverable(
                Some(ihdr.width),
                Some(ihdr.height),
                "PNG görüntü akışı (IDAT/zlib) eksik veya bozuk; açılamaz.",
            ),
        );
    }

    if seen_valid_iend && !any_crc_fixed && !truncated_chunk {
        let out = if build_output {
            Some(data[..intact_end].to_vec())
        } else {
            None
        };
        return (
            out,
            MediaRepairReport::intact(
                Some(ihdr.width),
                Some(ihdr.height),
                "PNG yapısı ve CRC-32 toplamları sağlam; doğrudan açılabilir.",
            ),
        );
    }

    let out = if build_output {
        let approx = 8 + idat_bytes_total + kept_chunks.len() * 16 + 12;
        let mut rebuilt = Vec::with_capacity(approx);
        rebuilt.extend_from_slice(&PNG_SIGNATURE);
        for (ctype, payload) in kept_chunks {
            push_png_chunk(&mut rebuilt, &ctype, payload);
        }
        rebuilt.extend_from_slice(&PNG_IEND_CHUNK);
        Some(rebuilt)
    } else {
        None
    };

    (
        out,
        MediaRepairReport::repaired(
            Some(ihdr.width),
            Some(ihdr.height),
            "Eksik PNG IEND/CRC yapısı onarıldı; kurtarılan satırlar açılabilir.",
        ),
    )
}

// ============================================================================
// 5. GIF Inspection & Active Repair
// ============================================================================

fn process_gif(data: &[u8], build_output: bool) -> (Option<Vec<u8>>, MediaRepairReport) {
    if data.len() < 14 || (!data.starts_with(b"GIF87a") && !data.starts_with(b"GIF89a")) {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "Geçerli GIF başlığı bulunamadı; açılamaz.",
            ),
        );
    }
    let width = read_u16_le(data, 6).unwrap_or(0) as u32;
    let height = read_u16_le(data, 8).unwrap_or(0) as u32;
    if width == 0 || height == 0 {
        return (
            None,
            MediaRepairReport::unrecoverable(None, None, "GIF ekran boyutları geçersiz; açılamaz."),
        );
    }

    let packed = data[10];
    let gct_size = if (packed & 0x80) != 0 {
        3 * (1usize << ((packed & 0x07) + 1))
    } else {
        0
    };
    let mut pos = 13 + gct_size;
    if pos >= data.len() {
        return (
            None,
            MediaRepairReport::unrecoverable(
                Some(width),
                Some(height),
                "GIF renk tablosu veya görüntü bloğu kesilmiş; açılamaz.",
            ),
        );
    }

    let mut complete_images = 0usize;
    let mut last_clean_end = pos;
    let mut partial_image_fix: Option<(usize, u8)> = None; // (sub_block_len_offset, new_len)
    let mut needs_sub_terminator = false;

    while pos < data.len() {
        match data[pos] {
            0x3B => {
                // Proper GIF trailer.
                if complete_images >= 1 {
                    let end = pos + 1;
                    let out = if build_output {
                        Some(data[..end].to_vec())
                    } else {
                        None
                    };
                    return (
                        out,
                        MediaRepairReport::intact(
                            Some(width),
                            Some(height),
                            "GIF yapısı sağlam; doğrudan açılabilir.",
                        ),
                    );
                }
                return (
                    None,
                    MediaRepairReport::unrecoverable(
                        Some(width),
                        Some(height),
                        "GIF dosyasında görüntü karesi bulunamadı; açılamaz.",
                    ),
                );
            }
            0x21 => {
                // Extension introducer.
                if pos + 2 >= data.len() {
                    break;
                }
                let mut cur = pos + 2;
                let mut ext_ok = false;
                while cur < data.len() {
                    let sub_len = data[cur] as usize;
                    cur += 1;
                    if sub_len == 0 {
                        ext_ok = true;
                        break;
                    }
                    if cur + sub_len > data.len() {
                        break;
                    }
                    cur += sub_len;
                }
                if !ext_ok {
                    break;
                }
                pos = cur;
                last_clean_end = pos;
            }
            0x2C => {
                // Image Descriptor (10 bytes).
                if pos + 10 > data.len() {
                    break;
                }
                let img_w = read_u16_le(data, pos + 5).unwrap_or(0);
                let img_h = read_u16_le(data, pos + 7).unwrap_or(0);
                if img_w == 0 || img_h == 0 {
                    break;
                }
                let img_packed = data[pos + 9];
                let lct_size = if (img_packed & 0x80) != 0 {
                    3 * (1usize << ((img_packed & 0x07) + 1))
                } else {
                    0
                };
                let lzw_pos = pos + 10 + lct_size;
                if lzw_pos >= data.len() {
                    break;
                }
                let lzw_min = data[lzw_pos];
                if !(2..=12).contains(&lzw_min) {
                    break;
                }

                let mut cur = lzw_pos + 1;
                let mut img_data_bytes = 0usize;
                let mut finished_img = false;

                while cur < data.len() {
                    let sub_len = data[cur] as usize;
                    if sub_len == 0 {
                        cur += 1;
                        finished_img = true;
                        break;
                    }
                    if cur + 1 + sub_len <= data.len() {
                        img_data_bytes += sub_len;
                        cur += 1 + sub_len;
                    } else {
                        // Truncated mid-sub-block!
                        let avail = data.len() - (cur + 1);
                        if avail > 0 {
                            img_data_bytes += avail;
                            partial_image_fix = Some((cur, avail as u8));
                            needs_sub_terminator = true;
                            last_clean_end = data.len();
                        } else if img_data_bytes > 0 {
                            needs_sub_terminator = true;
                            last_clean_end = cur;
                        }
                        break;
                    }
                }

                if finished_img && img_data_bytes > 0 {
                    complete_images += 1;
                    pos = cur;
                    last_clean_end = pos;
                } else if !finished_img && img_data_bytes > 0 {
                    if cur >= data.len() && partial_image_fix.is_none() {
                        needs_sub_terminator = true;
                        last_clean_end = data.len();
                    }
                    complete_images += 1;
                    break;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }

    if complete_images >= 1 {
        let out = if build_output {
            let mut repaired = data[..last_clean_end].to_vec();
            if let Some((off, new_len)) = partial_image_fix {
                if off < repaired.len() {
                    repaired[off] = new_len;
                }
            }
            if needs_sub_terminator {
                repaired.push(0x00);
            }
            repaired.push(0x3B);
            Some(repaired)
        } else {
            None
        };
        return (
            out,
            MediaRepairReport::repaired(
                Some(width),
                Some(height),
                "Eksik GIF alt-blok sonlandırıcı/trailer (0x3B) onarıldı; görsel açılabilir.",
            ),
        );
    }

    (
        None,
        MediaRepairReport::unrecoverable(
            Some(width),
            Some(height),
            "GIF görüntü akışı bulunamadı veya hasarlı; açılamaz.",
        ),
    )
}

// ============================================================================
// 6. BMP Inspection & Active Scanline/Header Repair
// ============================================================================

fn process_bmp(data: &[u8], build_output: bool) -> (Option<Vec<u8>>, MediaRepairReport) {
    if data.len() < 54 || &data[0..2] != b"BM" {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "Geçerli BMP başlığı bulunamadı; açılamaz.",
            ),
        );
    }

    let bf_size = read_u32_le(data, 2).unwrap_or(0);
    let bf_off_bits = read_u32_le(data, 10).unwrap_or(0) as usize;
    let dib_size = read_u32_le(data, 14).unwrap_or(0) as usize;
    let bi_width = read_i32_le(data, 18).unwrap_or(0);
    let bi_height = read_i32_le(data, 22).unwrap_or(0);
    let bi_planes = read_u16_le(data, 26).unwrap_or(0);
    let bi_bpp = read_u16_le(data, 28).unwrap_or(0);
    let bi_compression = read_u32_le(data, 30).unwrap_or(u32::MAX);
    let bi_size_image = read_u32_le(data, 34).unwrap_or(0) as usize;

    let abs_height = bi_height.unsigned_abs();
    if dib_size < 40
        || bf_off_bits < 14 + dib_size
        || bf_off_bits >= data.len()
        || !(1..=65_535).contains(&bi_width)
        || !(1..=65_535).contains(&abs_height)
        || bi_planes != 1
        || !matches!(bi_bpp, 1 | 4 | 8 | 16 | 24 | 32)
        || bi_compression > 6
    {
        return (
            None,
            MediaRepairReport::unrecoverable(None, None, "BMP DIB başlığı geçersiz; açılamaz."),
        );
    }

    let width = bi_width as u32;
    let avail_pixels = data.len() - bf_off_bits;
    if avail_pixels == 0 {
        return (
            None,
            MediaRepairReport::unrecoverable(
                Some(width),
                Some(abs_height),
                "BMP piksel verisi eksik; açılamaz.",
            ),
        );
    }

    // Uncompressed / bitfields BMP
    if matches!(bi_compression, 0 | 3 | 6) {
        let row_stride = ((u64::from(width) * u64::from(bi_bpp)).div_ceil(32)) * 4;
        if row_stride == 0 {
            return (
                None,
                MediaRepairReport::unrecoverable(
                    None,
                    None,
                    "BMP satır uzunluğu geçersiz; açılamaz.",
                ),
            );
        }
        let expected_pixels = row_stride.saturating_mul(u64::from(abs_height));
        let expected_total = (bf_off_bits as u64).saturating_add(expected_pixels);

        if avail_pixels as u64 >= expected_pixels {
            let header_consistent =
                (bf_size as u64) >= expected_total && (bf_size as usize) <= data.len();
            if header_consistent {
                let out = if build_output {
                    Some(data[..expected_total as usize].to_vec())
                } else {
                    None
                };
                return (
                    out,
                    MediaRepairReport::intact(
                        Some(width),
                        Some(abs_height),
                        "BMP yapısı sağlam; doğrudan açılabilir.",
                    ),
                );
            }
            let new_total = expected_total as usize;
            let out = if build_output {
                let mut repaired = data[..new_total].to_vec();
                repaired[2..6].copy_from_slice(&(new_total as u32).to_le_bytes());
                repaired[34..38].copy_from_slice(&(expected_pixels as u32).to_le_bytes());
                Some(repaired)
            } else {
                None
            };
            return (
                out,
                MediaRepairReport::repaired(
                    Some(width),
                    Some(abs_height),
                    "BMP dosya boyutu başlığı onarıldı; görsel açılabilir.",
                ),
            );
        }

        // Truncated pixel array -> clamp height to complete recovered scanlines (preserving sign).
        let complete_rows = (avail_pixels as u64) / row_stride;
        if complete_rows >= 1 {
            let new_img_size = (complete_rows * row_stride) as usize;
            let new_total = bf_off_bits + new_img_size;
            let new_h_i32 = if bi_height < 0 {
                -(complete_rows as i32)
            } else {
                complete_rows as i32
            };
            let out = if build_output {
                let mut repaired = data[..new_total].to_vec();
                repaired[2..6].copy_from_slice(&(new_total as u32).to_le_bytes());
                repaired[22..26].copy_from_slice(&new_h_i32.to_le_bytes());
                repaired[34..38].copy_from_slice(&(new_img_size as u32).to_le_bytes());
                Some(repaired)
            } else {
                None
            };
            return (
                out,
                MediaRepairReport::repaired(
                    Some(width),
                    Some(complete_rows as u32),
                    format!(
                        "Kesilmiş BMP başlığı onarıldı ({complete_rows}/{abs_height} satır kurtarıldı)."
                    ),
                ),
            );
        }

        return (
            None,
            MediaRepairReport::unrecoverable(
                Some(width),
                Some(abs_height),
                "BMP dosyasında tek tam tarama satırı bile yok; açılamaz.",
            ),
        );
    }

    // RLE4 / RLE8 BMP
    if avail_pixels >= 2 {
        let expected_end = bf_off_bits.saturating_add(bi_size_image);
        if bi_size_image > 0 && expected_end <= data.len() && (bf_size as usize) <= data.len() {
            return (
                if build_output {
                    Some(data[..expected_end].to_vec())
                } else {
                    None
                },
                MediaRepairReport::intact(
                    Some(width),
                    Some(abs_height),
                    "BMP (RLE) yapısı sağlam; doğrudan açılabilir.",
                ),
            );
        }
        let out = if build_output {
            let mut repaired = data.to_vec();
            repaired.extend_from_slice(&[0x00, 0x01]); // RLE End-Of-Bitmap marker
            let new_total = repaired.len() as u32;
            let new_img = (repaired.len() - bf_off_bits) as u32;
            repaired[2..6].copy_from_slice(&new_total.to_le_bytes());
            repaired[34..38].copy_from_slice(&new_img.to_le_bytes());
            Some(repaired)
        } else {
            None
        };
        return (
            out,
            MediaRepairReport::repaired(
                Some(width),
                Some(abs_height),
                "Kesilmiş BMP (RLE) başlığı ve bitiş işareti onarıldı.",
            ),
        );
    }

    (
        None,
        MediaRepairReport::unrecoverable(
            Some(width),
            Some(abs_height),
            "BMP piksel verisi eksik; açılamaz.",
        ),
    )
}

// ============================================================================
// 7. RIFF Container Formats: WEBP, AVI, WAV
// ============================================================================

fn parse_webp_dimensions(fourcc: &[u8], payload: &[u8]) -> Option<(u32, u32)> {
    match fourcc {
        b"VP8 " => {
            if payload.len() < 10 {
                return None;
            }
            if (payload[0] & 0x01) != 0 || payload[3..6] != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let w = (u16::from_le_bytes([payload[6], payload[7]]) & 0x3FFF) as u32;
            let h = (u16::from_le_bytes([payload[8], payload[9]]) & 0x3FFF) as u32;
            (w > 0 && h > 0).then_some((w, h))
        }
        b"VP8L" => {
            if payload.len() < 5 || payload[0] != 0x2F {
                return None;
            }
            let bits = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
            let w = (bits & 0x3FFF) + 1;
            let h = ((bits >> 14) & 0x3FFF) + 1;
            Some((w, h))
        }
        b"VP8X" => {
            if payload.len() < 10 {
                return None;
            }
            let w = 1
                + (u32::from(payload[4])
                    | (u32::from(payload[5]) << 8)
                    | (u32::from(payload[6]) << 16));
            let h = 1
                + (u32::from(payload[7])
                    | (u32::from(payload[8]) << 8)
                    | (u32::from(payload[9]) << 16));
            (w > 0 && h > 0 && w <= 16_384 && h <= 16_384).then_some((w, h))
        }
        _ => None,
    }
}

fn process_webp(data: &[u8], build_output: bool) -> (Option<Vec<u8>>, MediaRepairReport) {
    if data.len() < 24 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "Geçerli WebP RIFF başlığı bulunamadı; açılamaz.",
            ),
        );
    }

    let first_cc = &data[12..16];
    if !matches!(first_cc, b"VP8 " | b"VP8L" | b"VP8X") {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "WebP VP8/VP8L/VP8X akış başlığı eksik; açılamaz.",
            ),
        );
    }

    let declared_riff_total = (read_u32_le(data, 4).unwrap_or(0) as usize).saturating_add(8);
    let first_payload = &data[20..];
    let dims = parse_webp_dimensions(first_cc, first_payload);
    if dims.is_none() {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "WebP görüntü başlığı (VP8/VP8L/VP8X) bozuk; açılamaz.",
            ),
        );
    }
    let (width, height) = dims.unwrap();

    // Walk sub-chunks to see whether RIFF/chunk lengths fit inside available bytes.
    let mut pos = 12usize;
    let mut needs_patch = declared_riff_total > data.len() || declared_riff_total < 24;
    let mut truncated_chunk_off: Option<usize> = None;
    let mut has_image_payload = false;

    while pos + 8 <= data.len() {
        let cc = &data[pos..pos + 4];
        if !cc.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
            break;
        }
        let csz = read_u32_le(data, pos + 4).unwrap_or(0) as usize;
        let payload_start = pos + 8;
        let avail = data.len() - payload_start;
        if matches!(cc, b"VP8 " | b"VP8L" | b"ANMF")
            && avail >= 5
            && has_nontrivial_bytes(&data[payload_start..], 2)
        {
            has_image_payload = true;
        }
        if csz > avail {
            needs_patch = true;
            truncated_chunk_off = Some(pos);
            break;
        }
        let padded = csz.saturating_add(csz & 1);
        let Some(next) = payload_start.checked_add(padded) else {
            break;
        };
        pos = next;
        if pos >= declared_riff_total {
            break;
        }
    }

    if !has_image_payload && first_cc == b"VP8X" {
        return (
            None,
            MediaRepairReport::unrecoverable(
                Some(width),
                Some(height),
                "WebP VP8X başlığı var ancak görüntü karesi yok; açılamaz.",
            ),
        );
    }

    if !needs_patch {
        let intact_end = declared_riff_total.min(data.len());
        return (
            if build_output {
                Some(data[..intact_end].to_vec())
            } else {
                None
            },
            MediaRepairReport::intact(
                Some(width),
                Some(height),
                "WebP yapısı sağlam; doğrudan açılabilir.",
            ),
        );
    }

    let out = if build_output {
        let mut repaired = data.to_vec();
        if let Some(chunk_off) = truncated_chunk_off {
            let new_csz = (repaired.len() - (chunk_off + 8)) as u32;
            repaired[chunk_off + 4..chunk_off + 8].copy_from_slice(&new_csz.to_le_bytes());
        }
        let new_riff_sz = (repaired.len() - 8) as u32;
        repaired[4..8].copy_from_slice(&new_riff_sz.to_le_bytes());
        Some(repaired)
    } else {
        None
    };

    (
        out,
        MediaRepairReport::repaired(
            Some(width),
            Some(height),
            "Kesilmiş WebP RIFF ve veri parçası boyutu onarıldı; görsel açılabilir.",
        ),
    )
}

fn parse_avi_avih_dims(hdrl_data: &[u8]) -> (Option<u32>, Option<u32>) {
    let mut pos = 0usize;
    while pos + 8 <= hdrl_data.len() {
        let id = &hdrl_data[pos..pos + 4];
        let sz = read_u32_le(hdrl_data, pos + 4).unwrap_or(0) as usize;
        if id == b"avih" && pos + 8 + 40 <= hdrl_data.len() && sz >= 40 {
            let w = read_u32_le(hdrl_data, pos + 8 + 32).unwrap_or(0);
            let h = read_u32_le(hdrl_data, pos + 8 + 36).unwrap_or(0);
            if (1..=16_384).contains(&w) && (1..=16_384).contains(&h) {
                return (Some(w), Some(h));
            }
        }
        let step = 8usize.saturating_add(sz).saturating_add(sz & 1);
        if step < 8 || pos + step > hdrl_data.len() {
            break;
        }
        pos += step;
    }
    (None, None)
}

fn process_avi(data: &[u8], build_output: bool) -> (Option<Vec<u8>>, MediaRepairReport) {
    if data.len() < 32 || &data[0..4] != b"RIFF" || &data[8..12] != b"AVI " {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "Geçerli AVI RIFF başlığı bulunamadı; oynatılamaz.",
            ),
        );
    }

    let declared_riff_total = (read_u32_le(data, 4).unwrap_or(0) as usize).saturating_add(8);
    let mut pos = 12usize;
    let mut has_hdrl = false;
    let mut width = None;
    let mut height = None;
    let mut movi_info: Option<(usize, usize, bool)> = None; // (list_offset, declared_end, truncated)

    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let sz = read_u32_le(data, pos + 4).unwrap_or(0) as usize;
        let payload_start = pos + 8;

        if id == b"LIST" && payload_start + 4 <= data.len() {
            let list_type = &data[payload_start..payload_start + 4];
            let declared_end = payload_start.saturating_add(sz);
            if list_type == b"hdrl" {
                if declared_end <= data.len() && sz >= 12 {
                    has_hdrl = true;
                    let (w, h) = parse_avi_avih_dims(&data[payload_start + 4..declared_end]);
                    width = w;
                    height = h;
                } else {
                    break;
                }
            } else if list_type == b"movi" {
                let truncated = declared_end > data.len();
                movi_info = Some((pos, declared_end, truncated));
                if truncated {
                    break;
                }
            }
        }

        let step = 8usize.saturating_add(sz).saturating_add(sz & 1);
        if step < 8 || pos.checked_add(step).is_none_or(|n| n > data.len()) {
            break;
        }
        pos += step;
    }

    let Some((movi_off, _movi_decl_end, movi_truncated)) = movi_info else {
        return (
            None,
            MediaRepairReport::unrecoverable(
                width,
                height,
                "AVI başlığı (hdrl) veya medya akışı (movi) eksik; oynatılamaz.",
            ),
        );
    };

    if !has_hdrl {
        return (
            None,
            MediaRepairReport::unrecoverable(
                width,
                height,
                "AVI hdrl başlık listesi eksik; oynatılamaz.",
            ),
        );
    }

    let movi_data_start = movi_off + 12;
    if movi_data_start + 8 > data.len() || !has_nontrivial_bytes(&data[movi_data_start..], 4) {
        return (
            None,
            MediaRepairReport::unrecoverable(
                width,
                height,
                "AVI movi medya verisi boş veya sıfırlanmış; oynatılamaz.",
            ),
        );
    }

    let riff_truncated = declared_riff_total > data.len() || declared_riff_total < movi_data_start;
    if !riff_truncated && !movi_truncated {
        let end = declared_riff_total.min(data.len());
        return (
            if build_output {
                Some(data[..end].to_vec())
            } else {
                None
            },
            MediaRepairReport::intact(
                width,
                height,
                "AVI kapsayıcısı (hdrl + movi) sağlam; doğrudan oynatılabilir.",
            ),
        );
    }

    // Truncated AVI -> find clean end inside `movi` (last complete sub-chunk if any, else EOF).
    let mut sub_pos = movi_data_start;
    let mut last_complete_frame = movi_data_start;
    while sub_pos + 8 <= data.len() {
        let sub_cc = &data[sub_pos..sub_pos + 4];
        if !sub_cc
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b' ')
        {
            break;
        }
        let sub_sz = read_u32_le(data, sub_pos + 4).unwrap_or(0) as usize;
        let next = sub_pos
            .saturating_add(8)
            .saturating_add(sub_sz)
            .saturating_add(sub_sz & 1);
        if next <= data.len() && next > sub_pos {
            last_complete_frame = next;
            sub_pos = next;
        } else {
            break;
        }
    }

    let final_end = if last_complete_frame > movi_data_start {
        last_complete_frame
    } else {
        data.len()
    };

    let out = if build_output {
        let mut repaired = data[..final_end].to_vec();
        let new_movi_sz = (repaired.len() - (movi_off + 8)) as u32;
        repaired[movi_off + 4..movi_off + 8].copy_from_slice(&new_movi_sz.to_le_bytes());
        let new_riff_sz = (repaired.len() - 8) as u32;
        repaired[4..8].copy_from_slice(&new_riff_sz.to_le_bytes());
        Some(repaired)
    } else {
        None
    };

    (
        out,
        MediaRepairReport::repaired(
            width,
            height,
            "Kesilmiş AVI RIFF/movi boyutu onarıldı; kurtarılan bölüm oynatılabilir.",
        ),
    )
}

fn process_wav(data: &[u8], build_output: bool) -> (Option<Vec<u8>>, MediaRepairReport) {
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "Geçerli WAV RIFF başlığı bulunamadı; açılamaz.",
            ),
        );
    }

    let declared_riff_total = (read_u32_le(data, 4).unwrap_or(0) as usize).saturating_add(8);
    let mut pos = 12usize;
    let mut has_fmt = false;
    let mut data_chunk: Option<(usize, usize, bool)> = None;

    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let sz = read_u32_le(data, pos + 4).unwrap_or(0) as usize;
        let payload_start = pos + 8;
        if id == b"fmt " && sz >= 16 && payload_start + sz <= data.len() {
            has_fmt = true;
        } else if id == b"data" {
            let truncated = payload_start.saturating_add(sz) > data.len();
            data_chunk = Some((pos, sz, truncated));
            if truncated {
                break;
            }
        }
        let step = 8usize.saturating_add(sz).saturating_add(sz & 1);
        if step < 8 || pos.checked_add(step).is_none_or(|n| n > data.len()) {
            break;
        }
        pos += step;
    }

    let Some((data_off, _decl_sz, data_truncated)) = data_chunk else {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "WAV ses verisi (data) bölümü eksik; oynatılamaz.",
            ),
        );
    };
    if !has_fmt || data_off + 8 >= data.len() {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "WAV biçim başlığı (fmt) eksik; oynatılamaz.",
            ),
        );
    }

    let riff_truncated = declared_riff_total > data.len();
    if !riff_truncated && !data_truncated {
        return (
            if build_output {
                Some(data[..declared_riff_total.min(data.len())].to_vec())
            } else {
                None
            },
            MediaRepairReport::intact(None, None, "WAV ses yapısı sağlam; doğrudan oynatılabilir."),
        );
    }

    let out = if build_output {
        let mut repaired = data.to_vec();
        let new_data_sz = (repaired.len() - (data_off + 8)) as u32;
        repaired[data_off + 4..data_off + 8].copy_from_slice(&new_data_sz.to_le_bytes());
        let new_riff_sz = (repaired.len() - 8) as u32;
        repaired[4..8].copy_from_slice(&new_riff_sz.to_le_bytes());
        Some(repaired)
    } else {
        None
    };

    (
        out,
        MediaRepairReport::repaired(
            None,
            None,
            "Kesilmiş WAV RIFF/data başlığı onarıldı; kurtarılan ses oynatılabilir.",
        ),
    )
}

// ============================================================================
// 8. MP4 / MOV / M4V Box Parser, H.264 Scanner & Full `moov` Synthesizer
// ============================================================================

fn is_known_mp4_top_box(fourcc: &[u8; 4]) -> bool {
    matches!(
        fourcc,
        b"ftyp"
            | b"free"
            | b"skip"
            | b"wide"
            | b"mdat"
            | b"moov"
            | b"meta"
            | b"uuid"
            | b"pdin"
            | b"styp"
            | b"moof"
            | b"mfra"
            | b"sidx"
            | b"ssix"
            | b"prft"
            | b"emsg"
            | b"pnot"
            | b"pict"
            | b"junk"
    )
}

fn is_valid_ftyp_brand(data: &[u8], off: usize) -> bool {
    let Some(brand) = data.get(off..off + 4) else {
        return false;
    };
    brand
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b' ' || b == b'-' || b == b'_')
}

#[derive(Clone, Debug)]
struct Mp4Box {
    fourcc: [u8; 4],
    offset: usize,
    hdr_len: usize,
    declared_size: u64,
    size_was_zero: bool,
    avail_end: usize,
    truncated: bool,
}

fn walk_mp4_top_boxes(data: &[u8]) -> Vec<Mp4Box> {
    let mut boxes = Vec::new();
    let mut pos = 0usize;

    while pos + 8 <= data.len() {
        let Some(size32) = read_u32_be(data, pos).map(u64::from) else {
            break;
        };
        let fourcc: [u8; 4] = match data[pos + 4..pos + 8].try_into() {
            Ok(v) => v,
            Err(_) => break,
        };
        if !is_known_mp4_top_box(&fourcc) {
            break;
        }

        let (declared_size, hdr_len, size_was_zero) = if size32 == 1 {
            let Some(sz64) = read_u64_be(data, pos + 8) else {
                break;
            };
            if sz64 < 16 {
                break;
            }
            (sz64, 16usize, false)
        } else if size32 == 0 {
            let rem = (data.len() - pos) as u64;
            if rem < 8 {
                break;
            }
            (rem, 8usize, true)
        } else {
            if size32 < 8 {
                break;
            }
            (size32, 8usize, false)
        };

        let end_u64 = (pos as u64).saturating_add(declared_size);
        let truncated = end_u64 > data.len() as u64;
        let avail_end = end_u64.min(data.len() as u64) as usize;

        boxes.push(Mp4Box {
            fourcc,
            offset: pos,
            hdr_len,
            declared_size,
            size_was_zero,
            avail_end,
            truncated,
        });

        if truncated || size_was_zero {
            break;
        }
        pos = avail_end;
    }

    boxes
}

struct MoovInfo {
    valid: bool,
    width: Option<u32>,
    height: Option<u32>,
}

fn iter_child_boxes(slice: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= slice.len() {
        let Some(sz32) = read_u32_be(slice, pos).map(u64::from) else {
            break;
        };
        let fourcc: [u8; 4] = match slice[pos + 4..pos + 8].try_into() {
            Ok(v) => v,
            Err(_) => break,
        };
        let (sz, hdr) = if sz32 == 1 {
            let Some(s64) = read_u64_be(slice, pos + 8) else {
                break;
            };
            if s64 < 16 {
                break;
            }
            (s64 as usize, 16usize)
        } else if sz32 == 0 {
            (slice.len() - pos, 8usize)
        } else {
            if sz32 < 8 {
                break;
            }
            (sz32 as usize, 8usize)
        };
        if pos.checked_add(sz).is_none_or(|e| e > slice.len()) {
            break;
        }
        out.push((fourcc, &slice[pos + hdr..pos + sz]));
        pos += sz;
    }
    out
}

fn inspect_moov(moov_payload: &[u8]) -> MoovInfo {
    let mut has_valid_trak = false;
    let mut has_mvex = false;
    let mut width = None;
    let mut height = None;

    for (cc, payload) in iter_child_boxes(moov_payload) {
        if &cc == b"mvex" {
            has_mvex = true;
        } else if &cc == b"trak" {
            let mut has_stsd = false;
            let mut has_stsz = false;
            let mut has_stco = false;

            for (tcc, tpay) in iter_child_boxes(payload) {
                if &tcc == b"tkhd" && !tpay.is_empty() {
                    let ver = tpay[0];
                    let dim_off = if ver == 1 { 88 } else { 76 };
                    if dim_off + 8 <= tpay.len() {
                        let w = read_u32_be(tpay, dim_off).unwrap_or(0) >> 16;
                        let h = read_u32_be(tpay, dim_off + 4).unwrap_or(0) >> 16;
                        if (1..=16_384).contains(&w) && (1..=16_384).contains(&h) && width.is_none()
                        {
                            width = Some(w);
                            height = Some(h);
                        }
                    }
                } else if &tcc == b"mdia" {
                    for (mcc, mpay) in iter_child_boxes(tpay) {
                        if &mcc == b"minf" {
                            for (icc, ipay) in iter_child_boxes(mpay) {
                                if &icc == b"stbl" {
                                    for (scc, spay) in iter_child_boxes(ipay) {
                                        match &scc {
                                            b"stsd" if spay.len() >= 16 => {
                                                has_stsd = true;
                                                // Fallback VisualSampleEntry width/height
                                                if width.is_none() && spay.len() >= 8 + 36 {
                                                    let entry = &spay[8..];
                                                    if entry.len() >= 36
                                                        && matches!(
                                                            &entry[4..8],
                                                            b"avc1"
                                                                | b"avc3"
                                                                | b"hev1"
                                                                | b"hvc1"
                                                                | b"mp4v"
                                                        )
                                                    {
                                                        let w = read_u16_be(entry, 32).unwrap_or(0)
                                                            as u32;
                                                        let h = read_u16_be(entry, 34).unwrap_or(0)
                                                            as u32;
                                                        if (1..=16_384).contains(&w)
                                                            && (1..=16_384).contains(&h)
                                                        {
                                                            width = Some(w);
                                                            height = Some(h);
                                                        }
                                                    }
                                                }
                                            }
                                            b"stsz" | b"stz2" => has_stsz = true,
                                            b"stco" | b"co64" => has_stco = true,
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if has_stsd && ((has_stsz && has_stco) || has_mvex) {
                has_valid_trak = true;
            }
        }
    }

    MoovInfo {
        valid: has_valid_trak,
        width,
        height,
    }
}

// --- H.264 BitReader, Exp-Golomb SPS Parser, and NAL Scanner ---

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bit(&mut self) -> Option<u32> {
        let byte_idx = self.bit_pos / 8;
        if byte_idx >= self.data.len() {
            return None;
        }
        let bit_idx = 7 - (self.bit_pos % 8);
        self.bit_pos += 1;
        Some(u32::from((self.data[byte_idx] >> bit_idx) & 1))
    }

    fn read_bits(&mut self, n: usize) -> Option<u32> {
        if n > 32 {
            return None;
        }
        let mut val = 0u32;
        for _ in 0..n {
            val = (val << 1) | self.read_bit()?;
        }
        Some(val)
    }

    fn read_ue(&mut self) -> Option<u32> {
        let mut zeros = 0usize;
        while self.read_bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        if zeros == 0 {
            return Some(0);
        }
        let suffix = self.read_bits(zeros)?;
        ((1u32 << zeros) - 1).checked_add(suffix)
    }

    fn read_se(&mut self) -> Option<i32> {
        let u = self.read_ue()?;
        let mag = (u.div_ceil(2)) as i32;
        if (u & 1) == 1 {
            Some(mag)
        } else {
            Some(-mag)
        }
    }
}

fn rbsp_from_nal(nal_body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal_body.len());
    let mut zeros = 0usize;
    for &b in nal_body {
        if zeros >= 2 && b == 0x03 {
            zeros = 0;
            continue;
        }
        if b == 0x00 {
            zeros += 1;
        } else {
            zeros = 0;
        }
        out.push(b);
    }
    out
}

struct SpsInfo {
    profile_idc: u8,
    profile_compat: u8,
    level_idc: u8,
    width: u32,
    height: u32,
}

fn is_valid_h264_profile(profile_idc: u8) -> bool {
    matches!(
        profile_idc,
        44 | 66 | 77 | 83 | 86 | 88 | 100 | 110 | 118 | 122 | 128 | 134 | 138 | 139 | 244
    )
}

fn is_valid_h264_level(level_idc: u8) -> bool {
    matches!(
        level_idc,
        9 | 10
            | 11
            | 12
            | 13
            | 20
            | 21
            | 22
            | 30
            | 31
            | 32
            | 40
            | 41
            | 42
            | 50
            | 51
            | 52
            | 60
            | 61
            | 62
    )
}

fn looks_like_h264_sps(nal: &[u8]) -> bool {
    if nal.len() < 4 || nal.len() > 512 {
        return false;
    }
    let b0 = nal[0];
    if (b0 & 0x80) != 0 || (b0 & 0x1F) != 7 || ((b0 >> 5) & 0x03) == 0 {
        return false;
    }
    is_valid_h264_profile(nal[1]) && (nal[2] & 0x03) == 0 && is_valid_h264_level(nal[3])
}

fn looks_like_h264_pps(nal: &[u8]) -> bool {
    if nal.len() < 2 || nal.len() > 256 {
        return false;
    }
    let b0 = nal[0];
    (b0 & 0x80) == 0 && (b0 & 0x1F) == 8 && ((b0 >> 5) & 0x03) != 0
}

fn parse_h264_sps(nal: &[u8]) -> SpsInfo {
    let profile_idc = *nal.get(1).unwrap_or(&66);
    let profile_compat = *nal.get(2).unwrap_or(&0xC0);
    let level_idc = *nal.get(3).unwrap_or(&31);
    let fallback = SpsInfo {
        profile_idc,
        profile_compat,
        level_idc,
        width: 1280,
        height: 720,
    };

    if nal.len() < 5 {
        return fallback;
    }
    let rbsp = rbsp_from_nal(&nal[1..]);
    let mut br = BitReader::new(&rbsp);

    let mut parse_dims = || -> Option<(u32, u32)> {
        let p_idc = br.read_bits(8)? as u8;
        let _compat = br.read_bits(8)?;
        let _level = br.read_bits(8)?;
        let _sps_id = br.read_ue()?;

        let mut chroma_format_idc = 1u32;
        let mut separate_colour_plane_flag = 0u32;
        if matches!(
            p_idc,
            100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134
        ) {
            chroma_format_idc = br.read_ue()?;
            if chroma_format_idc == 3 {
                separate_colour_plane_flag = br.read_bit()?;
            }
            let _bit_depth_luma = br.read_ue()?;
            let _bit_depth_chroma = br.read_ue()?;
            let _qpprime = br.read_bit()?;
            let scaling_present = br.read_bit()?;
            if scaling_present == 1 {
                let count = if chroma_format_idc != 3 { 8 } else { 12 };
                for i in 0..count {
                    if br.read_bit()? == 1 {
                        let list_sz = if i < 6 { 16 } else { 64 };
                        let mut last_scale = 8i32;
                        let mut next_scale = 8i32;
                        for _ in 0..list_sz {
                            if next_scale != 0 {
                                let delta = br.read_se()?;
                                next_scale = (last_scale + delta + 256) % 256;
                            }
                            last_scale = if next_scale == 0 {
                                last_scale
                            } else {
                                next_scale
                            };
                        }
                    }
                }
            }
        }

        let _log2_max_frame_num_m4 = br.read_ue()?;
        let poc_type = br.read_ue()?;
        if poc_type == 0 {
            let _log2_max_poc_lsb_m4 = br.read_ue()?;
        } else if poc_type == 1 {
            let _delta_always_zero = br.read_bit()?;
            let _off_non_ref = br.read_se()?;
            let _off_top_bot = br.read_se()?;
            let cycle_len = br.read_ue()?.min(256);
            for _ in 0..cycle_len {
                let _ = br.read_se()?;
            }
        }

        let _max_num_ref_frames = br.read_ue()?;
        let _gaps_allowed = br.read_bit()?;
        let width_mbs_m1 = br.read_ue()?;
        let height_map_m1 = br.read_ue()?;
        let frame_mbs_only = br.read_bit()?;
        if frame_mbs_only == 0 {
            let _mb_adaptive = br.read_bit()?;
        }
        let _direct_8x8 = br.read_bit()?;
        let cropping = br.read_bit()?;
        let (crop_l, crop_r, crop_t, crop_b) = if cropping == 1 {
            (br.read_ue()?, br.read_ue()?, br.read_ue()?, br.read_ue()?)
        } else {
            (0, 0, 0, 0)
        };

        let raw_w = width_mbs_m1.checked_add(1)?.checked_mul(16)?;
        let raw_h = (2 - frame_mbs_only)
            .checked_mul(height_map_m1.checked_add(1)?)?
            .checked_mul(16)?;
        let (crop_ux, crop_uy) = if chroma_format_idc == 0 || separate_colour_plane_flag == 1 {
            (1, 2 - frame_mbs_only)
        } else if chroma_format_idc == 1 {
            (2, 2 * (2 - frame_mbs_only))
        } else if chroma_format_idc == 2 {
            (2, 2 - frame_mbs_only)
        } else {
            (1, 2 - frame_mbs_only)
        };

        let w = raw_w.checked_sub(crop_l.checked_add(crop_r)?.checked_mul(crop_ux)?)?;
        let h = raw_h.checked_sub(crop_t.checked_add(crop_b)?.checked_mul(crop_uy)?)?;
        if (16..=16_384).contains(&w) && (16..=16_384).contains(&h) {
            Some((w, h))
        } else {
            None
        }
    };

    match parse_dims() {
        Some((width, height)) => SpsInfo {
            profile_idc,
            profile_compat,
            level_idc,
            width,
            height,
        },
        None => fallback,
    }
}

#[derive(Clone, Debug)]
struct VideoSample {
    offset: u64,
    size: u32,
    is_sync: bool,
}

struct RecoveredH264Stream {
    sps: Vec<u8>,
    pps: Vec<u8>,
    samples: Vec<VideoSample>,
    is_annex_b: bool,
    annex_b_frames: Vec<(Vec<u8>, bool)>, // (length-prefixed NAL bytes, is_sync) when converted from Annex-B
}

fn is_plausible_avcc_nal(mdat: &[u8], pos: usize) -> Option<(usize, u8)> {
    let nal_len = read_u32_be(mdat, pos)? as usize;
    if nal_len == 0 || nal_len > 16 * 1024 * 1024 || pos + 4 + nal_len > mdat.len() {
        return None;
    }
    let header = mdat[pos + 4];
    if (header & 0x80) != 0 {
        return None;
    }
    let nal_type = header & 0x1F;
    let nal_ref_idc = (header >> 5) & 0x03;
    let nal = &mdat[pos + 4..pos + 4 + nal_len];

    let valid = match nal_type {
        7 => looks_like_h264_sps(nal),
        8 => looks_like_h264_pps(nal),
        5 => nal_ref_idc != 0 && nal_len >= 4,
        1 => nal_len >= 4,
        6 | 9 | 10 | 11 | 12 => nal_len <= 65_536,
        _ => false,
    };
    valid.then_some((nal_len, nal_type))
}

fn scan_mdat_h264_avcc(mdat: &[u8], mdat_file_payload_off: u64) -> Option<RecoveredH264Stream> {
    let mut sps: Option<Vec<u8>> = None;
    let mut pps: Option<Vec<u8>> = None;
    let mut samples: Vec<VideoSample> = Vec::new();
    let mut has_idr = false;

    let mut pos = 0usize;
    let mut in_sync_chain = true; // Offset 0 starts in strict AVCC chain mode.

    while pos + 5 <= mdat.len() {
        if let Some((nal_len, nal_type)) = is_plausible_avcc_nal(mdat, pos) {
            let next_pos = pos + 4 + nal_len;
            // Outside an active contiguous AVCC chain, require either validated SPS/PPS or
            // chain confirmation (another valid NAL immediately following, or clean EOF).
            let confirmed = in_sync_chain
                || matches!(nal_type, 7 | 8)
                || next_pos == mdat.len()
                || is_plausible_avcc_nal(mdat, next_pos).is_some();

            if confirmed {
                let nal = &mdat[pos + 4..next_pos];
                match nal_type {
                    7 => {
                        if sps.is_none() {
                            sps = Some(nal.to_vec());
                        }
                    }
                    8 => {
                        if pps.is_none() {
                            pps = Some(nal.to_vec());
                        }
                    }
                    5 => {
                        has_idr = true;
                        samples.push(VideoSample {
                            offset: mdat_file_payload_off + pos as u64,
                            size: (4 + nal_len) as u32,
                            is_sync: true,
                        });
                    }
                    1 => {
                        samples.push(VideoSample {
                            offset: mdat_file_payload_off + pos as u64,
                            size: (4 + nal_len) as u32,
                            is_sync: false,
                        });
                    }
                    _ => {}
                }
                pos = next_pos;
                in_sync_chain = true;
                continue;
            }
        }

        // Lost contiguous AVCC sync (e.g. interleaved AAC audio packet): scan forward for next NAL.
        in_sync_chain = false;
        pos += 1;
    }

    if let (Some(sps_bytes), Some(pps_bytes)) = (sps, pps) {
        if has_idr && !samples.is_empty() {
            return Some(RecoveredH264Stream {
                sps: sps_bytes,
                pps: pps_bytes,
                samples,
                is_annex_b: false,
                annex_b_frames: Vec::new(),
            });
        }
    }
    None
}

fn find_annex_b_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 3 < data.len() {
        if data[i] == 0x00 && data[i + 1] == 0x00 {
            if data[i + 2] == 0x01 {
                return Some((i, 3));
            }
            if i + 3 < data.len() && data[i + 2] == 0x00 && data[i + 3] == 0x01 {
                return Some((i, 4));
            }
        }
        i += 1;
    }
    None
}

fn scan_mdat_h264_annex_b(mdat: &[u8]) -> Option<RecoveredH264Stream> {
    let mut sps: Option<Vec<u8>> = None;
    let mut pps: Option<Vec<u8>> = None;
    let mut frames: Vec<(Vec<u8>, bool)> = Vec::new();
    let mut has_idr = false;

    let mut cur = find_annex_b_start_code(mdat, 0);
    while let Some((sc_pos, sc_len)) = cur {
        let nal_start = sc_pos + sc_len;
        let next = find_annex_b_start_code(mdat, nal_start);
        let nal_end = next.map(|(p, _)| p).unwrap_or(mdat.len());
        if nal_end > nal_start {
            let nal = &mdat[nal_start..nal_end];
            let b0 = nal[0];
            if (b0 & 0x80) == 0 {
                let nal_type = b0 & 0x1F;
                let nal_ref = (b0 >> 5) & 0x03;
                match nal_type {
                    7 if looks_like_h264_sps(nal) => {
                        if sps.is_none() {
                            sps = Some(nal.to_vec());
                        }
                    }
                    8 if looks_like_h264_pps(nal) => {
                        if pps.is_none() {
                            pps = Some(nal.to_vec());
                        }
                    }
                    5 if nal_ref != 0 && nal.len() >= 4 => {
                        has_idr = true;
                        let mut avcc = Vec::with_capacity(4 + nal.len());
                        avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                        avcc.extend_from_slice(nal);
                        frames.push((avcc, true));
                    }
                    1 if nal.len() >= 4 => {
                        let mut avcc = Vec::with_capacity(4 + nal.len());
                        avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                        avcc.extend_from_slice(nal);
                        frames.push((avcc, false));
                    }
                    _ => {}
                }
            }
        }
        cur = next;
    }

    if let (Some(sps_bytes), Some(pps_bytes)) = (sps, pps) {
        if has_idr && !frames.is_empty() {
            return Some(RecoveredH264Stream {
                sps: sps_bytes,
                pps: pps_bytes,
                samples: Vec::new(),
                is_annex_b: true,
                annex_b_frames: frames,
            });
        }
    }
    None
}

// --- ISO-BMFF `moov` Box Synthesizer ---

fn mp4_box(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let size = (8 + payload.len()) as u32;
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(fourcc);
    out.extend_from_slice(payload);
    out
}

fn mp4_full_box(fourcc: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut full = Vec::with_capacity(4 + payload.len());
    full.push(version);
    full.push(((flags >> 16) & 0xFF) as u8);
    full.push(((flags >> 8) & 0xFF) as u8);
    full.push((flags & 0xFF) as u8);
    full.extend_from_slice(payload);
    mp4_box(fourcc, &full)
}

fn identity_matrix_bytes() -> [u8; 36] {
    let vals: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];
    let mut out = [0u8; 36];
    for (i, v) in vals.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn synthesize_moov(sps_info: &SpsInfo, sps: &[u8], pps: &[u8], samples: &[VideoSample]) -> Vec<u8> {
    const TIMESCALE: u32 = 30_000;
    const SAMPLE_DELTA: u32 = 1_000; // 30 fps
    let sample_count = samples.len() as u32;
    let duration = sample_count.saturating_mul(SAMPLE_DELTA);
    let matrix = identity_matrix_bytes();

    // 1. mvhd
    let mut mvhd_pay = Vec::with_capacity(96);
    mvhd_pay.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    mvhd_pay.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    mvhd_pay.extend_from_slice(&TIMESCALE.to_be_bytes());
    mvhd_pay.extend_from_slice(&duration.to_be_bytes());
    mvhd_pay.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
    mvhd_pay.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    mvhd_pay.extend_from_slice(&[0u8; 10]); // reserved
    mvhd_pay.extend_from_slice(&matrix);
    mvhd_pay.extend_from_slice(&[0u8; 24]); // pre_defined
    mvhd_pay.extend_from_slice(&2u32.to_be_bytes()); // next_track_id
    let mvhd = mp4_full_box(b"mvhd", 0, 0, &mvhd_pay);

    // 2. tkhd (flags = 0x03: track_enabled | track_in_movie)
    let mut tkhd_pay = Vec::with_capacity(80);
    tkhd_pay.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    tkhd_pay.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    tkhd_pay.extend_from_slice(&1u32.to_be_bytes()); // track_id
    tkhd_pay.extend_from_slice(&0u32.to_be_bytes()); // reserved
    tkhd_pay.extend_from_slice(&duration.to_be_bytes());
    tkhd_pay.extend_from_slice(&[0u8; 8]); // reserved
    tkhd_pay.extend_from_slice(&0u16.to_be_bytes()); // layer
    tkhd_pay.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
    tkhd_pay.extend_from_slice(&0u16.to_be_bytes()); // volume (0 for video)
    tkhd_pay.extend_from_slice(&0u16.to_be_bytes()); // reserved
    tkhd_pay.extend_from_slice(&matrix);
    tkhd_pay.extend_from_slice(&(sps_info.width << 16).to_be_bytes());
    tkhd_pay.extend_from_slice(&(sps_info.height << 16).to_be_bytes());
    let tkhd = mp4_full_box(b"tkhd", 0, 0x0000_0003, &tkhd_pay);

    // 3. mdhd
    let mut mdhd_pay = Vec::with_capacity(20);
    mdhd_pay.extend_from_slice(&0u32.to_be_bytes());
    mdhd_pay.extend_from_slice(&0u32.to_be_bytes());
    mdhd_pay.extend_from_slice(&TIMESCALE.to_be_bytes());
    mdhd_pay.extend_from_slice(&duration.to_be_bytes());
    mdhd_pay.extend_from_slice(&0x55C4u16.to_be_bytes()); // "und"
    mdhd_pay.extend_from_slice(&0u16.to_be_bytes());
    let mdhd = mp4_full_box(b"mdhd", 0, 0, &mdhd_pay);

    // 4. hdlr ("vide")
    let mut hdlr_pay = Vec::with_capacity(33);
    hdlr_pay.extend_from_slice(&0u32.to_be_bytes());
    hdlr_pay.extend_from_slice(b"vide");
    hdlr_pay.extend_from_slice(&[0u8; 12]);
    hdlr_pay.extend_from_slice(b"VideoHandler\0");
    let hdlr = mp4_full_box(b"hdlr", 0, 0, &hdlr_pay);

    // 5. vmhd (flags = 1 required by ISO 14496-12)
    let vmhd = mp4_full_box(b"vmhd", 0, 0x0000_0001, &[0u8; 8]);

    // 6. dinf -> dref -> url (self-contained flag = 1)
    let url_box = mp4_full_box(b"url ", 0, 0x0000_0001, &[]);
    let mut dref_pay = Vec::with_capacity(4 + url_box.len());
    dref_pay.extend_from_slice(&1u32.to_be_bytes());
    dref_pay.extend_from_slice(&url_box);
    let dref = mp4_full_box(b"dref", 0, 0, &dref_pay);
    let dinf = mp4_box(b"dinf", &dref);

    // 7. stsd -> avc1 -> avcC
    let mut avcc_pay = Vec::with_capacity(11 + sps.len() + pps.len());
    avcc_pay.push(1); // configurationVersion
    avcc_pay.push(sps_info.profile_idc);
    avcc_pay.push(sps_info.profile_compat);
    avcc_pay.push(sps_info.level_idc);
    avcc_pay.push(0xFF); // 4-byte NAL length prefix (lengthSizeMinusOne = 3)
    avcc_pay.push(0xE1); // 1 SPS
    avcc_pay.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc_pay.extend_from_slice(sps);
    avcc_pay.push(1); // 1 PPS
    avcc_pay.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc_pay.extend_from_slice(pps);
    let avcc = mp4_box(b"avcC", &avcc_pay);

    let mut avc1_pay = Vec::with_capacity(78 + avcc.len());
    avc1_pay.extend_from_slice(&[0u8; 6]); // reserved
    avc1_pay.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    avc1_pay.extend_from_slice(&[0u8; 16]); // pre_defined + reserved
    avc1_pay.extend_from_slice(&(sps_info.width as u16).to_be_bytes());
    avc1_pay.extend_from_slice(&(sps_info.height as u16).to_be_bytes());
    avc1_pay.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi horiz
    avc1_pay.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi vert
    avc1_pay.extend_from_slice(&0u32.to_be_bytes()); // reserved
    avc1_pay.extend_from_slice(&1u16.to_be_bytes()); // frame_count
    avc1_pay.extend_from_slice(&[0u8; 32]); // compressorname
    avc1_pay.extend_from_slice(&0x0018u16.to_be_bytes()); // depth 24
    avc1_pay.extend_from_slice(&0xFFFFu16.to_be_bytes()); // pre_defined = -1
    avc1_pay.extend_from_slice(&avcc);
    let avc1 = mp4_box(b"avc1", &avc1_pay);

    let mut stsd_pay = Vec::with_capacity(4 + avc1.len());
    stsd_pay.extend_from_slice(&1u32.to_be_bytes());
    stsd_pay.extend_from_slice(&avc1);
    let stsd = mp4_full_box(b"stsd", 0, 0, &stsd_pay);

    // 8. stts (30 fps uniform)
    let mut stts_pay = Vec::with_capacity(12);
    stts_pay.extend_from_slice(&1u32.to_be_bytes());
    stts_pay.extend_from_slice(&sample_count.to_be_bytes());
    stts_pay.extend_from_slice(&SAMPLE_DELTA.to_be_bytes());
    let stts = mp4_full_box(b"stts", 0, 0, &stts_pay);

    // 9. stss (sync/IDR keyframes, 1-based)
    let sync_indices: Vec<u32> = samples
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.is_sync.then_some((i + 1) as u32))
        .collect();
    let mut stss_pay = Vec::with_capacity(4 + sync_indices.len() * 4);
    stss_pay.extend_from_slice(&(sync_indices.len() as u32).to_be_bytes());
    for idx in &sync_indices {
        stss_pay.extend_from_slice(&idx.to_be_bytes());
    }
    let stss = mp4_full_box(b"stss", 0, 0, &stss_pay);

    // 10. stsc (1 sample per chunk -> allows interleaved/non-contiguous frame offsets cleanly)
    let mut stsc_pay = Vec::with_capacity(16);
    stsc_pay.extend_from_slice(&1u32.to_be_bytes());
    stsc_pay.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
    stsc_pay.extend_from_slice(&1u32.to_be_bytes()); // samples_per_chunk
    stsc_pay.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
    let stsc = mp4_full_box(b"stsc", 0, 0, &stsc_pay);

    // 11. stsz
    let mut stsz_pay = Vec::with_capacity(8 + samples.len() * 4);
    stsz_pay.extend_from_slice(&0u32.to_be_bytes()); // variable sizes
    stsz_pay.extend_from_slice(&sample_count.to_be_bytes());
    for s in samples {
        stsz_pay.extend_from_slice(&s.size.to_be_bytes());
    }
    let stsz = mp4_full_box(b"stsz", 0, 0, &stsz_pay);

    // 12. stco / co64
    let needs_co64 = samples.iter().any(|s| s.offset > u64::from(u32::MAX));
    let chunk_offsets = if needs_co64 {
        let mut co64_pay = Vec::with_capacity(4 + samples.len() * 8);
        co64_pay.extend_from_slice(&sample_count.to_be_bytes());
        for s in samples {
            co64_pay.extend_from_slice(&s.offset.to_be_bytes());
        }
        mp4_full_box(b"co64", 0, 0, &co64_pay)
    } else {
        let mut stco_pay = Vec::with_capacity(4 + samples.len() * 4);
        stco_pay.extend_from_slice(&sample_count.to_be_bytes());
        for s in samples {
            stco_pay.extend_from_slice(&(s.offset as u32).to_be_bytes());
        }
        mp4_full_box(b"stco", 0, 0, &stco_pay)
    };

    // Assemble stbl -> minf -> mdia -> trak -> moov
    let mut stbl_pay = Vec::new();
    stbl_pay.extend_from_slice(&stsd);
    stbl_pay.extend_from_slice(&stts);
    stbl_pay.extend_from_slice(&stss);
    stbl_pay.extend_from_slice(&stsc);
    stbl_pay.extend_from_slice(&stsz);
    stbl_pay.extend_from_slice(&chunk_offsets);
    let stbl = mp4_box(b"stbl", &stbl_pay);

    let mut minf_pay = Vec::new();
    minf_pay.extend_from_slice(&vmhd);
    minf_pay.extend_from_slice(&dinf);
    minf_pay.extend_from_slice(&stbl);
    let minf = mp4_box(b"minf", &minf_pay);

    let mut mdia_pay = Vec::new();
    mdia_pay.extend_from_slice(&mdhd);
    mdia_pay.extend_from_slice(&hdlr);
    mdia_pay.extend_from_slice(&minf);
    let mdia = mp4_box(b"mdia", &mdia_pay);

    let mut trak_pay = Vec::new();
    trak_pay.extend_from_slice(&tkhd);
    trak_pay.extend_from_slice(&mdia);
    let trak = mp4_box(b"trak", &trak_pay);

    let mut moov_pay = Vec::new();
    moov_pay.extend_from_slice(&mvhd);
    moov_pay.extend_from_slice(&trak);
    mp4_box(b"moov", &moov_pay)
}

fn patch_mdat_size_header(buf: &mut [u8], mdat: &Mp4Box, new_box_len: u64) {
    if mdat.hdr_len == 16 && mdat.offset + 16 <= buf.len() {
        buf[mdat.offset..mdat.offset + 4].copy_from_slice(&1u32.to_be_bytes());
        buf[mdat.offset + 8..mdat.offset + 16].copy_from_slice(&new_box_len.to_be_bytes());
    } else if mdat.offset + 4 <= buf.len() {
        buf[mdat.offset..mdat.offset + 4].copy_from_slice(&(new_box_len as u32).to_be_bytes());
    }
}

fn process_mp4(data: &[u8], build_output: bool) -> (Option<Vec<u8>>, MediaRepairReport) {
    let boxes = walk_mp4_top_boxes(data);
    let ftyp = boxes.iter().find(|b| &b.fourcc == b"ftyp" && !b.truncated);
    let Some(ftyp_box) = ftyp else {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "Geçerli MP4/MOV ftyp başlığı bulunamadı; oynatılamaz.",
            ),
        );
    };
    if !is_valid_ftyp_brand(data, ftyp_box.offset + ftyp_box.hdr_len) {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "MP4/MOV ftyp marka kodu geçersiz; oynatılamaz.",
            ),
        );
    }

    let moov_box = boxes.iter().find(|b| &b.fourcc == b"moov" && !b.truncated);
    let moov_info = moov_box.map(|m| inspect_moov(&data[m.offset + m.hdr_len..m.avail_end]));

    // Pick the largest mdat box (with non-empty payload).
    let mdat_box = boxes
        .iter()
        .filter(|b| &b.fourcc == b"mdat" && b.avail_end > b.offset + b.hdr_len)
        .max_by_key(|b| b.avail_end - (b.offset + b.hdr_len));

    let Some(mdat) = mdat_box else {
        return (
            None,
            MediaRepairReport::unrecoverable(
                moov_info.as_ref().and_then(|m| m.width),
                moov_info.as_ref().and_then(|m| m.height),
                "MP4/MOV video verisi (mdat) bulunamadı; oynatılamaz.",
            ),
        );
    };

    let mdat_payload = &data[mdat.offset + mdat.hdr_len..mdat.avail_end];
    if mdat_payload.len() < 8 || !has_nontrivial_bytes(mdat_payload, 4) {
        return (
            None,
            MediaRepairReport::unrecoverable(
                moov_info.as_ref().and_then(|m| m.width),
                moov_info.as_ref().and_then(|m| m.height),
                "Video başlığı (moov) ve H.264 kare yapısı yok; oynatılamaz.",
            ),
        );
    }

    // Case 1 & 2: Valid intact `moov` exists!
    if let (Some(moov), Some(info)) = (moov_box, moov_info.as_ref().filter(|i| i.valid)) {
        if !mdat.truncated {
            let last_end = boxes.last().map(|b| b.avail_end).unwrap_or(data.len());
            let out = if build_output {
                Some(data[..last_end].to_vec())
            } else {
                None
            };
            return (
                out,
                MediaRepairReport::intact(
                    info.width,
                    info.height,
                    "MP4/MOV yapısı (ftyp + moov + mdat) sağlam; doğrudan oynatılabilir.",
                ),
            );
        }

        // Fast-start MP4 (`moov` before truncated `mdat`): patch `mdat` box length to exact EOF.
        if moov.offset < mdat.offset {
            let out = if build_output {
                let mut repaired = data[..mdat.avail_end].to_vec();
                let new_mdat_len = (repaired.len() - mdat.offset) as u64;
                patch_mdat_size_header(&mut repaired, mdat, new_mdat_len);
                Some(repaired)
            } else {
                None
            };
            return (
                out,
                MediaRepairReport::repaired(
                    info.width,
                    info.height,
                    "Kesilmiş MP4 mdat boyutu onarıldı; kurtarılan kısım oynatılabilir.",
                ),
            );
        }
    }

    // Case 3: `moov` is MISSING or broken -> scan `mdat` for H.264 SPS + PPS + IDR/slice frames
    // and synthesize a brand-new ISO-BMFF `moov` index!
    let mdat_payload_file_off = (mdat.offset + mdat.hdr_len) as u64;
    let recovered = scan_mdat_h264_avcc(mdat_payload, mdat_payload_file_off)
        .or_else(|| scan_mdat_h264_annex_b(mdat_payload));

    if let Some(stream) = recovered {
        let sps_info = parse_h264_sps(&stream.sps);
        let frame_count = if stream.is_annex_b {
            stream.annex_b_frames.len()
        } else {
            stream.samples.len()
        };

        let out = if build_output {
            if !stream.is_annex_b {
                // Keep everything up to `mdat.avail_end`, patch `mdat` size, append synthesized `moov`.
                let mut repaired = data[..mdat.avail_end].to_vec();
                let new_mdat_len = (repaired.len() - mdat.offset) as u64;
                if mdat.truncated || mdat.size_was_zero || mdat.declared_size != new_mdat_len {
                    patch_mdat_size_header(&mut repaired, mdat, new_mdat_len);
                }
                let moov_bytes =
                    synthesize_moov(&sps_info, &stream.sps, &stream.pps, &stream.samples);
                repaired.extend_from_slice(&moov_bytes);
                Some(repaired)
            } else {
                // Rebuild `mdat` with AVCC 4-byte length-prefixed frames converted from Annex-B.
                let prefix_end = ftyp_box.avail_end;
                let mut mdat_body = Vec::new();
                let mut samples = Vec::with_capacity(stream.annex_b_frames.len());
                let new_mdat_payload_start = (prefix_end + 8) as u64;
                for (frame_bytes, is_sync) in &stream.annex_b_frames {
                    let off = new_mdat_payload_start + mdat_body.len() as u64;
                    mdat_body.extend_from_slice(frame_bytes);
                    samples.push(VideoSample {
                        offset: off,
                        size: frame_bytes.len() as u32,
                        is_sync: *is_sync,
                    });
                }
                let new_mdat = mp4_box(b"mdat", &mdat_body);
                let moov_bytes = synthesize_moov(&sps_info, &stream.sps, &stream.pps, &samples);
                let mut repaired =
                    Vec::with_capacity(prefix_end + new_mdat.len() + moov_bytes.len());
                repaired.extend_from_slice(&data[..prefix_end]);
                repaired.extend_from_slice(&new_mdat);
                repaired.extend_from_slice(&moov_bytes);
                Some(repaired)
            }
        } else {
            None
        };

        return (
            out,
            MediaRepairReport::repaired(
                Some(sps_info.width),
                Some(sps_info.height),
                format!(
                    "Eksik MP4 moov dizini H.264 karelerinden yeniden oluşturuldu ({frame_count} kare oynatılabilir)."
                ),
            ),
        );
    }

    (
        None,
        MediaRepairReport::unrecoverable(
            None,
            None,
            "Video başlığı (moov) ve H.264 kare yapısı yok; oynatılamaz.",
        ),
    )
}

// ============================================================================
// 9. MKV / WEBM (EBML) Inspection & Active Truncation Repair
// ============================================================================

fn read_ebml_id(data: &[u8], pos: usize) -> Option<(u32, usize)> {
    let first = *data.get(pos)?;
    if first == 0 {
        return None;
    }
    let width = first.leading_zeros() as usize + 1;
    if width > 4 || pos + width > data.len() {
        return None;
    }
    let mut id = 0u32;
    for i in 0..width {
        id = (id << 8) | u32::from(data[pos + i]);
    }
    Some((id, width))
}

/// Returns `(Option<size>, vint_width)` where `None` means EBML "unknown size" (all 1-bits).
fn read_ebml_size(data: &[u8], pos: usize) -> Option<(Option<u64>, usize)> {
    let first = *data.get(pos)?;
    if first == 0 {
        return None;
    }
    let width = first.leading_zeros() as usize + 1;
    if width > 8 || pos + width > data.len() {
        return None;
    }
    let mask = if width == 8 {
        0u8
    } else {
        (1u8 << (8 - width)) - 1
    };
    let mut val = u64::from(first & mask);
    let mut all_ones = (first & mask) == mask;
    for i in 1..width {
        let b = data[pos + i];
        if b != 0xFF {
            all_ones = false;
        }
        val = (val << 8) | u64::from(b);
    }
    if all_ones {
        Some((None, width))
    } else {
        Some((Some(val), width))
    }
}

fn write_ebml_unknown_vint(buf: &mut [u8], pos: usize, width: usize) {
    if width == 0 || width > 8 || pos + width > buf.len() {
        return;
    }
    let lead = if width == 8 {
        0x01u8
    } else {
        (1u8 << (8 - width)) | ((1u8 << (8 - width)) - 1)
    };
    buf[pos] = lead;
    for b in &mut buf[pos + 1..pos + width] {
        *b = 0xFF;
    }
}

fn parse_mkv_video_dims(tracks_payload: &[u8]) -> (Option<u32>, Option<u32>) {
    let mut w = None;
    let mut h = None;
    // Scan inside Tracks for Video (0xE0) -> PixelWidth (0xB0) & PixelHeight (0xBA)
    let mut pos = 0usize;
    while pos + 2 < tracks_payload.len() {
        if let Some((id, id_len)) = read_ebml_id(tracks_payload, pos) {
            if let Some((Some(sz), sz_len)) = read_ebml_size(tracks_payload, pos + id_len) {
                let body_start = pos + id_len + sz_len;
                let body_end = body_start.saturating_add(sz as usize);
                if body_end <= tracks_payload.len() {
                    if id == 0xAE || id == 0xE0 {
                        let (sw, sh) = parse_mkv_video_dims(&tracks_payload[body_start..body_end]);
                        if w.is_none() {
                            w = sw;
                        }
                        if h.is_none() {
                            h = sh;
                        }
                    } else if (id == 0xB0 || id == 0xBA) && (1..=4).contains(&sz) {
                        let mut v = 0u32;
                        for &b in &tracks_payload[body_start..body_end] {
                            v = (v << 8) | u32::from(b);
                        }
                        if (1..=16_384).contains(&v) {
                            if id == 0xB0 {
                                w = Some(v);
                            } else {
                                h = Some(v);
                            }
                        }
                    }
                    pos = body_end;
                    continue;
                }
            }
        }
        pos += 1;
    }
    (w, h)
}

fn process_mkv(data: &[u8], build_output: bool) -> (Option<Vec<u8>>, MediaRepairReport) {
    if data.len() < 24 || !data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "Geçerli MKV/WebM EBML başlığı bulunamadı; oynatılamaz.",
            ),
        );
    }

    let Some((Some(ebml_sz), ebml_vint_w)) = read_ebml_size(data, 4) else {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "MKV/WebM EBML boyutu geçersiz; oynatılamaz.",
            ),
        );
    };
    let seg_search_start = 4 + ebml_vint_w + ebml_sz as usize;
    if seg_search_start + 8 > data.len() {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "MKV/WebM Segment başlığı eksik; oynatılamaz.",
            ),
        );
    }

    // Locate Segment (18 53 80 67)
    let seg_pos = if data[seg_search_start..].starts_with(&[0x18, 0x53, 0x80, 0x67]) {
        seg_search_start
    } else if let Some(rel) = data[seg_search_start..data.len().min(seg_search_start + 64)]
        .windows(4)
        .position(|w| w == [0x18, 0x53, 0x80, 0x67])
    {
        seg_search_start + rel
    } else {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "MKV/WebM Segment (18 53 80 67) bulunamadı; oynatılamaz.",
            ),
        );
    };

    let Some((seg_sz_opt, seg_vint_w)) = read_ebml_size(data, seg_pos + 4) else {
        return (
            None,
            MediaRepairReport::unrecoverable(
                None,
                None,
                "MKV/WebM Segment boyutu geçersiz; oynatılamaz.",
            ),
        );
    };

    let seg_body_start = seg_pos + 4 + seg_vint_w;
    let seg_truncated =
        seg_sz_opt.is_some_and(|sz| (seg_body_start as u64).saturating_add(sz) > data.len() as u64);

    // Walk Level-1 elements inside Segment for Tracks (16 54 AE 6B) and Cluster (1F 43 B6 75).
    let mut pos = seg_body_start;
    let mut has_tracks = false;
    let mut has_cluster = false;
    let mut width = None;
    let mut height = None;
    let mut truncated_cluster_vint: Option<(usize, usize)> = None;

    while pos + 4 <= data.len() {
        let Some((id, id_len)) = read_ebml_id(data, pos) else {
            break;
        };
        let Some((sz_opt, sz_w)) = read_ebml_size(data, pos + id_len) else {
            break;
        };
        let body_start = pos + id_len + sz_w;
        if body_start > data.len() {
            break;
        }

        match id {
            0x1654_AE6B => {
                // Tracks
                let body_end = sz_opt
                    .map(|s| (body_start as u64).saturating_add(s).min(data.len() as u64) as usize)
                    .unwrap_or(data.len());
                if body_end > body_start + 4 {
                    has_tracks = true;
                    let (w, h) = parse_mkv_video_dims(&data[body_start..body_end]);
                    if width.is_none() {
                        width = w;
                        height = h;
                    }
                }
            }
            0x1F43_B675 => {
                // Cluster
                let avail = data.len() - body_start;
                if avail >= 4 && has_nontrivial_bytes(&data[body_start..], 2) {
                    has_cluster = true;
                    if sz_opt
                        .is_some_and(|s| (body_start as u64).saturating_add(s) > data.len() as u64)
                    {
                        truncated_cluster_vint = Some((pos + id_len, sz_w));
                    }
                }
            }
            _ => {}
        }

        match sz_opt {
            Some(sz) => {
                let next = (body_start as u64).saturating_add(sz);
                if next > data.len() as u64 {
                    break;
                }
                pos = next as usize;
            }
            None => {
                // Unknown-sized element: scan forward for Tracks/Cluster signatures if still missing.
                if !has_tracks {
                    if let Some(rel) = data[body_start..]
                        .windows(4)
                        .position(|w| w == [0x16, 0x54, 0xAE, 0x6B])
                    {
                        pos = body_start + rel;
                        continue;
                    }
                }
                if !has_cluster {
                    if let Some(rel) = data[body_start..]
                        .windows(4)
                        .position(|w| w == [0x1F, 0x43, 0xB6, 0x75])
                    {
                        pos = body_start + rel;
                        continue;
                    }
                }
                break;
            }
        }
    }

    if !has_tracks || !has_cluster {
        return (
            None,
            MediaRepairReport::unrecoverable(
                width,
                height,
                "MKV/WebM iz bilgisi (Tracks) veya medya kümesi (Cluster) eksik; oynatılamaz.",
            ),
        );
    }

    if seg_truncated || truncated_cluster_vint.is_some() {
        let out = if build_output {
            let mut repaired = data.to_vec();
            if seg_truncated {
                write_ebml_unknown_vint(&mut repaired, seg_pos + 4, seg_vint_w);
            }
            if let Some((vpos, vw)) = truncated_cluster_vint {
                write_ebml_unknown_vint(&mut repaired, vpos, vw);
            }
            Some(repaired)
        } else {
            None
        };
        return (
            out,
            MediaRepairReport::repaired(
                width,
                height,
                "Kesilmiş MKV/WebM segment/küme boyutu onarıldı; kurtarılan bölüm oynatılabilir.",
            ),
        );
    }

    (
        if build_output {
            Some(data.to_vec())
        } else {
            None
        },
        MediaRepairReport::intact(
            width,
            height,
            "MKV/WebM yapısı (EBML + Tracks + Cluster) sağlam; doğrudan oynatılabilir.",
        ),
    )
}

// ============================================================================
// 10. Lightweight Structural Checks for Secondary Photo/Audio/Video Formats
// ============================================================================

fn inspect_isobmff_generic(ext: &str, data: &[u8]) -> MediaRepairReport {
    let boxes = walk_mp4_top_boxes(data);
    let has_ftyp = boxes.iter().any(|b| &b.fourcc == b"ftyp" && !b.truncated);
    let has_body = boxes
        .iter()
        .any(|b| matches!(&b.fourcc, b"meta" | b"mdat" | b"moov"));
    if has_ftyp && has_body {
        MediaRepairReport::intact(
            None,
            None,
            format!(
                "{} ISO-BMFF başlığı doğrulandı; açılabilir.",
                ext.to_ascii_uppercase()
            ),
        )
    } else {
        MediaRepairReport::unrecoverable(
            None,
            None,
            format!(
                "{} başlığı eksik veya bozuk; açılamaz.",
                ext.to_ascii_uppercase()
            ),
        )
    }
}

fn inspect_tiff_like(data: &[u8]) -> MediaRepairReport {
    if data.len() < 16 {
        return MediaRepairReport::unrecoverable(None, None, "TIFF/RAW başlığı eksik; açılamaz.");
    }
    let ifd_off = if data.starts_with(b"II\x2A\x00") {
        read_u32_le(data, 4).unwrap_or(0) as usize
    } else if data.starts_with(b"MM\x00\x2A") {
        read_u32_be(data, 4).unwrap_or(0) as usize
    } else {
        return MediaRepairReport::unrecoverable(
            None,
            None,
            "TIFF/RAW başlığı geçersiz; açılamaz.",
        );
    };
    if ifd_off >= 8 && ifd_off < data.len() {
        MediaRepairReport::intact(None, None, "TIFF/RAW başlığı sağlam; açılabilir.")
    } else {
        MediaRepairReport::unrecoverable(None, None, "TIFF/RAW IFD dizini erişilemez; açılamaz.")
    }
}

fn inspect_ico(data: &[u8]) -> MediaRepairReport {
    if data.len() >= 22 && data[0..4] == [0x00, 0x00, 0x01, 0x00] {
        let count = read_u16_le(data, 4).unwrap_or(0) as usize;
        if count >= 1 && 6 + count * 16 <= data.len() {
            let w = if data[6] == 0 {
                256
            } else {
                u32::from(data[6])
            };
            let h = if data[7] == 0 {
                256
            } else {
                u32::from(data[7])
            };
            return MediaRepairReport::intact(
                Some(w),
                Some(h),
                "ICO simge başlığı sağlam; açılabilir.",
            );
        }
    }
    MediaRepairReport::unrecoverable(None, None, "ICO başlığı geçersiz; açılamaz.")
}

fn inspect_svg(data: &[u8]) -> MediaRepairReport {
    if let Ok(text) = std::str::from_utf8(&data[..data.len().min(8192)]) {
        if text.to_ascii_lowercase().contains("<svg") {
            return MediaRepairReport::intact(None, None, "SVG vektör yapısı geçerli; açılabilir.");
        }
    }
    MediaRepairReport::unrecoverable(None, None, "Geçerli SVG içeriği bulunamadı; açılamaz.")
}

fn inspect_psd(data: &[u8]) -> MediaRepairReport {
    if data.len() >= 26 && &data[0..4] == b"8BPS" {
        let ver = read_u16_be(data, 4).unwrap_or(0);
        let h = read_u32_be(data, 14).unwrap_or(0);
        let w = read_u32_be(data, 18).unwrap_or(0);
        if matches!(ver, 1 | 2) && w > 0 && h > 0 && w <= 300_000 && h <= 300_000 {
            return MediaRepairReport::intact(Some(w), Some(h), "PSD başlığı sağlam; açılabilir.");
        }
    }
    MediaRepairReport::unrecoverable(None, None, "PSD başlığı bozuk; açılamaz.")
}

fn inspect_mp3(data: &[u8]) -> MediaRepairReport {
    let has_id3 = data.starts_with(b"ID3") && data.len() >= 10;
    let has_sync = data
        .windows(2)
        .take(4096)
        .any(|w| w[0] == 0xFF && (w[1] & 0xE0) == 0xE0 && (w[1] & 0x06) != 0);
    if (has_id3 || has_sync) && has_nontrivial_bytes(data, 8) {
        MediaRepairReport::intact(None, None, "MP3 ses akışı sağlam; oynatılabilir.")
    } else {
        MediaRepairReport::unrecoverable(None, None, "MP3 ses karesi bulunamadı; oynatılamaz.")
    }
}

fn inspect_aac(data: &[u8]) -> MediaRepairReport {
    let has_adts = data.len() >= 7 && data[0] == 0xFF && (data[1] & 0xF0) == 0xF0;
    let has_ftyp = data.len() >= 16 && &data[4..8] == b"ftyp";
    if (has_adts || has_ftyp) && has_nontrivial_bytes(data, 8) {
        MediaRepairReport::intact(None, None, "AAC ses akışı sağlam; oynatılabilir.")
    } else {
        MediaRepairReport::unrecoverable(None, None, "AAC ses başlığı bulunamadı; oynatılamaz.")
    }
}

fn inspect_magic_simple(data: &[u8], magic: &[u8], label: &str) -> MediaRepairReport {
    if data.starts_with(magic) && data.len() > magic.len() + 8 && has_nontrivial_bytes(data, 8) {
        MediaRepairReport::intact(None, None, format!("{label} yapısı sağlam; oynatılabilir."))
    } else {
        MediaRepairReport::unrecoverable(
            None,
            None,
            format!("{label} başlığı eksik veya bozuk; oynatılamaz."),
        )
    }
}

// ============================================================================
// 11. Comprehensive Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn build_minimal_jpeg(
        width: u16,
        height: u16,
        app1_payload: Option<&[u8]>,
        scan: &[u8],
        with_eoi: bool,
    ) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        if let Some(app1) = app1_payload {
            out.extend_from_slice(&[0xFF, 0xE1]);
            let len = (app1.len() + 2) as u16;
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(app1);
        }
        // DQT (minimal valid 65-byte quantization table)
        out.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
        out.extend_from_slice(&[16u8; 64]);
        // SOF0 (8-bit, 1 component)
        out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
        // DHT (minimal valid Huffman table)
        out.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x14, 0x00]);
        let mut counts = [0u8; 16];
        counts[0] = 1;
        out.extend_from_slice(&counts);
        out.push(0x00);
        // SOS (1 component)
        out.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
        out.extend_from_slice(scan);
        if with_eoi {
            out.extend_from_slice(&[0xFF, 0xD9]);
        }
        out
    }

    fn build_valid_png(width: u32, height: u32, idat_payload: &[u8], with_iend: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&PNG_SIGNATURE);
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
        push_png_chunk(&mut out, b"IHDR", &ihdr);
        push_png_chunk(&mut out, b"IDAT", idat_payload);
        if with_iend {
            out.extend_from_slice(&PNG_IEND_CHUNK);
        }
        out
    }

    fn build_ftyp_box() -> Vec<u8> {
        let mut pay = Vec::new();
        pay.extend_from_slice(b"isom");
        pay.extend_from_slice(&0x0000_0200u32.to_be_bytes());
        pay.extend_from_slice(b"isomiso2avc1mp41");
        mp4_box(b"ftyp", &pay)
    }

    // Genuine 1280x720 H.264 Baseline Level 3.1 SPS NAL unit (Exp-Golomb verified).
    fn sample_h264_sps_1280x720() -> Vec<u8> {
        vec![0x67, 0x42, 0xC0, 0x1F, 0xF4, 0x02, 0x80, 0x2D, 0xC8]
    }

    fn sample_h264_pps() -> Vec<u8> {
        vec![0x68, 0xCE, 0x38, 0x80]
    }

    #[test]
    fn jpeg_find_end_ignores_embedded_exif_thumbnail_eoi() {
        let thumb = build_minimal_jpeg(160, 120, None, &[0x12, 0x34, 0x56, 0x78], true);
        let mut exif = b"Exif\0\0II*\0\x08\0\0\0".to_vec();
        exif.extend_from_slice(&thumb);
        let main_scan = &[
            0xAA, 0xFF, 0x00, 0xBB, 0xFF, 0xD0, 0xCC, 0xDD, 0xFF, 0x00, 0xEE, 0x11,
        ];
        let mut full = build_minimal_jpeg(4000, 3000, Some(&exif), main_scan, true);
        let real_len = full.len();
        full.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0xFF, 0xD9]); // trailing disk garbage

        assert_eq!(find_jpeg_end(&full), Some(real_len));
        let report = inspect_media("jpg", &full);
        assert_eq!(report.status, "intact");
        assert!(report.openable);
        assert!(!report.repaired);
        assert_eq!(report.width, Some(4000));
        assert_eq!(report.height, Some(3000));
    }

    #[test]
    fn jpeg_truncated_repair_trims_zeros_and_appends_eoi() {
        let mut broken = build_minimal_jpeg(
            1920,
            1080,
            None,
            &[0x45, 0x67, 0x89, 0xAB, 0xCD, 0xFF, 0x00, 0x00, 0x00, 0x00],
            false,
        );
        // Add a dangling 0xFF at EOF too.
        broken.push(0xFF);

        let inspected = inspect_media("jpeg", &broken);
        assert_eq!(inspected.status, "repaired");
        assert!(inspected.openable);
        assert!(inspected.repaired);

        let (fixed, report) = repair_media("jpeg", &broken);
        assert_eq!(report.status, "repaired");
        assert!(report.openable);
        assert!(report.repaired);
        assert!(fixed.ends_with(&[0xFF, 0xD9]));
        assert_eq!(fixed[fixed.len() - 3], 0xCD);

        // Round-trip: repaired output MUST inspect as completely intact!
        let recheck = inspect_media("jpg", &fixed);
        assert_eq!(recheck.status, "intact");
        assert!(recheck.openable);
        assert!(!recheck.repaired);
        assert_eq!(recheck.width, Some(1920));
        assert_eq!(recheck.height, Some(1080));
    }

    #[test]
    fn jpeg_damaged_tail_marker_repairs_cleanly() {
        // Scan data followed by an overwritten sector containing invalid unstuffed marker FF 42.
        let damaged = build_minimal_jpeg(
            800,
            600,
            None,
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0xFF, 0x42, 0x99, 0x88, 0x77],
            false,
        );
        let (fixed, report) = repair_media("jpg", &damaged);
        assert_eq!(report.status, "repaired");
        assert!(report.openable);
        assert!(fixed.ends_with(&[0x55, 0xFF, 0xD9]));
        assert_eq!(inspect_media("jpg", &fixed).status, "intact");
    }

    #[test]
    fn jpeg_exif_thumbnail_rescued_when_main_scan_corrupted() {
        let thumb = build_minimal_jpeg(320, 240, None, &[0x2A, 0x3B, 0x4C, 0x5D, 0x6E], true);
        let mut exif = b"Exif\0\0MM\0*\0\0\0\x08".to_vec();
        exif.extend_from_slice(&thumb);

        // Main photo has APP1 with intact EXIF thumbnail, then zeroed/corrupted main body (no SOS).
        let mut corrupt_main = vec![0xFF, 0xD8, 0xFF, 0xE1];
        let app1_len = (exif.len() + 2) as u16;
        corrupt_main.extend_from_slice(&app1_len.to_be_bytes());
        corrupt_main.extend_from_slice(&exif);
        corrupt_main.extend_from_slice(&[0x00; 256]);

        let (rescued, report) = repair_media("jpg", &corrupt_main);
        assert_eq!(report.status, "thumbnail_rescued");
        assert!(report.openable);
        assert!(report.repaired);
        assert_eq!(report.width, Some(320));
        assert_eq!(report.height, Some(240));
        assert_eq!(rescued, thumb);
        assert_eq!(inspect_media("jpg", &rescued).status, "intact");
    }

    #[test]
    fn jpeg_zeros_and_garbage_are_unrecoverable() {
        let zeros = vec![0u8; 4096];
        let r0 = inspect_media("jpg", &zeros);
        assert_eq!(r0.status, "unrecoverable");
        assert!(!r0.openable);

        let mut fake = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        fake.extend_from_slice(&[0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01]);
        fake.extend_from_slice(&[0x55; 256]); // random junk, no SOF/DQT/DHT/SOS
        let r1 = inspect_media("jpg", &fake);
        assert_eq!(r1.status, "unrecoverable");
        assert!(!r1.openable);
    }

    #[test]
    fn png_find_end_walks_chunks_and_repairs_truncated_idat_and_iend() {
        // Real RFC 1950 zlib header (0x78 0x9C) + payload that even contains literal "IEND\xAE\x42\x60\x82"!
        let mut idat_data = vec![0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00];
        idat_data.extend_from_slice(b"IEND\xAE\x42\x60\x82");
        idat_data.extend_from_slice(&[0x05, 0xFE, 0x02, 0xFE]);

        let intact = build_valid_png(64, 48, &idat_data, true);
        let mut with_junk = intact.clone();
        with_junk.extend_from_slice(&[0xAA; 64]);
        assert_eq!(find_png_end(&with_junk), Some(intact.len()));
        assert_eq!(inspect_media("png", &with_junk).status, "intact");

        // Truncate mid-IDAT (drop last 18 bytes covering tail of IDAT + CRC + entire IEND)
        let truncated = &intact[..intact.len() - 18];
        let (repaired, report) = repair_media("png", truncated);
        assert_eq!(report.status, "repaired");
        assert!(report.openable);
        assert!(report.repaired);
        assert_eq!(report.width, Some(64));
        assert_eq!(report.height, Some(48));
        assert!(repaired.ends_with(&PNG_IEND_CHUNK));

        // Verify round-trip CRC & chunk structure is 100% intact!
        let recheck = inspect_media("png", &repaired);
        assert_eq!(recheck.status, "intact");
        assert!(recheck.openable);
        assert!(!recheck.repaired);
    }

    #[test]
    fn png_zeroed_idat_is_unrecoverable() {
        let bad = build_valid_png(100, 100, &[0x00; 32], true);
        let report = inspect_media("png", &bad);
        assert_eq!(report.status, "unrecoverable");
        assert!(!report.openable);
    }

    #[test]
    fn mp4_find_size_supports_32bit_and_64bit_largesize_boxes() {
        let ftyp = build_ftyp_box();
        let free_box = mp4_box(b"free", &[0u8; 16]);
        // Build 64-bit largesize mdat (`size32 == 1`)
        let payload = vec![0xABu8; 64];
        let mut mdat64 = Vec::new();
        mdat64.extend_from_slice(&1u32.to_be_bytes());
        mdat64.extend_from_slice(b"mdat");
        mdat64.extend_from_slice(&((16 + payload.len()) as u64).to_be_bytes());
        mdat64.extend_from_slice(&payload);

        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&ftyp);
        mp4.extend_from_slice(&free_box);
        mp4.extend_from_slice(&mdat64);
        let exact_end = mp4.len();
        mp4.extend_from_slice(b"GARBAGE_TRAILING_SECTOR_DATA_123456");

        assert_eq!(find_mp4_size(&mp4), Some(exact_end));
    }

    #[test]
    fn mp4_missing_moov_synthesizes_valid_moov_from_avcc_h264_frames() {
        let ftyp = build_ftyp_box();
        let sps = sample_h264_sps_1280x720();
        let pps = sample_h264_pps();
        let idr1 = vec![0x65, 0x88, 0x84, 0x21, 0xA0, 0x33, 0x44, 0x55, 0x66, 0x77];
        let pframe = vec![0x41, 0x9A, 0x24, 0x6C, 0x88, 0x99, 0xAA, 0xBB];
        let idr2 = vec![0x65, 0x88, 0x84, 0x01, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22];

        let mut mdat_body = Vec::new();
        for nal in [&sps, &pps, &idr1, &pframe, &idr2] {
            mdat_body.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            mdat_body.extend_from_slice(nal);
        }
        // Simulate truncated mdat: declared size is 500 KiB larger than actual bytes, NO moov!
        let mut broken_mp4 = Vec::new();
        broken_mp4.extend_from_slice(&ftyp);
        let fake_mdat_len = (8 + mdat_body.len() + 500_000) as u32;
        broken_mp4.extend_from_slice(&fake_mdat_len.to_be_bytes());
        broken_mp4.extend_from_slice(b"mdat");
        broken_mp4.extend_from_slice(&mdat_body);

        let inspected = inspect_media("mp4", &broken_mp4);
        assert_eq!(inspected.status, "repaired");
        assert!(inspected.openable);
        assert!(inspected.repaired);
        assert_eq!(inspected.width, Some(1280));
        assert_eq!(inspected.height, Some(720));

        let (repaired, report) = repair_media("mp4", &broken_mp4);
        assert_eq!(report.status, "repaired");
        assert!(report.openable);
        assert!(report.note_tr.contains("3 kare"));

        // Verify synthesized boxes (`moov`, `avcC`, `stsz`, `stco`, `stss`) are genuinely present!
        assert!(repaired.windows(4).any(|w| w == b"moov"));
        assert!(repaired.windows(4).any(|w| w == b"avcC"));
        assert!(repaired.windows(4).any(|w| w == b"stsz"));
        assert!(repaired.windows(4).any(|w| w == b"stco"));
        assert!(repaired.windows(4).any(|w| w == b"stss"));

        // Verify round-trip inspection passes as 100% intact with exact dimensions!
        let recheck = inspect_media("mp4", &repaired);
        assert_eq!(recheck.status, "intact");
        assert!(recheck.openable);
        assert!(!recheck.repaired);
        assert_eq!(recheck.width, Some(1280));
        assert_eq!(recheck.height, Some(720));
        assert_eq!(find_mp4_size(&repaired), Some(repaired.len()));
    }

    #[test]
    fn mp4_missing_moov_annex_b_fallback_converts_and_synthesizes_moov() {
        let ftyp = build_ftyp_box();
        let sps = sample_h264_sps_1280x720();
        let pps = sample_h264_pps();
        let idr = vec![0x65, 0x88, 0x84, 0x10, 0x20, 0x30, 0x40, 0x50];

        let mut mdat_body = Vec::new();
        for nal in [&sps, &pps, &idr] {
            mdat_body.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            mdat_body.extend_from_slice(nal);
        }
        let mut broken = Vec::new();
        broken.extend_from_slice(&ftyp);
        broken.extend_from_slice(&mp4_box(b"mdat", &mdat_body));

        let (repaired, report) = repair_media("mov", &broken);
        assert_eq!(report.status, "repaired");
        assert!(report.openable);
        assert_eq!(report.width, Some(1280));
        assert_eq!(report.height, Some(720));
        assert_eq!(inspect_media("mov", &repaired).status, "intact");
    }

    #[test]
    fn mp4_faststart_truncated_mdat_patches_mdat_size() {
        let ftyp = build_ftyp_box();
        let sps = sample_h264_sps_1280x720();
        let pps = sample_h264_pps();
        let sps_info = parse_h264_sps(&sps);
        let moov = synthesize_moov(
            &sps_info,
            &sps,
            &pps,
            &[VideoSample {
                offset: 512,
                size: 16,
                is_sync: true,
            }],
        );
        let mut faststart = Vec::new();
        faststart.extend_from_slice(&ftyp);
        faststart.extend_from_slice(&moov);
        // Truncated mdat claiming 100 KiB when only 64 bytes exist.
        faststart.extend_from_slice(&100_000u32.to_be_bytes());
        faststart.extend_from_slice(b"mdat");
        faststart.extend_from_slice(&[0x5A; 64]);

        let (repaired, report) = repair_media("mp4", &faststart);
        assert_eq!(report.status, "repaired");
        assert!(report.openable);
        assert_eq!(inspect_media("mp4", &repaired).status, "intact");
    }

    #[test]
    fn mp4_garbage_or_empty_mdat_without_moov_is_unrecoverable() {
        let ftyp = build_ftyp_box();
        let mut garbage_mp4 = ftyp.clone();
        garbage_mp4.extend_from_slice(&mp4_box(
            b"mdat",
            &[
                0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
                0x99, 0xAA,
            ],
        ));
        let r1 = inspect_media("mp4", &garbage_mp4);
        assert_eq!(r1.status, "unrecoverable");
        assert!(!r1.openable);

        let mut zero_mp4 = ftyp;
        zero_mp4.extend_from_slice(&mp4_box(b"mdat", &[0u8; 256]));
        let r2 = inspect_media("mp4", &zero_mp4);
        assert_eq!(r2.status, "unrecoverable");
        assert!(!r2.openable);
    }

    #[test]
    fn gif_truncated_subblock_repairs_length_and_appends_trailer() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&16u16.to_le_bytes()); // width
        gif.extend_from_slice(&16u16.to_le_bytes()); // height
        gif.extend_from_slice(&[0x00, 0x00, 0x00]); // no GCT
                                                    // Image descriptor
        gif.push(0x2C);
        gif.extend_from_slice(&[0, 0, 0, 0]); // left, top
        gif.extend_from_slice(&16u16.to_le_bytes());
        gif.extend_from_slice(&16u16.to_le_bytes());
        gif.push(0x00); // packed
        gif.push(0x02); // LZW min code size
        gif.push(0x20); // sub-block declares 32 bytes, but only 5 survive!
        gif.extend_from_slice(&[0x4C, 0x01, 0x02, 0x03, 0x04]);

        let (repaired, report) = repair_media("gif", &gif);
        assert_eq!(report.status, "repaired");
        assert!(report.openable);
        assert!(repaired.ends_with(&[0x00, 0x3B]));
        assert_eq!(inspect_media("gif", &repaired).status, "intact");
    }

    #[test]
    fn bmp_truncated_clamps_height_preserving_sign() {
        for height_i32 in [10i32, -10i32] {
            let width = 4u32; // 4 * 3 = 12 bytes per row (already 4-byte aligned)
            let row_stride = 12usize;
            let full_img_sz = row_stride * 10;
            let total_sz = 54 + full_img_sz;

            let mut bmp = Vec::with_capacity(54 + row_stride * 4 + 5);
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&(total_sz as u32).to_le_bytes());
            bmp.extend_from_slice(&0u32.to_le_bytes());
            bmp.extend_from_slice(&54u32.to_le_bytes()); // bfOffBits
            bmp.extend_from_slice(&40u32.to_le_bytes()); // biSize
            bmp.extend_from_slice(&(width as i32).to_le_bytes());
            bmp.extend_from_slice(&height_i32.to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes()); // planes
            bmp.extend_from_slice(&24u16.to_le_bytes()); // 24bpp
            bmp.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
            bmp.extend_from_slice(&(full_img_sz as u32).to_le_bytes());
            bmp.extend_from_slice(&[0u8; 16]); // pels/colors
                                               // Only 4 complete scanlines + 5 partial bytes survived!
            bmp.extend_from_slice(&vec![0x7F; row_stride * 4 + 5]);

            let (repaired, report) = repair_media("bmp", &bmp);
            assert_eq!(report.status, "repaired");
            assert!(report.openable);
            assert_eq!(report.width, Some(4));
            assert_eq!(report.height, Some(4));
            let patched_h = read_i32_le(&repaired, 22).unwrap();
            assert_eq!(patched_h, if height_i32 < 0 { -4 } else { 4 });
            assert_eq!(repaired.len(), 54 + row_stride * 4);
            assert_eq!(inspect_media("bmp", &repaired).status, "intact");
        }
    }

    #[test]
    fn webp_and_avi_truncated_headers_repair_cleanly() {
        // Truncated WebP VP8L (lossless 320x240)
        let bits: u32 = (320 - 1) | ((240 - 1) << 14);
        let mut vp8l = vec![0x2F];
        vp8l.extend_from_slice(&bits.to_le_bytes());
        vp8l.extend_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);

        let mut webp = Vec::new();
        webp.extend_from_slice(b"RIFF");
        webp.extend_from_slice(&50_000u32.to_le_bytes()); // claims 50 KiB
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(b"VP8L");
        webp.extend_from_slice(&49_988u32.to_le_bytes());
        webp.extend_from_slice(&vp8l);

        assert_eq!(find_riff_size(&webp), Some(webp.len()));
        let (repaired_webp, webp_rep) = repair_media("webp", &webp);
        assert_eq!(webp_rep.status, "repaired");
        assert!(webp_rep.openable);
        assert_eq!(webp_rep.width, Some(320));
        assert_eq!(webp_rep.height, Some(240));
        assert_eq!(inspect_media("webp", &repaired_webp).status, "intact");

        // Truncated AVI (`RIFF....AVI ` + `LIST hdrl` + truncated `LIST movi`)
        let mut avih = vec![0u8; 56];
        avih[32..36].copy_from_slice(&640u32.to_le_bytes());
        avih[36..40].copy_from_slice(&360u32.to_le_bytes());
        let mut hdrl = b"hdrl".to_vec();
        hdrl.extend_from_slice(b"avih");
        hdrl.extend_from_slice(&(avih.len() as u32).to_le_bytes());
        hdrl.extend_from_slice(&avih);

        let mut avi = Vec::new();
        avi.extend_from_slice(b"RIFF");
        avi.extend_from_slice(&999_999u32.to_le_bytes());
        avi.extend_from_slice(b"AVI ");
        avi.extend_from_slice(b"LIST");
        avi.extend_from_slice(&(hdrl.len() as u32).to_le_bytes());
        avi.extend_from_slice(&hdrl);
        avi.extend_from_slice(b"LIST");
        avi.extend_from_slice(&500_000u32.to_le_bytes()); // truncated movi!
        avi.extend_from_slice(b"movi");
        // One complete frame + one partial frame
        avi.extend_from_slice(b"00dc");
        avi.extend_from_slice(&12u32.to_le_bytes());
        avi.extend_from_slice(&[0x33; 12]);
        avi.extend_from_slice(b"00dc");
        avi.extend_from_slice(&400u32.to_le_bytes());
        avi.extend_from_slice(&[0x44; 10]);

        let (repaired_avi, avi_rep) = repair_media("avi", &avi);
        assert_eq!(avi_rep.status, "repaired");
        assert!(avi_rep.openable);
        assert_eq!(avi_rep.width, Some(640));
        assert_eq!(avi_rep.height, Some(360));
        assert_eq!(inspect_media("avi", &repaired_avi).status, "intact");
    }

    #[test]
    fn mkv_intact_repaired_and_unrecoverable() {
        let mut mkv = vec![0x1A, 0x45, 0xDF, 0xA3, 0x84, 0x42, 0x86, 0x81, 0x01]; // EBML header (4 bytes body)
                                                                                  // Segment with 2-byte finite VINT declaring 2000 bytes (truncated!)
        mkv.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0x47, 0xD0]);
        // Tracks (16 54 AE 6B) with 12-byte body containing TrackEntry -> Video -> 640x480
        let mut video = vec![0xB0, 0x82, 0x02, 0x80, 0xBA, 0x82, 0x01, 0xE0]; // 640 x 480
        let mut v_el = vec![0xE0, (0x80 | video.len() as u8)];
        v_el.append(&mut video);
        let mut te_el = vec![0xAE, (0x80 | v_el.len() as u8)];
        te_el.append(&mut v_el);
        mkv.extend_from_slice(&[0x16, 0x54, 0xAE, 0x6B, (0x80 | te_el.len() as u8)]);
        mkv.extend_from_slice(&te_el);
        // Cluster (1F 43 B6 75) declaring 500 bytes (truncated to 10 bytes!)
        mkv.extend_from_slice(&[0x1F, 0x43, 0xB6, 0x75, 0x41, 0xF4]);
        mkv.extend_from_slice(&[0xE7, 0x81, 0x00, 0xA3, 0x85, 0x81, 0x00, 0x00, 0x80, 0x55]);

        let (repaired, rep) = repair_media("mkv", &mkv);
        assert_eq!(rep.status, "repaired");
        assert!(rep.openable);
        assert_eq!(rep.width, Some(640));
        assert_eq!(rep.height, Some(480));
        assert_eq!(inspect_media("mkv", &repaired).status, "intact");

        // Missing Cluster -> unrecoverable
        let no_cluster = &mkv[..mkv.len() - 16];
        let bad = inspect_media("webm", no_cluster);
        assert_eq!(bad.status, "unrecoverable");
        assert!(!bad.openable);
    }

    #[test]
    fn mp4_interleaved_audio_packets_still_recover_h264_frames() {
        let ftyp = build_ftyp_box();
        let sps = sample_h264_sps_1280x720();
        let pps = sample_h264_pps();
        let idr = vec![0x65, 0x88, 0x84, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let p1 = vec![0x41, 0x9A, 0x11, 0x22, 0x33, 0x44, 0x55];
        let fake_aac = vec![
            0x21, 0x10, 0x05, 0x20, 0xA4, 0x1B, 0x99, 0x7E, 0x44, 0x33, 0x22,
        ];

        let mut mdat_body = Vec::new();
        mdat_body.extend_from_slice(&fake_aac); // leading AAC audio chunk
        for nal in [&sps, &pps, &idr] {
            mdat_body.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            mdat_body.extend_from_slice(nal);
        }
        mdat_body.extend_from_slice(&fake_aac); // interleaved AAC audio chunk
        mdat_body.extend_from_slice(&(p1.len() as u32).to_be_bytes());
        mdat_body.extend_from_slice(&p1);

        let mut broken = Vec::new();
        broken.extend_from_slice(&ftyp);
        broken.extend_from_slice(&mp4_box(b"mdat", &mdat_body));

        let (repaired, rep) = repair_media("mp4", &broken);
        assert_eq!(rep.status, "repaired");
        assert!(rep.openable);
        assert!(rep.note_tr.contains("2 kare"));
        assert_eq!(inspect_media("mp4", &repaired).status, "intact");
    }

    #[test]
    fn progressive_jpeg_and_wav_repair_work() {
        // Progressive JPEG (SOF2 + 2 SOS scans)
        let mut prog = vec![0xFF, 0xD8];
        prog.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
        prog.extend_from_slice(&[16u8; 64]);
        prog.extend_from_slice(&[
            0xFF, 0xC2, 0x00, 0x0B, 0x08, 0x01, 0x00, 0x01, 0x40, 0x01, 0x01, 0x11, 0x00,
        ]);
        prog.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x14, 0x00]);
        let mut counts = [0u8; 16];
        counts[0] = 1;
        prog.extend_from_slice(&counts);
        prog.push(0x00);
        // Scan 1
        prog.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00]);
        prog.extend_from_slice(&[0x12, 0x34, 0x56]);
        // Scan 2
        prog.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x01, 0x3F, 0x00]);
        prog.extend_from_slice(&[0x78, 0x9A, 0xBC, 0xFF, 0xD9]);

        let p_rep = inspect_media("jpg", &prog);
        assert_eq!(p_rep.status, "intact");
        assert!(p_rep.openable);
        assert_eq!(p_rep.width, Some(320));
        assert_eq!(p_rep.height, Some(256));

        // Truncated WAV
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&50_000u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&[1, 0, 2, 0, 0x44, 0xAC, 0, 0, 0x10, 0xB1, 2, 0, 4, 0, 16, 0]);
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&49_000u32.to_le_bytes());
        wav.extend_from_slice(&[0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80]);

        let (repaired_wav, w_rep) = repair_media("wav", &wav);
        assert_eq!(w_rep.status, "repaired");
        assert!(w_rep.openable);
        assert_eq!(inspect_media("wav", &repaired_wav).status, "intact");
    }

    #[test]
    fn png_multi_idat_truncated_in_second_chunk_repairs_cleanly() {
        let mut png = Vec::new();
        png.extend_from_slice(&PNG_SIGNATURE);
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&256u32.to_be_bytes());
        ihdr.extend_from_slice(&256u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
        push_png_chunk(&mut png, b"IHDR", &ihdr);
        push_png_chunk(
            &mut png,
            b"IDAT",
            &[0x78, 0x9C, 0x62, 0x60, 0x01, 0x02, 0x03, 0x04],
        );
        // Second IDAT chunk truncated mid-stream (declares 100 bytes, only 10 survive, no IEND)
        png.extend_from_slice(&100u32.to_be_bytes());
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x11, 0x22, 0x33, 0x44, 0x55]);

        let (repaired, rep) = repair_media("png", &png);
        assert_eq!(rep.status, "repaired");
        assert!(rep.openable);
        assert_eq!(rep.width, Some(256));
        assert_eq!(rep.height, Some(256));
        assert!(repaired.ends_with(&PNG_IEND_CHUNK));
        assert_eq!(inspect_media("png", &repaired).status, "intact");
    }

    #[test]
    fn mp4_high_profile_1080p_cropped_sps_and_midframe_truncation() {
        let ftyp = build_ftyp_box();
        // Genuine High Profile (100) Level 4.0 1920x1080 SPS (68*16=1088 minus crop_bottom=4*2 -> 1080)
        let sps_1080p = vec![
            0x67, 0x64, 0x00, 0x28, 0xAC, 0xEC, 0x07, 0x80, 0x22, 0x7E, 0x54,
        ];
        let pps = sample_h264_pps();
        let idr = vec![0x65, 0x88, 0x84, 0x21, 0xA0, 0x11, 0x22, 0x33, 0x44];
        let p1 = vec![0x41, 0x9A, 0x24, 0x6C, 0x55, 0x66, 0x77, 0x88];

        let mut mdat_body = Vec::new();
        for nal in [&sps_1080p, &pps, &idr, &p1] {
            mdat_body.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            mdat_body.extend_from_slice(nal);
        }
        // Append a mid-frame truncated P-slice claiming 8000 bytes when only 12 bytes survived
        mdat_body.extend_from_slice(&8000u32.to_be_bytes());
        mdat_body.extend_from_slice(&[
            0x41, 0x9A, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A,
        ]);

        let mut broken = Vec::new();
        broken.extend_from_slice(&ftyp);
        broken.extend_from_slice(&0u32.to_be_bytes()); // size == 0 ("extends to EOF")
        broken.extend_from_slice(b"mdat");
        broken.extend_from_slice(&mdat_body);

        let (repaired, rep) = repair_media("mp4", &broken);
        assert_eq!(rep.status, "repaired");
        assert!(rep.openable);
        assert_eq!(rep.width, Some(1920));
        assert_eq!(rep.height, Some(1080));
        assert!(rep.note_tr.contains("2 kare"));
        assert_eq!(inspect_media("mp4", &repaired).status, "intact");
    }
}
