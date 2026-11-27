use chrono::Utc;
use serde_json::{json, Value};
use std::fmt::Write as _;

pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn mask_serial(serial: &str) -> String {
    let trimmed = serial.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 4 {
        return "•".repeat(chars.len());
    }
    let head: String = chars[..2].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    let dots = "•".repeat((chars.len() - 4).clamp(4, 8));
    format!("{head}{dots}{tail}")
}

pub fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if matches!(
                    k.as_str(),
                    "serial"
                        | "os_serial"
                        | "nvme_serial"
                        | "serial_number"
                        | "api_key"
                        | "ai_api_key"
                ) {
                    if let Some(s) = v.as_str() {
                        if !s.is_empty() {
                            *v = json!(mask_serial(s));
                        }
                    }
                } else if k == "data_dir" {
                    if v.as_str().is_some_and(|s| !s.is_empty()) {
                        *v = json!("[redacted]");
                    }
                } else {
                    redact_value(v);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                redact_value(item);
            }
        }
        _ => {}
    }
}

fn fmt_bytes(val: &Value) -> String {
    let n = val
        .as_u64()
        .map(|u| u as f64)
        .or_else(|| val.as_f64())
        .or_else(|| val.as_str().and_then(|s| s.parse::<f64>().ok()));
    let Some(v) = n.filter(|n| n.is_finite() && *n >= 0.0) else {
        return "—".into();
    };
    let units = ["B", "KB", "MB", "GB", "TB"];
    let idx = ((v.max(1.0).log10() / 3.0).floor() as usize).min(4);
    format!("{:.1} {}", v / 1000f64.powi(idx as i32), units[idx])
}

pub fn render_html_report(kind: &str, state: &Value, jobs: Option<&Value>, redact: bool) -> String {
    let mut state = state.clone();
    let mut jobs_val = jobs.cloned();
    if redact {
        redact_value(&mut state);
        if let Some(ref mut j) = jobs_val {
            redact_value(j);
        }
    }
    let en = state["language"].as_str() == Some("en");
    let t = |tr: &'static str, en_s: &'static str| crate::i18n::pick(en, tr, en_s);
    let version = state["version"].as_str().unwrap_or("1.0.0");
    let source = if state["source"] == "replay" {
        t("Kaydedilmiş ölçüm", "Recorded snapshot")
    } else {
        t("Canlı sistem izlemesi", "Live system monitoring")
    };
    let health_mode = if state["health_mode"] == "strict" {
        t("Sıkı Mod", "Strict mode")
    } else {
        t("Varsayılan", "Default")
    };
    let generated = Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let mut body = String::new();
    let _ = writeln!(
        body,
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>SSD Sentinel v{} {}</title><style>
body{{font-family:'Segoe UI',system-ui,sans-serif;background:#101214;color:#edf1f2;margin:0;padding:32px;line-height:1.5}}
.wrap{{max-width:1080px;margin:0 auto}}
header{{border-bottom:1px solid #2b3035;padding-bottom:16px;margin-bottom:24px;display:flex;justify-content:space-between;flex-wrap:wrap;gap:12px}}
h1{{font-size:22px;margin:0 0 4px}}h2{{font-size:17px;margin:0 0 12px}}h3{{font-size:14px;margin:16px 0 8px;color:#b4bdc4}}
.meta{{font-size:12px;color:#929da7}}
.pill{{display:inline-block;padding:2px 8px;border-radius:999px;font-size:11px;font-weight:600;background:#1a332b;color:#71dfb0;border:1px solid #2b3035}}
.pill.warning{{background:#342d20;color:#f1c572}}.pill.critical{{background:#382528;color:#f68d8d}}
.card{{background:#191d21;border:1px solid #2b3035;border-radius:12px;padding:20px;margin-bottom:20px}}
.grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:12px;margin:12px 0}}
.kv{{background:#13171b;border:1px solid #252a2f;border-radius:8px;padding:10px 12px}}
.kv span{{display:block;font-size:11px;color:#929da7}}.kv strong{{font-size:14px}}
table{{width:100%;border-collapse:collapse;font-size:12px;margin-top:8px}}
th,td{{text-align:left;padding:7px 8px;border-bottom:1px solid #252a2f}}
th{{color:#929da7;font-weight:600}}.mono{{font-family:Consolas,monospace}}
@media print{{body{{background:#fff;color:#111}}.card,.kv{{background:#fff;border-color:#ddd}}}}
</style></head><body><div class=\"wrap\">
<header><div><h1>SSD Sentinel v{} — {}</h1><div class=\"meta\">{}: {} · {}: {}</div></div>
<div class=\"meta\">{}: <strong>{}</strong> · {}: <strong>{}</strong> · {}: <strong>{}</strong></div></header>",
        if en { "en" } else { "tr" },
        html_escape(version),
        t("Raporu", "Report"),
        html_escape(version),
        t("Tanı ve Sağlık Raporu", "Diagnostic & Health Report"),
        t("Rapor türü", "Report type"),
        html_escape(kind),
        t("Oluşturma", "Generated"),
        html_escape(&generated),
        t("Kaynak", "Source"),
        html_escape(source),
        t("Sağlık modu", "Health mode"),
        html_escape(health_mode),
        t("Seri maskeleme", "Serial masking"),
        if redact {
            t("Açık", "On")
        } else {
            t("Kapalı", "Off")
        }
    );

    if let Some(disks) = state["disks"].as_array() {
        for disk in disks {
            let model = disk["model"]
                .as_str()
                .unwrap_or(t("Bilinmeyen Disk", "Unknown Drive"));
            let kind_str = disk["type"].as_str().unwrap_or("—");
            let status = disk["health"]["status"].as_str().unwrap_or("unknown");
            let pct = disk["health"]["percent"]
                .as_u64()
                .map(|p| if en { format!("{p}%") } else { format!("%{p}") })
                .unwrap_or_else(|| t("Bilinmiyor", "Unknown").into());
            let temp = disk["metrics"]["temperature_c"]
                .as_f64()
                .map(|t| format!("{t:.0} °C"))
                .unwrap_or_else(|| "—".into());
            let hours = disk["metrics"]["power_on_hours"]
                .as_u64()
                .map(|h| format!("{h} {}", t("sa", "h")))
                .unwrap_or_else(|| "—".into());
            let serial = disk["serial"]
                .as_str()
                .unwrap_or(t("Bilinmiyor", "Unknown"));
            let firmware = disk["firmware"].as_str().unwrap_or("—");
            let bus = disk["bus_link"]["current"]
                .as_str()
                .or_else(|| disk["interface"].as_str())
                .unwrap_or("—");
            let bottleneck = disk["bus_link"]["bottleneck"] == true;
            let aligned = match disk["optimization"]["aligned_4k"].as_bool() {
                Some(true) => t("4K Hizalı", "4K Aligned"),
                Some(false) => t("Hizasız!", "Misaligned!"),
                None => "—",
            };
            let op_pct = disk["optimization"]["over_provisioning_percent"]
                .as_f64()
                .map(|p| {
                    if en {
                        format!("{p:.1}%")
                    } else {
                        format!("%{p:.1}")
                    }
                })
                .unwrap_or_else(|| "—".into());
            let cls = match status {
                "critical" => "critical",
                "warning" | "unknown" => "warning",
                _ => "",
            };
            let _ = writeln!(
                body,
                "<section class=\"card\"><div style=\"display:flex;justify-content:space-between;align-items:center;gap:12px;flex-wrap:wrap\"><h2>{} <span class=\"meta\">({})</span></h2><span class=\"pill {}\">{}: {} · {}</span></div>
<p class=\"meta\">{}</p>
<div class=\"grid\">
<div class=\"kv\"><span>{}</span><strong>{}</strong></div>
<div class=\"kv\"><span>{}</span><strong>{}</strong></div>
<div class=\"kv\"><span>{}</span><strong>{}</strong></div>
<div class=\"kv\"><span>{}</span><strong>{}</strong></div>
<div class=\"kv\"><span>{}</span><strong class=\"mono\">{} · {}</strong></div>
<div class=\"kv\"><span>{}</span><strong>{}{}</strong></div>
<div class=\"kv\"><span>{}</span><strong>{} · {}</strong></div>
</div>",
                html_escape(model),
                html_escape(kind_str),
                cls,
                t("Sağlık", "Health"),
                html_escape(&pct),
                html_escape(status),
                html_escape(disk["health"]["explanation"].as_str().unwrap_or("")),
                t("Sıcaklık", "Temperature"),
                html_escape(&temp),
                t("Çalışma Süresi", "Power-On Time"),
                html_escape(&hours),
                t("Kapasite", "Capacity"),
                html_escape(&fmt_bytes(&disk["metrics"]["capacity_bytes"])),
                t("Toplam Yazılan", "Total Written"),
                html_escape(&fmt_bytes(&disk["metrics"]["written_bytes"])),
                t("Seri / Firmware", "Serial / Firmware"),
                html_escape(serial),
                html_escape(firmware),
                t("Bağlantı Hattı", "Bus Link"),
                html_escape(bus),
                if bottleneck {
                    t(" (Darboğaz!)", " (Bottleneck!)")
                } else {
                    ""
                },
                t("4K Hizalama / OP", "4K Alignment / OP"),
                html_escape(aligned),
                html_escape(&op_pct)
            );
            if let Some(vols) = disk["volumes"].as_array().filter(|v| !v.is_empty()) {
                let _ = write!(
                    body,
                    "<h3>{}</h3><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
                    t("Bölümler", "Volumes"),
                    t("Harf", "Letter"),
                    t("Etiket", "Label"),
                    t("Dosya Sistemi", "File System"),
                    t("Boş / Toplam", "Free / Total")
                );
                for v in vols {
                    let _ = writeln!(
                        body,
                        "<tr><td class=\"mono\">{}</td><td>{}</td><td>{}</td><td>{} / {}</td></tr>",
                        html_escape(v["letter"].as_str().unwrap_or("?")),
                        html_escape(v["label"].as_str().unwrap_or(t("Yerel Disk", "Local Disk"))),
                        html_escape(v["filesystem"].as_str().unwrap_or("—")),
                        html_escape(&fmt_bytes(&v["free_bytes"])),
                        html_escape(&fmt_bytes(&v["size_bytes"]))
                    );
                }
                body.push_str("</tbody></table>");
            }
            if let Some(smart) = disk["smart"].as_array().filter(|s| !s.is_empty()) {
                let _ = write!(
                    body,
                    "<h3>{}</h3><table><thead><tr><th>ID</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
                    t("SMART Alanları", "SMART Attributes"),
                    t("Alan", "Attribute"),
                    t("Ham Değer", "Raw Value"),
                    t("Güncel", "Current"),
                    t("En Kötü", "Worst"),
                    t("Eşik", "Threshold"),
                    t("Durum", "Status")
                );
                for r in smart {
                    let name = if en {
                        r["original"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .or_else(|| r["name"].as_str())
                            .unwrap_or("")
                    } else {
                        r["name"].as_str().unwrap_or("")
                    };
                    let _ = writeln!(
                        body,
                        "<tr><td class=\"mono\">{}</td><td>{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td>{}</td></tr>",
                        html_escape(r["id"].as_str().unwrap_or("")),
                        html_escape(name),
                        html_escape(r["raw"].as_str().unwrap_or("")),
                        r["value"].as_f64().map(|n| format!("{n:.0}")).unwrap_or_else(|| "—".into()),
                        r["worst"].as_f64().map(|n| format!("{n:.0}")).unwrap_or_else(|| "—".into()),
                        r["threshold"].as_f64().map(|n| format!("{n:.0}")).unwrap_or_else(|| "—".into()),
                        html_escape(r["level"].as_str().unwrap_or("ok"))
                    );
                }
                body.push_str("</tbody></table>");
            }
            body.push_str("</section>");
        }
    }

    if let Some(jobs) = jobs_val.as_ref().and_then(|v| v["jobs"].as_array()) {
        if !jobs.is_empty() {
            let _ = write!(
                body,
                "<section class=\"card\"><h2>{}</h2><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
                t("Kayıtlı Test / Doğrulama İşleri", "Recorded Test / Verification Jobs"),
                t("Tarih", "Date"),
                t("Disk", "Drive"),
                t("Bölüm", "Volume"),
                t("Tür", "Type"),
                t("Durum", "Status")
            );
            for j in jobs {
                let _ = writeln!(
                    body,
                    "<tr><td>{}</td><td>{}</td><td class=\"mono\">{}</td><td>{}</td><td>{}</td></tr>",
                    html_escape(j["started_at"].as_str().unwrap_or("—")),
                    html_escape(j["model"].as_str().unwrap_or("—")),
                    html_escape(j["letter"].as_str().unwrap_or("—")),
                    html_escape(j["kind"].as_str().unwrap_or(kind)),
                    html_escape(j["status"].as_str().unwrap_or("—"))
                );
            }
            body.push_str("</tbody></table></section>");
        }
    }

    body.push_str("</div></body></html>");
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_masking_hides_middle_and_short_serials() {
        assert_eq!(mask_serial("SN1234567890"), "SN••••••••90");
        assert_eq!(mask_serial("1234"), "••••");
        assert_eq!(mask_serial(""), "");
    }

    #[test]
    fn redact_value_masks_nested_serial_fields() {
        let mut val = json!({
            "disks": [{
                "model": "Test SSD",
                "serial": "ABCDEF123456",
                "os_serial": "XYZ987654321",
                "nvme_serial": "NVME12349876"
            }]
        });
        redact_value(&mut val);
        assert_eq!(val["disks"][0]["serial"], "AB••••••••56");
        assert_eq!(val["disks"][0]["os_serial"], "XY••••••••21");
        assert_eq!(val["disks"][0]["nvme_serial"], "NV••••••••76");
    }

    #[test]
    fn html_report_escapes_xss_and_redacts_serials() {
        let state = json!({
            "version": "1.0.0",
            "source": "live",
            "health_mode": "default",
            "disks": [{
                "model": "<script>alert(1)</script>",
                "type": "NVMe SSD",
                "serial": "SECRET9999SERIAL",
                "firmware": "FW1",
                "health": {"percent": 98, "status": "ok", "explanation": "<b>ok</b>"},
                "metrics": {"temperature_c": 40, "power_on_hours": 10, "capacity_bytes": 500000000000u64, "written_bytes": null},
                "bus_link": {"current": "PCIe 4.0 x4", "bottleneck": false},
                "optimization": {"aligned_4k": true, "over_provisioning_percent": 7.0},
                "volumes": [],
                "smart": []
            }]
        });
        let html = render_html_report("snapshot", &state, None, true);
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("SECRET9999SERIAL"));
        assert!(html.contains("SE••••••••AL"));
    }

    #[test]
    fn html_report_renders_clean_english_without_turkish_leakage() {
        let state = json!({
            "version": "1.0.0",
            "source": "live",
            "language": "en",
            "health_mode": "default",
            "disks": [{
                "model": "Test SSD",
                "type": "NVMe SSD",
                "serial": "ABCDEF123456",
                "firmware": "FW1",
                "health": {"percent": 98, "status": "ok", "explanation": "fine"},
                "metrics": {"temperature_c": 40, "power_on_hours": 10, "capacity_bytes": 500000000000u64, "written_bytes": null},
                "bus_link": {"current": "PCIe 4.0 x4", "bottleneck": true},
                "optimization": {"aligned_4k": false, "over_provisioning_percent": 7.0},
                "volumes": [{"letter": "C", "label": null, "filesystem": "NTFS", "free_bytes": 1000u64, "size_bytes": 2000u64}],
                "smart": [{"id": "5", "name": "Yeniden eşlenen sektör", "original": "Reallocated_Sector_Ct", "raw": "0", "value": 100, "worst": 100, "threshold": 10, "level": "ok"}]
            }]
        });
        let html = render_html_report(
            "snapshot",
            &state,
            Some(
                &json!({"jobs":[{"started_at":"x","model":"m","letter":"C","kind":"surface","status":"completed"}]}),
            ),
            true,
        );
        assert!(html.contains("<html lang=\"en\">"));
        assert!(
            html.contains("Diagnostic & Health Report")
                || html.contains("Diagnostic &amp; Health Report")
                || html.contains("Diagnostic & Health Report")
        );
        assert!(html.contains("SMART Attributes"));
        assert!(html.contains("Reallocated_Sector_Ct"));
        let turkish = ['ç', 'ğ', 'ı', 'ö', 'ş', 'ü', 'Ç', 'Ğ', 'İ', 'Ö', 'Ş', 'Ü'];
        assert!(
            !html.chars().any(|c| turkish.contains(&c)),
            "English report leaked Turkish characters"
        );
    }

    #[test]
    fn redact_value_hides_data_dir() {
        let mut v = json!({"data_dir": "C:\\Users\\someone\\x", "nested": {"data_dir": ""}});
        redact_value(&mut v);
        assert_eq!(v["data_dir"], "[redacted]");
        assert_eq!(v["nested"]["data_dir"], "");
    }
}
