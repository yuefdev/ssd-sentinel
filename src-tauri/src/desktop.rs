use crate::{
    core::{Config, Core, JobRequest},
    optimize::OptimizeRequest,
    report,
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State,
};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;

#[tauri::command]
fn get_state(core: State<'_, Arc<Core>>) -> Value {
    core.state()
}
#[tauri::command]
fn refresh(core: State<'_, Arc<Core>>) {
    core.refresh();
}
#[tauri::command]
fn get_history(core: State<'_, Arc<Core>>, disk_id: String) -> Result<Value, String> {
    core.history(&disk_id)
}
#[tauri::command]
fn get_events(core: State<'_, Arc<Core>>, disk_id: Option<String>) -> Result<Value, String> {
    core.events(disk_id.as_deref())
}
#[tauri::command]
fn get_jobs(
    core: State<'_, Arc<Core>>,
    kind: String,
    disk_id: Option<String>,
) -> Result<Value, String> {
    core.saved_jobs(&kind, disk_id.as_deref())
}
#[tauri::command]
fn get_health_forecast(core: State<'_, Arc<Core>>, disk_id: String) -> Result<Value, String> {
    core.health_forecast(&disk_id)
}
#[tauri::command]
fn get_surface_checkpoint(core: State<'_, Arc<Core>>, disk_id: String) -> Result<Value, String> {
    Ok(core.surface_checkpoint(&disk_id)?.unwrap_or(Value::Null))
}
#[tauri::command]
fn clear_surface_checkpoint(core: State<'_, Arc<Core>>, disk_id: String) -> Result<(), String> {
    core.clear_surface_checkpoint(&disk_id)
}
#[tauri::command]
async fn start_job(core: State<'_, Arc<Core>>, request: JobRequest) -> Result<Value, String> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || core.start_job(request))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn cancel_job(core: State<'_, Arc<Core>>, id: String) -> Result<Value, String> {
    core.cancel_job(&id)
}
#[tauri::command]
async fn disk_action(
    core: State<'_, Arc<Core>>,
    disk_id: String,
    action: String,
    confirmation: Option<String>,
    letter: Option<String>,
) -> Result<Value, String> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        core.disk_action(
            &disk_id,
            &action,
            confirmation.as_deref(),
            letter.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn optimize_action(
    core: State<'_, Arc<Core>>,
    request: OptimizeRequest,
) -> Result<Value, String> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || core.optimize_action(request))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn get_optimizations(core: State<'_, Arc<Core>>) -> Result<Value, String> {
    core.optimizations()
}
#[tauri::command]
async fn analyze_storage_tiering(
    core: State<'_, Arc<Core>>,
    disk_id: String,
    letter: String,
) -> Result<Value, String> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || core.analyze_storage_tiering(&disk_id, &letter))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn relocate_tier_item(
    core: State<'_, Arc<Core>>,
    disk_id: String,
    source_path: String,
    target_letter: String,
    direction: String,
    confirmation: String,
) -> Result<Value, String> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        core.relocate_tier_item(
            &disk_id,
            &source_path,
            &target_letter,
            &direction,
            &confirmation,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn rollback_tier_move(
    core: State<'_, Arc<Core>>,
    move_id: String,
    confirmation: String,
) -> Result<Value, String> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || core.rollback_tier_move(&move_id, &confirmation))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn list_tier_moves(core: State<'_, Arc<Core>>) -> Result<Value, String> {
    core.list_tier_moves()
}
#[tauri::command]
async fn analyze_dev_workspace(
    core: State<'_, Arc<Core>>,
    disk_id: String,
    letter: String,
    custom_root: Option<String>,
) -> Result<Value, String> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        core.analyze_dev_workspace(&disk_id, &letter, custom_root.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn clean_dev_artifact(
    core: State<'_, Arc<Core>>,
    disk_id: String,
    artifact_path: String,
    confirmation: String,
) -> Result<Value, String> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        core.clean_dev_artifact(&disk_id, &artifact_path, &confirmation)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn pick_dev_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(selected) = app.dialog().file().blocking_pick_folder() else {
            return Ok(None);
        };
        let path = selected.into_path().map_err(|e| e.to_string())?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
fn restart_as_admin(app: tauri::AppHandle) -> Result<Value, String> {
    #[cfg(windows)]
    {
        use std::ptr::{null, null_mut};
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        if crate::platform::is_admin() {
            return Ok(json!({"relaunching":false,"already_admin":true}));
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut params = vec!["--elevated-relaunch".to_owned()];
        for arg in std::env::args().skip(1) {
            if arg == "--elevated-relaunch" {
                continue;
            }
            if arg.contains([' ', '\t', '"']) {
                params.push(format!("\"{}\"", arg.replace('"', "\\\"")));
            } else {
                params.push(arg);
            }
        }
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
        let op_w = wide("runas");
        let exe_w = wide(&exe.to_string_lossy());
        let params_w = wide(&params.join(" "));
        let dir_w = exe
            .parent()
            .map(|p| wide(&p.to_string_lossy()))
            .unwrap_or_else(|| vec![0]);
        let res = unsafe {
            ShellExecuteW(
                null_mut(),
                op_w.as_ptr(),
                exe_w.as_ptr(),
                params_w.as_ptr(),
                if dir_w.len() > 1 {
                    dir_w.as_ptr()
                } else {
                    null()
                },
                SW_SHOWNORMAL,
            )
        };
        if (res as isize) > 32 {
            let handle = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(80));
                if let Some(core) = handle.try_state::<Arc<Core>>() {
                    core.shutdown();
                }
                handle.exit(0);
            });
            Ok(json!({"relaunching":true}))
        } else {
            Ok(json!({"relaunching":false,"cancelled":true}))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Err("Yönetici yükseltmesi yalnız Windows'ta desteklenir.".into())
    }
}
struct TrayMenus {
    show: MenuItem<tauri::Wry>,
    refresh: MenuItem<tauri::Wry>,
    minimise: CheckMenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

fn apply_tray_language(menus: &TrayMenus, lang: &str) {
    let en = lang == "en";
    let _ = menus.show.set_text(if en {
        "Show window"
    } else {
        "Pencereyi göster"
    });
    let _ = menus
        .refresh
        .set_text(if en { "Refresh now" } else { "Şimdi yenile" });
    let _ = menus.minimise.set_text(if en {
        "Minimize to tray on close"
    } else {
        "Kapatınca tepsiye küçült"
    });
    let _ = menus.quit.set_text(if en { "Quit" } else { "Çıkış" });
}

pub fn format_tray_tooltip(disks: &[Value], replay: bool, lang: &str) -> String {
    let header = if replay {
        if lang == "en" {
            "SSD Sentinel · Replay mode"
        } else {
            "SSD Sentinel · Kayıt modu"
        }
    } else {
        "SSD Sentinel"
    };
    if disks.is_empty() {
        return header.to_owned();
    }
    let lines = disks
        .iter()
        .map(|d| {
            let model = d["model"].as_str().unwrap_or("?");
            let health = d["health"]["percent"]
                .as_u64()
                .map(|p| format!("{p}%"))
                .unwrap_or_else(|| "—".into());
            let temp = d["metrics"]["temperature_c"]
                .as_f64()
                .filter(|t| t.is_finite())
                .map(|t| format!("{}°C", t.round() as i64))
                .unwrap_or_else(|| "—".into());
            format!("{model} · {health} · {temp}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{header}\n{lines}")
}

pub const DEFAULT_UPDATE_REPO: &str = "yuefdev/ssd-sentinel";

pub fn resolve_update_repo(compile_time: Option<&str>, runtime: Option<&str>) -> Option<String> {
    let explicit = runtime
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| compile_time.map(str::trim).filter(|s| !s.is_empty()));
    let raw = match explicit {
        Some(s) if matches!(s.to_ascii_lowercase().as_str(), "none" | "off" | "disabled") => {
            return None;
        }
        Some(s) => s,
        None => DEFAULT_UPDATE_REPO,
    };
    if raw.contains("..") {
        return None;
    }
    let (owner, repo) = raw.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    let valid_part = |p: &str| {
        p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    if !valid_part(owner) || !valid_part(repo) {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

pub fn validate_github_release_url(url: &str, repo: &str) -> Result<(), String> {
    if url.is_empty()
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
        || url.contains("..")
        || url.contains('\\')
    {
        return Err("Geçersiz sürüm bağlantısı.".into());
    }
    let base = format!("https://github.com/{repo}");
    let releases = format!("{base}/releases");
    if url == base
        || url == releases
        || url.starts_with(&format!("{releases}/"))
        || url.starts_with(&format!("{releases}?"))
    {
        Ok(())
    } else {
        Err("Sürüm bağlantısı yalnızca resmi GitHub deposu altında açılabilir.".into())
    }
}

pub fn is_allowed_download_host(host: &str) -> bool {
    matches!(
        host.to_ascii_lowercase().as_str(),
        "github.com"
            | "api.github.com"
            | "objects.githubusercontent.com"
            | "release-assets.githubusercontent.com"
            | "github-releases.githubusercontent.com"
    )
}

pub fn parse_https_url(url: &str) -> Result<(String, String), String> {
    if url.is_empty() || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("Geçersiz HTTPS bağlantısı.".into());
    }
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "Yalnız HTTPS bağlantıları desteklenir.".to_owned())?;
    let (host_raw, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if host_raw.is_empty() || host_raw.contains(['@', ':']) {
        return Err("Geçersiz uzak sunucu adresi.".into());
    }
    Ok((host_raw.to_ascii_lowercase(), path.to_owned()))
}

pub fn parse_sha256_for_asset(manifest: &str, asset_name: &str) -> Option<String> {
    let normalize_sep = |s: &str| -> String {
        s.chars()
            .map(|c| match c {
                ' ' | '.' | '-' => '-',
                _ => c.to_ascii_lowercase(),
            })
            .collect()
    };
    let target = asset_name.trim().trim_start_matches("./");
    let target_norm = normalize_sep(target);
    for line in manifest.lines() {
        let trimmed = line.trim().trim_start_matches('\u{feff}');
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let hash = parts.next()?.trim();
        let raw_name = parts
            .next()?
            .trim()
            .trim_start_matches('*')
            .trim_start_matches("./");
        let base = raw_name.rsplit(['/', '\\']).next().unwrap_or(raw_name);
        if hash.len() == 64
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
            && (raw_name.eq_ignore_ascii_case(target)
                || base.eq_ignore_ascii_case(target)
                || normalize_sep(base) == target_norm)
        {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

pub fn select_release_assets(release: &Value) -> (Option<(String, String, u64)>, Option<String>) {
    let Some(assets) = release["assets"].as_array() else {
        return (None, None);
    };
    let mut setup_asset: Option<(String, String, u64)> = None;
    let mut sha_url: Option<String> = None;
    for asset in assets {
        let Some(name) = asset["name"].as_str() else {
            continue;
        };
        let Some(url) = asset["browser_download_url"].as_str() else {
            continue;
        };
        if !url.starts_with("https://") {
            continue;
        }
        if name.eq_ignore_ascii_case("SHA256SUMS.txt") {
            sha_url = Some(url.to_owned());
        } else if name.to_ascii_lowercase().ends_with("-setup.exe") {
            let size = asset["size"].as_u64().unwrap_or(0);
            let is_x64 = name.to_ascii_lowercase().contains("x64");
            if setup_asset.is_none() || is_x64 {
                setup_asset = Some((name.to_owned(), url.to_owned(), size));
            }
        }
    }
    (setup_asset, sha_url)
}

#[cfg(windows)]
fn winhttp_fetch(
    start_url: &str,
    accept: Option<&str>,
    user_agent: &str,
    timeout_ms: i32,
    max_bytes: usize,
    follow_redirects: usize,
) -> Result<(u32, Vec<u8>), String> {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Networking::WinHttp::{
        WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
        WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
        WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_FLAG_SECURE,
        WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
        WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LOCATION, WINHTTP_QUERY_STATUS_CODE,
    };

    struct HInternet(*mut c_void);
    impl Drop for HInternet {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    WinHttpCloseHandle(self.0);
                }
            }
        }
    }
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let agent = wide(user_agent);
    let session = HInternet(unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
            null(),
            null(),
            0,
        )
    });
    if session.0.is_null() {
        return Err("Ağ oturumu açılamadı; sürüm kontrolü çevrimdışı.".into());
    }
    unsafe {
        WinHttpSetTimeouts(session.0, timeout_ms, timeout_ms, timeout_ms, timeout_ms);
    }

    let mut current_url = start_url.to_owned();
    for hop in 0..=follow_redirects {
        let (host, path) = parse_https_url(&current_url)?;
        if !is_allowed_download_host(&host) {
            return Err(format!(
                "İzin verilmeyen indirme sunucusu reddedildi: {host}"
            ));
        }
        let host_w = wide(&host);
        let connect = HInternet(unsafe { WinHttpConnect(session.0, host_w.as_ptr(), 443, 0) });
        if connect.0.is_null() {
            return Err("Güncelleme sunucusuna bağlanılamadı.".into());
        }
        let verb = wide("GET");
        let path_w = wide(&path);
        let req = HInternet(unsafe {
            WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                path_w.as_ptr(),
                null(),
                null(),
                null(),
                WINHTTP_FLAG_SECURE,
            )
        });
        if req.0.is_null() {
            return Err("Güncelleme isteği oluşturulamadı.".into());
        }
        let mut policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        unsafe {
            WinHttpSetOption(
                req.0,
                WINHTTP_OPTION_REDIRECT_POLICY,
                (&mut policy as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
        if let Some(hdr) = accept {
            let hdr_w = wide(hdr);
            unsafe {
                WinHttpAddRequestHeaders(req.0, hdr_w.as_ptr(), u32::MAX, WINHTTP_ADDREQ_FLAG_ADD);
            }
        }
        let sent = unsafe { WinHttpSendRequest(req.0, null(), 0, null(), 0, 0, 0) };
        if sent == 0 || unsafe { WinHttpReceiveResponse(req.0, null_mut()) } == 0 {
            return Err("İnternet bağlantısı yok veya zaman aşımına uğradı.".into());
        }
        let mut status_code: u32 = 0;
        let mut status_size: u32 = std::mem::size_of::<u32>() as u32;
        unsafe {
            WinHttpQueryHeaders(
                req.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut status_code as *mut u32).cast(),
                &mut status_size,
                null_mut(),
            );
        }
        if matches!(status_code, 301 | 302 | 303 | 307 | 308) {
            if hop >= follow_redirects {
                return Err("Yönlendirme sınırı aşıldı.".into());
            }
            let mut loc_buf = [0u16; 2048];
            let mut loc_bytes: u32 = (loc_buf.len() * std::mem::size_of::<u16>()) as u32;
            let ok = unsafe {
                WinHttpQueryHeaders(
                    req.0,
                    WINHTTP_QUERY_LOCATION,
                    null(),
                    loc_buf.as_mut_ptr().cast(),
                    &mut loc_bytes,
                    null_mut(),
                )
            };
            if ok == 0 {
                return Err("Sunucu yönlendirme adresi bildirmedi.".into());
            }
            let units = (loc_bytes as usize) / std::mem::size_of::<u16>();
            let loc = String::from_utf16_lossy(&loc_buf[..units.min(loc_buf.len())]);
            current_url = loc.trim_end_matches('\0').to_owned();
            continue;
        }
        let mut body = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let mut read = 0u32;
            if unsafe {
                WinHttpReadData(req.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut read)
            } == 0
                || read == 0
            {
                break;
            }
            if body.len().saturating_add(read as usize) > max_bytes {
                return Err("İndirilen güncelleme paketi boyut sınırını aştı.".into());
            }
            body.extend_from_slice(&buf[..read as usize]);
        }
        return Ok((status_code, body));
    }
    Err("İndirme yönlendirmesi tamamlanamadı.".into())
}

fn active_update_repo() -> Option<String> {
    let runtime_repo = std::env::var("SSD_SENTINEL_UPDATE_REPO").ok();
    resolve_update_repo(
        option_env!("SSD_SENTINEL_UPDATE_REPO"),
        runtime_repo.as_deref(),
    )
}

fn parse_semver(tag: &str) -> (u64, u64, u64) {
    let clean = tag.trim().trim_start_matches(['v', 'V']);
    let mut parts = clean
        .split(['.', '-'])
        .filter_map(|p| p.parse::<u64>().ok());
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}
#[tauri::command]
async fn check_updates() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let current = env!("CARGO_PKG_VERSION");
        let Some(repo) = active_update_repo() else {
            return Ok(json!({
                "repo": Value::Null,
                "current": current,
                "status": "unconfigured",
                "latest": null,
                "message": "Sürüm deposu henüz yapılandırılmadı; yeni sürümleri proje sayfasından kontrol edebilirsiniz.",
                "message_en": "Release repository is not configured yet; check the project page for updates."
            }));
        };
        #[cfg(windows)]
        {
            let api_url = format!("https://api.github.com/repos/{repo}/releases/latest");
            let releases_fallback = format!("https://github.com/{repo}/releases");
            let ua = format!("SSD-Sentinel/{current}");
            let (status_code, body) = match winhttp_fetch(
                &api_url,
                Some("Accept: application/vnd.github+json\r\n"),
                &ua,
                5000,
                512 * 1024,
                0,
            ) {
                Ok(v) => v,
                Err(msg) => {
                    return Ok(json!({
                        "repo": repo,
                        "current": current,
                        "status": "unreachable",
                        "latest": null,
                        "url": releases_fallback,
                        "message": msg,
                        "message_en": "Could not reach GitHub Releases API (offline or timed out)."
                    }));
                }
            };
            if status_code == 404 {
                return Ok(json!({
                    "repo": repo,
                    "current": current,
                    "status": "no_release",
                    "latest": null,
                    "url": releases_fallback,
                    "message": "Henüz yayımlanmış bir sürüm kaydı bulunamadı.",
                    "message_en": "No published release was found yet."
                }));
            }
            if status_code != 200 {
                return Ok(json!({
                    "repo": repo,
                    "current": current,
                    "status": "unreachable",
                    "latest": null,
                    "url": releases_fallback,
                    "message": format!("Sürüm servisi HTTP {status_code} döndürdü."),
                    "message_en": format!("Release service returned HTTP {status_code}.")
                }));
            }
            let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let Some(tag) = parsed["tag_name"].as_str() else {
                return Ok(json!({
                    "repo": repo,
                    "current": current,
                    "status": "no_release",
                    "latest": null,
                    "url": releases_fallback,
                    "message": "Yeni sürüm kaydı bulunamadı.",
                    "message_en": "No release tag was found."
                }));
            };
            let url = parsed["html_url"].as_str().unwrap_or(&releases_fallback);
            let (setup_asset, sha256_url) = select_release_assets(&parsed);
            let notes: Option<String> = parsed["body"].as_str().and_then(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.chars().take(360).collect())
                }
            });
            let (asset_name, download_url, asset_size) = match setup_asset {
                Some((n, u, sz)) => (Some(n), Some(u), Some(sz)),
                None => (None, None, None),
            };
            if parse_semver(tag) > parse_semver(current) {
                Ok(json!({
                    "repo": repo,
                    "current": current,
                    "status": "update_available",
                    "latest": tag,
                    "url": url,
                    "download_url": download_url,
                    "asset_name": asset_name,
                    "asset_size": asset_size,
                    "sha256_url": sha256_url,
                    "notes": notes,
                    "message": format!("Yeni sürüm mevcut: {tag}"),
                    "message_en": format!("A new version is available: {tag}")
                }))
            } else {
                Ok(json!({
                    "repo": repo,
                    "current": current,
                    "status": "up_to_date",
                    "latest": tag,
                    "url": url,
                    "download_url": Value::Null,
                    "asset_name": Value::Null,
                    "asset_size": Value::Null,
                    "sha256_url": Value::Null,
                    "notes": Value::Null,
                    "message": "Güncel sürümü kullanıyorsunuz.",
                    "message_en": "You are using the latest version."
                }))
            }
        }
        #[cfg(not(windows))]
        {
            Ok(json!({
                "repo": repo,
                "current": current,
                "status": "unreachable",
                "latest": null,
                "message": "Yalnız Windows'ta desteklenir.",
                "message_en": "Supported on Windows only."
            }))
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn open_release_url(url: String) -> Result<(), String> {
    let repo = active_update_repo().ok_or("Güncelleme deposu yapılandırılmamış.")?;
    validate_github_release_url(&url, &repo)?;
    #[cfg(windows)]
    {
        use std::ptr::{null, null_mut};
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
        let op = wide("open");
        let target = wide(&url);
        let res = unsafe {
            ShellExecuteW(
                null_mut(),
                op.as_ptr(),
                target.as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            )
        };
        if (res as isize) <= 32 {
            return Err("Tarayıcı açılamadı.".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err("Yalnız Windows'ta desteklenir.".into())
    }
}

#[tauri::command]
async fn download_and_launch_update(
    download_url: String,
    asset_name: String,
    sha256_url: Option<String>,
) -> Result<Value, String> {
    let repo = active_update_repo().ok_or("Güncelleme deposu yapılandırılmamış.")?;
    let expected_prefix = format!("https://github.com/{repo}/releases/download/");
    if !download_url.starts_with(&expected_prefix)
        || download_url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        || download_url.contains("..")
        || download_url.contains('\\')
    {
        return Err("İndirme bağlantısı doğrulanamadı.".into());
    }
    if asset_name.is_empty()
        || !asset_name.to_ascii_lowercase().ends_with("-setup.exe")
        || asset_name.contains(['/', '\\', ':', '\0'])
        || asset_name.contains("..")
    {
        return Err("Geçersiz kurulum dosyası adı.".into());
    }
    if let Some(ref su) = sha256_url {
        if !su.starts_with(&expected_prefix)
            || su.chars().any(|c| c.is_control() || c.is_whitespace())
            || su.contains("..")
            || su.contains('\\')
        {
            return Err("SHA-256 doğrulama bağlantısı geçersiz.".into());
        }
    }
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || {
            use sha2::{Digest, Sha256};
            use std::io::Write;
            use std::ptr::{null, null_mut};
            use windows_sys::Win32::UI::Shell::ShellExecuteW;
            use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

            let ua = format!("SSD-Sentinel/{}", env!("CARGO_PKG_VERSION"));
            let (status, bytes) = winhttp_fetch(
                &download_url,
                None,
                &ua,
                15000,
                128 * 1024 * 1024,
                5,
            )?;
            if status != 200 || bytes.len() < 64 * 1024 {
                return Err(format!("Güncelleme paketi indirilemedi (HTTP {status})."));
            }

            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let actual_sha: String = hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();

            let update_dir = std::env::temp_dir().join("ssd-sentinel-update");
            std::fs::create_dir_all(&update_dir).map_err(|e| e.to_string())?;
            let dest = update_dir.join(&asset_name);
            {
                let mut file = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
                file.write_all(&bytes).map_err(|e| e.to_string())?;
                file.sync_all().map_err(|e| e.to_string())?;
            }

            let mut verified = false;
            if let Some(surl) = sha256_url {
                let (s_status, s_bytes) = winhttp_fetch(&surl, None, &ua, 10000, 256 * 1024, 5)?;
                if s_status != 200 {
                    let _ = std::fs::remove_file(&dest);
                    return Err(format!("SHA256SUMS.txt indirilemedi (HTTP {s_status})."));
                }
                let manifest = String::from_utf8_lossy(&s_bytes);
                let Some(expected) = parse_sha256_for_asset(&manifest, &asset_name) else {
                    let _ = std::fs::remove_file(&dest);
                    return Err("SHA256SUMS.txt içinde kurulum dosyası özeti bulunamadı.".into());
                };
                if expected != actual_sha {
                    let _ = std::fs::remove_file(&dest);
                    return Err("İndirilen kurulum dosyasının SHA-256 doğrulaması başarısız; güvenlik için silindi.".into());
                }
                verified = true;
            }

            let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
            let op_w = wide("open");
            let dest_w = wide(&dest.to_string_lossy());
            let dir_w = wide(&update_dir.to_string_lossy());
            let res = unsafe {
                ShellExecuteW(
                    null_mut(),
                    op_w.as_ptr(),
                    dest_w.as_ptr(),
                    null(),
                    dir_w.as_ptr(),
                    SW_SHOWNORMAL,
                )
            };
            if (res as isize) <= 32 {
                return Err("İndirilen kurulum dosyası başlatılamadı.".into());
            }
            Ok(json!({
                "launched": true,
                "path": dest.to_string_lossy(),
                "sha256": actual_sha,
                "verified": verified
            }))
        })
        .await
        .map_err(|e| e.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = (download_url, asset_name, sha256_url);
        Err("Yalnız Windows'ta desteklenir.".into())
    }
}
#[tauri::command]
fn set_preferences(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
    minimise_to_tray: bool,
    theme: Option<String>,
    health_mode: Option<String>,
    language: Option<String>,
    read_only_guard: Option<bool>,
) -> Result<(), String> {
    core.set_preferences(
        minimise_to_tray,
        health_mode.as_deref(),
        language.as_deref(),
        read_only_guard,
    )?;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_theme(theme.map(|t| {
            if t == "light" {
                tauri::Theme::Light
            } else {
                tauri::Theme::Dark
            }
        }));
    }
    if let Some(menus) = app.try_state::<TrayMenus>() {
        let _ = menus.minimise.set_checked(minimise_to_tray);
        if let Some(lang) = language.as_deref() {
            apply_tray_language(&menus, lang);
        }
    }
    Ok(())
}
#[tauri::command]
async fn restore_recovered_item(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
    job_id: String,
    item_id: String,
) -> Result<Value, String> {
    if core.config.snapshot_dir.is_some() || core.state()["source"] == "replay" {
        return Err("Kayıt modunda dosya kurtarma kapalıdır.".into());
    }
    let (source_letter, item) = core.find_recovery_item(&job_id, &item_id)?;
    if item["trim_zeroed"] == true {
        return Err(
            "Bu dosyanın sektörleri SSD TRIM tarafından sıfırlandığı için kurtarılamaz.".into(),
        );
    }
    let default_name = item["name"].as_str().unwrap_or("recovered.bin").to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let selected = app
            .dialog()
            .file()
            .set_file_name(&default_name)
            .blocking_save_file();
        let Some(selected) = selected else {
            return Ok(json!({"saved":false}));
        };
        let dest = selected.into_path().map_err(|e| e.to_string())?;
        crate::recovery::restore_item(&source_letter, &item, &dest)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn preview_recovered_item(
    core: State<'_, Arc<Core>>,
    job_id: String,
    item_id: String,
) -> Result<Value, String> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || core.preview_recovered_item(&job_id, &item_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn restore_recovered_batch(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
    job_id: String,
    item_ids: Vec<String>,
) -> Result<Value, String> {
    if core.config.snapshot_dir.is_some() || core.state()["source"] == "replay" {
        return Err("Kayıt modunda toplu dosya kurtarma kapalıdır.".into());
    }
    if item_ids.is_empty() {
        return Err("Toplu kurtarma için en az bir dosya seçin.".into());
    }
    let job = core.find_recovery_job(&job_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let selected = app.dialog().file().blocking_pick_folder();
        let Some(selected) = selected else {
            return Ok(json!({
                "saved": false,
                "cancelled": true,
                "restored": 0,
                "failed": 0,
                "skipped": 0,
                "dest_dir": "",
                "paths": [],
                "errors": []
            }));
        };
        let dest = selected.into_path().map_err(|e| e.to_string())?;
        crate::recovery::restore_batch(&job, &item_ids, &dest)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn save_report(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
    kind: String,
    disk_id: Option<String>,
    format: Option<String>,
    redact: Option<bool>,
) -> Result<Value, String> {
    let do_redact = redact.unwrap_or(true);
    let fmt = format.unwrap_or_else(|| "json".into());
    let state_val = core.state();
    let jobs_val = if kind == "snapshot" {
        None
    } else if matches!(
        kind.as_str(),
        "benchmark" | "surface" | "defrag" | "autopilot" | "recovery" | "smart_tier"
    ) {
        Some(json!({
            "source": state_val["source"],
            "kind": kind,
            "jobs": core.saved_jobs(&kind, disk_id.as_deref())?,
            "current": state_val["job"]
        }))
    } else {
        return Err("Geçersiz rapor türü.".into());
    };
    let prefer_html = fmt.eq_ignore_ascii_case("html");
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app.dialog().file().set_file_name(format!(
            "SSD-Sentinel-{kind}.{}",
            if prefer_html { "html" } else { "json" }
        ));
        dialog = if prefer_html {
            dialog
                .add_filter("HTML Raporu", &["html"])
                .add_filter("JSON", &["json"])
        } else {
            dialog
                .add_filter("JSON", &["json"])
                .add_filter("HTML Raporu", &["html"])
        };
        let selected = dialog.blocking_save_file();
        let Some(selected) = selected else {
            return Ok(json!({"saved":false}));
        };
        let path = selected.into_path().map_err(|e| e.to_string())?;
        let is_html = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
            || (prefer_html
                && !path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("json")));
        let bytes = if is_html {
            report::render_html_report(&kind, &state_val, jobs_val.as_ref(), do_redact).into_bytes()
        } else {
            let mut val = jobs_val.unwrap_or(state_val);
            if do_redact {
                report::redact_value(&mut val);
            }
            serde_json::to_vec_pretty(&val).map_err(|e| e.to_string())?
        };
        let parent = path.parent().ok_or("Geçersiz hedef klasör.")?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        use std::io::Write;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(&path).map_err(|e| e.to_string())?;
        Ok(json!({"saved":true,"path":path.to_string_lossy()}))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn show(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn tray_image(temp: Option<u32>, critical: bool, warning: bool) -> Image<'static> {
    let mut rgba = vec![0; 32 * 32 * 4];
    let colour: [u8; 4] = if critical {
        [248, 113, 113, 255]
    } else if warning {
        [250, 196, 90, 255]
    } else {
        [95, 221, 173, 255]
    };
    for y in 3..29 {
        for x in 2..30 {
            let i = (y * 32 + x) * 4;
            rgba[i..i + 4].copy_from_slice(&colour);
        }
    }
    let digits = [
        0b111101101101111u16,
        0b010110010010111,
        0b111001111100111,
        0b111001111001111,
        0b101101111001001,
        0b111100111001111,
        0b111100111101111,
        0b111001001001001,
        0b111101111101111,
        0b111101111001111,
    ];
    let glyphs = temp
        .map(|temp| {
            let number = temp.min(99);
            [
                digits[(number / 10) as usize],
                digits[(number % 10) as usize],
            ]
        })
        .unwrap_or([0b000000111000000; 2]);
    for (pos, glyph) in glyphs.into_iter().enumerate() {
        for y in 0..5 {
            for x in 0..3 {
                if (glyph >> (14 - (y * 3 + x))) & 1 == 1 {
                    for dy in 0..3 {
                        for dx in 0..3 {
                            let i = ((8 + y * 3 + dy) * 32 + 5 + pos * 12 + x * 3 + dx) * 4;
                            rgba[i..i + 4].copy_from_slice(&[16, 18, 20, 255]);
                        }
                    }
                }
            }
        }
    }
    Image::new_owned(rgba, 32, 32)
}

pub fn run() {
    let elevated_relaunch = std::env::args().any(|a| a == "--elevated-relaunch");
    let start_minimized = std::env::args().any(|a| a == "--minimized");
    let is_replay_cli = std::env::args().any(|a| a == "--snapshot-dir");
    if elevated_relaunch {
        std::thread::sleep(Duration::from_millis(450));
    }
    let mut builder = tauri::Builder::default();
    if !is_replay_cli {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _| {
            if argv.iter().any(|a| a == "--elevated-relaunch") {
                if let Some(core) = app.try_state::<Arc<Core>>() {
                    core.shutdown();
                }
                app.exit(0);
            } else {
                show(app);
            }
        }));
    }
    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_state,
            refresh,
            get_history,
            get_events,
            get_jobs,
            get_health_forecast,
            get_surface_checkpoint,
            clear_surface_checkpoint,
            start_job,
            cancel_job,
            disk_action,
            optimize_action,
            get_optimizations,
            analyze_storage_tiering,
            relocate_tier_item,
            rollback_tier_move,
            list_tier_moves,
            analyze_dev_workspace,
            clean_dev_artifact,
            pick_dev_folder,
            restart_as_admin,
            check_updates,
            open_release_url,
            download_and_launch_update,
            save_report,
            set_preferences,
            restore_recovered_item,
            preview_recovered_item,
            restore_recovered_batch
        ])
        .setup(move |app| {
            let mut data_dir = app.path().app_local_data_dir()?;
            let resource = app
                .path()
                .resource_dir()?
                .join("smartmontools/bin/smartctl.exe");
            let mut smartctl = if resource.is_file() {
                resource
            } else {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../tools/smartmontools/bin/smartctl.exe")
            };
            let mut snapshot_dir = None;
            let mut args = std::env::args().skip(1);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--snapshot-dir" => {
                        snapshot_dir = Some(PathBuf::from(
                            args.next().ok_or("--snapshot-dir requires a path")?,
                        ))
                    }
                    "--data-dir" => {
                        data_dir = PathBuf::from(args.next().ok_or("--data-dir requires a path")?)
                    }
                    "--smartctl" => {
                        smartctl = PathBuf::from(args.next().ok_or("--smartctl requires a path")?)
                    }
                    "--minimized" | "--elevated-relaunch" => {}
                    _ => return Err(format!("Bilinmeyen seçenek: {arg}").into()),
                }
            }
            let core = Core::new(Config {
                smartctl,
                data_dir,
                snapshot_dir,
            })?;
            app.manage(core.clone());
            let show_item = MenuItem::with_id(app, "show", "Pencereyi göster", true, None::<&str>)?;
            let refresh_item =
                MenuItem::with_id(app, "refresh", "Şimdi yenile", true, None::<&str>)?;
            let minimise = CheckMenuItem::with_id(
                app,
                "minimise",
                "Kapatınca tepsiye küçült",
                true,
                core.minimise_to_tray.load(Ordering::SeqCst),
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, "quit", "Çıkış", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &refresh_item, &minimise, &quit])?;
            let tray_menus = TrayMenus {
                show: show_item,
                refresh: refresh_item,
                minimise,
                quit,
            };
            {
                let lang = crate::core::lock(&core.language).clone();
                apply_tray_language(&tray_menus, &lang);
            }
            app.manage(tray_menus);
            TrayIconBuilder::with_id("sentinel")
                .icon(tray_image(None, false, false))
                .tooltip("SSD Sentinel · Başlatılıyor")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    let core = app.state::<Arc<Core>>();
                    match event.id.as_ref() {
                        "show" => show(app),
                        "refresh" => core.refresh(),
                        "minimise" => {
                            let _ =
                                core.set_minimise(!core.minimise_to_tray.load(Ordering::SeqCst));
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        show(tray.app_handle());
                    }
                })
                .build(app)?;
            if let Some(window) = app.get_webview_window("main") {
                if start_minimized {
                    let _ = window.hide();
                }
                let core = core.clone();
                let w = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        if core.minimise_to_tray.load(Ordering::SeqCst) {
                            api.prevent_close();
                            let _ = w.hide();
                        }
                    }
                });
            }
            core.start_background();
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut seen = HashSet::new();
                let mut hot = HashSet::new();
                let mut initial = true;
                while !core.stop.load(Ordering::SeqCst) {
                    let state = core.state();
                    let disks = state["disks"].as_array().cloned().unwrap_or_default();
                    let temp = disks
                        .iter()
                        .filter_map(|d| d["metrics"]["temperature_c"].as_f64())
                        .reduce(f64::max);
                    let critical = disks.iter().any(|d| {
                        d["health"]["status"] == "critical"
                            || d["health"]["percent"].as_u64().is_some_and(|p| p <= 25)
                    }) || temp.is_some_and(|t| t >= 55.0);
                    let warning = state["stale"] == true
                        || disks.iter().any(|d| {
                            d["health"]["status"] == "warning" || d["health"]["percent"].is_null()
                        });
                    let lang = state["language"].as_str().unwrap_or("tr");
                    let tooltip = format_tray_tooltip(&disks, state["source"] == "replay", lang);
                    if let Some(tray) = handle.tray_by_id("sentinel") {
                        let _ = tray.set_icon(Some(tray_image(
                            temp.map(|t| t.round() as u32),
                            critical,
                            warning,
                        )));
                        let _ = tray.set_tooltip(Some(tooltip));
                    }
                    if state["source"] == "live" && state["stale"] == false {
                        if let Ok(events) = core.events(None) {
                            for event in events.as_array().into_iter().flatten() {
                                let id = event["id"].to_string();
                                if seen.insert(id)
                                    && !initial
                                    && matches!(
                                        event["level"].as_str(),
                                        Some("warning" | "critical")
                                    )
                                {
                                    let title = if lang == "en" {
                                        "SSD Sentinel · New alert"
                                    } else {
                                        "SSD Sentinel · Yeni uyarı"
                                    };
                                    let _ = handle
                                        .notification()
                                        .builder()
                                        .title(title)
                                        .body(event["message"].as_str().unwrap_or(""))
                                        .show();
                                }
                            }
                            initial = false;
                        }
                        for disk in &disks {
                            let id = disk["id"].as_str().unwrap_or("").to_owned();
                            let t = disk["metrics"]["temperature_c"].as_f64();
                            if t.is_some_and(|t| t >= 55.0) && hot.insert(id.clone()) {
                                let title = if lang == "en" {
                                    "High disk temperature"
                                } else {
                                    "Disk sıcaklığı yüksek"
                                };
                                let _ = handle
                                    .notification()
                                    .builder()
                                    .title(title)
                                    .body(format!(
                                        "{} · {} °C",
                                        disk["model"].as_str().unwrap_or(""),
                                        t.unwrap()
                                    ))
                                    .show();
                            }
                            if t.is_some_and(|t| t <= 50.0) {
                                hot.remove(&id);
                            }
                        }
                    }
                    let _ = handle.emit("sentinel-state", &state);
                    for _ in 0..20 {
                        if core.stop.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("SSD Sentinel masaüstü başlatılamadı");
    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            handle.state::<Arc<Core>>().shutdown();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_tray_temperature_is_distinct_from_a_measured_zero() {
        assert!(
            tray_image(None, false, false).rgba() != tray_image(Some(0), false, false).rgba(),
            "Missing temperature must display dashes rather than a measured zero"
        );
    }

    #[test]
    fn tray_tooltip_formats_missing_health_and_temperature_without_null() {
        let disks = vec![
            json!({
                "model": "WD Blue",
                "health": { "percent": null },
                "metrics": { "temperature_c": null }
            }),
            json!({
                "model": "NVMe Pro",
                "health": { "percent": 98 },
                "metrics": { "temperature_c": 42.4 }
            }),
        ];
        let tr = format_tray_tooltip(&disks, false, "tr");
        assert!(!tr.contains("null"), "Tooltip must never show 'null': {tr}");
        assert!(tr.contains("WD Blue · — · —"));
        assert!(tr.contains("NVMe Pro · 98% · 42°C"));

        let en = format_tray_tooltip(&disks, true, "en");
        assert!(en.starts_with("SSD Sentinel · Replay mode\n"));
        assert!(!en.contains("null"));
    }

    #[test]
    fn resolve_update_repo_validates_slugs_and_defaults_to_official_repo() {
        assert_eq!(
            resolve_update_repo(None, None).as_deref(),
            Some(DEFAULT_UPDATE_REPO)
        );
        assert_eq!(
            resolve_update_repo(Some(""), Some("   ")).as_deref(),
            Some(DEFAULT_UPDATE_REPO)
        );
        assert_eq!(resolve_update_repo(None, Some("none")), None);
        assert_eq!(resolve_update_repo(Some("OFF"), None), None);
        assert_eq!(resolve_update_repo(None, Some("disabled")), None);
        assert_eq!(resolve_update_repo(Some("invalid"), None), None);
        assert_eq!(resolve_update_repo(Some("a/b/c"), None), None);
        assert_eq!(resolve_update_repo(Some("../evil"), None), None);
        assert_eq!(
            resolve_update_repo(Some("owner/ssd-sentinel"), None).as_deref(),
            Some("owner/ssd-sentinel")
        );
        assert_eq!(
            resolve_update_repo(Some("build/repo"), Some("runtime/repo")).as_deref(),
            Some("runtime/repo")
        );
    }

    #[test]
    fn validate_github_release_url_blocks_foreign_origins() {
        assert!(validate_github_release_url(
            "https://github.com/yuefdev/ssd-sentinel/releases/tag/v1.9.0",
            "yuefdev/ssd-sentinel"
        )
        .is_ok());
        assert!(validate_github_release_url(
            "https://github.com/yuefdev/ssd-sentinel/releases",
            "yuefdev/ssd-sentinel"
        )
        .is_ok());
        assert!(validate_github_release_url(
            "https://evil.example/yuefdev/ssd-sentinel/releases",
            "yuefdev/ssd-sentinel"
        )
        .is_err());
        assert!(validate_github_release_url(
            "https://github.com/yuefdev/ssd-sentinel/releases/../other",
            "yuefdev/ssd-sentinel"
        )
        .is_err());
        assert!(validate_github_release_url(
            "https://github.com/yuefdev/ssd-sentinel/releasesEvil",
            "yuefdev/ssd-sentinel"
        )
        .is_err());
    }

    #[test]
    fn parse_sha256_and_select_release_assets_handle_bom_and_setup_exe() {
        let manifest = "\u{feff}4a37f4206e21c6f6ba794119173611f9f3193760a252a0e5636050374b119b92  SSD Sentinel_1.9.0_x64-setup.exe\r\n365fa525a40f32576123595900cba8847fbaa42c892d1bbb6bd6c582af337b1b *./ssd-sentinel.exe\r\n";
        assert_eq!(
            parse_sha256_for_asset(manifest, "SSD Sentinel_1.9.0_x64-setup.exe").as_deref(),
            Some("4a37f4206e21c6f6ba794119173611f9f3193760a252a0e5636050374b119b92")
        );
        assert_eq!(
            parse_sha256_for_asset(manifest, "SSD.Sentinel_1.9.0_x64-setup.exe").as_deref(),
            Some("4a37f4206e21c6f6ba794119173611f9f3193760a252a0e5636050374b119b92")
        );
        assert_eq!(
            parse_sha256_for_asset(manifest, "SSD-Sentinel_1.9.0_x64-setup.exe").as_deref(),
            Some("4a37f4206e21c6f6ba794119173611f9f3193760a252a0e5636050374b119b92")
        );
        assert_eq!(
            parse_sha256_for_asset(manifest, "ssd-sentinel.exe").as_deref(),
            Some("365fa525a40f32576123595900cba8847fbaa42c892d1bbb6bd6c582af337b1b")
        );
        assert_eq!(parse_sha256_for_asset(manifest, "missing.exe"), None);

        let release = json!({
            "assets": [
                {
                    "name": "smartmontools-7.5-RELEASE_7_5-source.tar.gz",
                    "size": 12345,
                    "browser_download_url": "https://github.com/yuefdev/ssd-sentinel/releases/download/v2.0.0/smartmontools.tar.gz"
                },
                {
                    "name": "SSD Sentinel_2.0.0_x64-setup.exe",
                    "size": 3700000,
                    "browser_download_url": "https://github.com/yuefdev/ssd-sentinel/releases/download/v2.0.0/SSD%20Sentinel_2.0.0_x64-setup.exe"
                },
                {
                    "name": "SHA256SUMS.txt",
                    "size": 256,
                    "browser_download_url": "https://github.com/yuefdev/ssd-sentinel/releases/download/v2.0.0/SHA256SUMS.txt"
                }
            ]
        });
        let (setup, sha) = select_release_assets(&release);
        assert_eq!(
            setup,
            Some((
                "SSD Sentinel_2.0.0_x64-setup.exe".into(),
                "https://github.com/yuefdev/ssd-sentinel/releases/download/v2.0.0/SSD%20Sentinel_2.0.0_x64-setup.exe".into(),
                3700000
            ))
        );
        assert_eq!(
            sha.as_deref(),
            Some("https://github.com/yuefdev/ssd-sentinel/releases/download/v2.0.0/SHA256SUMS.txt")
        );
    }
}
