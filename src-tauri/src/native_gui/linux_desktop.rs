//! Linux desktop integrations without C UI/D-Bus/tray library links:
//! - D-Bus StatusNotifierWatcher probe (Vanilla GNOME 50 Tray Guard)
//! - Lightweight pure-Rust `zbus` StatusNotifierItem + Desktop Notifications
//! - XDG Desktop Portal dark/light appearance query (`org.freedesktop.portal.Settings`)
//! - Safe `xdg-open` URL/path launcher & `ureq` GitHub release checker with SHA-256

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

const REPO_API_LATEST: &str = "https://api.github.com/repos/yuefdev/ssd-sentinel/releases/latest";
const ALLOWED_UPDATE_HOSTS: &[&str] = &[
    "github.com",
    "api.github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "github-releases.githubusercontent.com",
];

#[cfg(target_os = "linux")]
pub fn has_status_notifier_watcher() -> bool {
    let Ok(conn) = zbus::blocking::Connection::session() else {
        return false;
    };
    let Ok(reply) = conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &("org.kde.StatusNotifierWatcher",),
    ) else {
        return false;
    };
    reply.body().deserialize::<bool>().unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub fn has_status_notifier_watcher() -> bool {
    true
}

/// Spawns a pure-Rust D-Bus StatusNotifierItem background service when a watcher is present.
/// Clicking Activate sets `restore_requested` to true so the main window un-minimizes/focuses.
#[cfg(target_os = "linux")]
pub fn start_status_notifier_item(
    restore_requested: Arc<AtomicBool>,
    quit_requested: Arc<AtomicBool>,
) -> bool {
    if !has_status_notifier_watcher() {
        return false;
    }

    struct SentinelSni {
        restore: Arc<AtomicBool>,
        quit: Arc<AtomicBool>,
    }

    #[zbus::interface(name = "org.kde.StatusNotifierItem")]
    impl SentinelSni {
        #[zbus(property)]
        fn category(&self) -> &str {
            "SystemServices"
        }
        #[zbus(property)]
        fn id(&self) -> &str {
            "ssd-sentinel"
        }
        #[zbus(property)]
        fn title(&self) -> &str {
            "SSD Sentinel"
        }
        #[zbus(property)]
        fn status(&self) -> &str {
            "Active"
        }
        #[zbus(property)]
        fn icon_name(&self) -> &str {
            "com.ssdsentinel.desktop"
        }
        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            false
        }
        fn activate(&self, _x: i32, _y: i32) {
            self.restore.store(true, Ordering::SeqCst);
        }
        fn secondary_activate(&self, _x: i32, _y: i32) {
            self.restore.store(true, Ordering::SeqCst);
        }
        fn context_menu(&self, _x: i32, _y: i32) {
            self.restore.store(true, Ordering::SeqCst);
        }
        fn quit(&self) {
            self.quit.store(true, Ordering::SeqCst);
        }
    }

    std::thread::Builder::new()
        .name("sni-tray".into())
        .spawn(move || {
            let pid = std::process::id();
            let bus_name = format!("org.kde.StatusNotifierItem-{pid}-1");
            let sni = SentinelSni {
                restore: restore_requested,
                quit: quit_requested,
            };
            let Ok(builder) = zbus::blocking::connection::Builder::session() else {
                return;
            };
            let Ok(builder) = builder.name(bus_name.as_str()) else {
                return;
            };
            let Ok(builder) = builder.serve_at("/StatusNotifierItem", sni) else {
                return;
            };
            let Ok(conn) = builder.build() else {
                return;
            };
            let _ = conn.call_method(
                Some("org.kde.StatusNotifierWatcher"),
                "/StatusNotifierWatcher",
                Some("org.kde.StatusNotifierWatcher"),
                "RegisterStatusNotifierItem",
                &(bus_name.as_str(),),
            );
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        })
        .is_ok()
}

#[cfg(not(target_os = "linux"))]
pub fn start_status_notifier_item(
    _restore_requested: Arc<AtomicBool>,
    _quit_requested: Arc<AtomicBool>,
) -> bool {
    false
}

/// Queries `org.freedesktop.portal.Settings` for `org.freedesktop.appearance.color-scheme`:
/// `Some(true)` = dark, `Some(false)` = light, `None` = no preference / portal unavailable.
#[cfg(target_os = "linux")]
pub fn detect_portal_dark_mode() -> Option<bool> {
    let conn = zbus::blocking::Connection::session().ok()?;
    let reply = conn
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Settings"),
            "ReadOne",
            &("org.freedesktop.appearance", "color-scheme"),
        )
        .ok()?;
    let val: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
    let code = u32::try_from(&val).ok()?;
    match code {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

#[cfg(not(target_os = "linux"))]
pub fn detect_portal_dark_mode() -> Option<bool> {
    None
}

#[cfg(target_os = "linux")]
pub fn send_desktop_notification(summary: &str, body: &str) {
    let summary = summary.to_owned();
    let body = body.to_owned();
    std::thread::spawn(move || {
        let Ok(conn) = zbus::blocking::Connection::session() else {
            return;
        };
        let hints: std::collections::HashMap<&str, zbus::zvariant::Value<'_>> =
            std::collections::HashMap::new();
        let actions: Vec<&str> = Vec::new();
        let _ = conn.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "SSD Sentinel",
                0u32,
                "com.ssdsentinel.desktop",
                summary.as_str(),
                body.as_str(),
                actions,
                hints,
                5000i32,
            ),
        );
    });
}

#[cfg(not(target_os = "linux"))]
pub fn send_desktop_notification(_summary: &str, _body: &str) {}

pub fn open_url_or_path(target: &str) -> Result<(), String> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err("Empty target".into());
    }
    #[cfg(target_os = "linux")]
    {
        for bin in ["/usr/bin/xdg-open", "/bin/xdg-open", "xdg-open"] {
            if let Ok(mut child) = Command::new(bin)
                .arg(trimmed)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
        }
        Err("xdg-open not available on PATH".into())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(format!("Unsupported open helper for {trimmed}"))
    }
}

fn is_allowed_update_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    ALLOWED_UPDATE_HOSTS.contains(&host.as_str())
}

fn version_gt(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .trim_start_matches(['v', 'V'])
            .split(['-', '+'])
            .next()
            .unwrap_or("0")
            .split('.')
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    let max_len = va.len().max(vb.len());
    for i in 0..max_len {
        let x = *va.get(i).unwrap_or(&0);
        let y = *vb.get(i).unwrap_or(&0);
        if x != y {
            return x > y;
        }
    }
    false
}

pub fn check_github_updates(current_version: &str, en: bool) -> Result<Value, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(12)))
        .build()
        .new_agent();
    let body: Value = agent
        .get(REPO_API_LATEST)
        .header("User-Agent", &format!("SSD-Sentinel/{current_version}"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| {
            crate::i18n::pick(
                en,
                &format!("Güncelleme denetimi başarısız: {e}"),
                &format!("Update check failed: {e}"),
            )
            .to_owned()
        })?
        .into_body()
        .read_json()
        .map_err(|e| format!("GitHub JSON parse error: {e}"))?;

    let tag = body["tag_name"].as_str().unwrap_or("").trim();
    let latest = tag.trim_start_matches(['v', 'V']);
    let has_update = !latest.is_empty() && version_gt(latest, current_version);
    let html_url = body["html_url"]
        .as_str()
        .filter(|u| is_allowed_update_url(u))
        .unwrap_or("https://github.com/yuefdev/ssd-sentinel/releases/latest");
    let asset = body["assets"].as_array().and_then(|arr| {
        arr.iter().find(|a| {
            let n = a["name"].as_str().unwrap_or("").to_ascii_lowercase();
            n.ends_with(".appimage") || n.ends_with(".deb") || n.ends_with(".tar.gz")
        })
    });
    Ok(json!({
        "current": current_version,
        "latest": latest,
        "has_update": has_update,
        "url": html_url,
        "notes": body["body"].as_str().unwrap_or(""),
        "asset_name": asset.and_then(|a| a["name"].as_str()),
        "asset_url": asset.and_then(|a| a["browser_download_url"].as_str()).filter(|u| is_allowed_update_url(u)),
    }))
}

#[allow(dead_code)]
pub fn verify_downloaded_sha256(path: &Path, expected_hex: &str) -> Result<(), String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let got: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if !got.eq_ignore_ascii_case(expected_hex.trim()) {
        return Err(format!(
            "SHA-256 mismatch: expected {expected_hex}, got {got}"
        ));
    }
    Ok(())
}

#[allow(dead_code)]
pub fn download_verified_asset(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
) -> Result<(), String> {
    if !is_allowed_update_url(url) {
        return Err("Update URL host rejected by allowlist.".into());
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .new_agent();
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut reader = resp.into_body().into_reader();
    let tmp = dest.with_extension("part");
    let mut out = File::create(&tmp).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0usize;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += n;
        if total > 256 * 1024 * 1024 {
            let _ = std::fs::remove_file(&tmp);
            return Err("Update package exceeded 256 MiB limit.".into());
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    out.sync_all().map_err(|e| e.to_string())?;
    if let Some(hex) = expected_sha256 {
        if let Err(e) = verify_downloaded_sha256(&tmp, hex) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}
