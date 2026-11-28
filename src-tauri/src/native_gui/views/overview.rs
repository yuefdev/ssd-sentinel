use crate::i18n::pick;
use crate::native_gui::{
    linux_desktop,
    theme::{self, Palette},
    views::{small_kv, UiAppState, UiCommand},
};
use egui::{Color32, RichText};
use egui_plot::{Corner, Legend, Line, Plot, PlotPoints};
use serde_json::Value;

pub fn show(
    ui: &mut egui::Ui,
    state: &Value,
    disk: Option<&Value>,
    ui_state: &mut UiAppState,
    p: &Palette,
    en: bool,
    tx: &mut Vec<UiCommand>,
) {
    let Some(disk) = disk else {
        theme::card_frame(p).show(ui, |ui| {
            ui.heading(pick(
                en,
                "Henüz disk bulunamadı",
                "No drives discovered yet",
            ));
            ui.label(
                RichText::new(state["error"].as_str().unwrap_or(pick(
                    en,
                    "Veriler yükleniyor...",
                    "Loading telemetry...",
                )))
                .color(p.muted),
            );
        });
        return;
    };

    let model = disk["model"].as_str().unwrap_or("Storage Device");
    let kind = disk["type"].as_str().unwrap_or("Unknown");
    let device = disk["device"].as_str().unwrap_or("—");
    let serial = disk["serial"].as_str().unwrap_or("—");
    let fw = disk["firmware"].as_str().unwrap_or("—");
    let health = &disk["health"];
    let pct = health["percent"].as_f64();
    let h_status = health["status"].as_str().unwrap_or("unknown");
    let h_color = theme::health_color(h_status, pct, p);
    let metrics = &disk["metrics"];
    let temp = metrics["temperature_c"].as_f64();
    let poh = metrics["power_on_hours"].as_u64();
    let cap = metrics["capacity_bytes"].as_u64();
    let written = metrics["written_bytes"].as_u64();

    // Top Hero Card
    theme::card_frame(p).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.heading(RichText::new(model).strong());
                    theme::badge(ui, kind, p.accent);
                    if let Some(sched) = disk["optimization"]["io_scheduler"].as_str() {
                        theme::badge(ui, &format!("I/O: {sched}"), p.muted);
                    }
                });
                ui.label(
                    RichText::new(format!("{device} · S/N: {serial} · FW: {fw}"))
                        .size(12.0)
                        .color(p.muted)
                        .monospace(),
                );
                if let Some(exp) = health["explanation"].as_str() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(exp).size(13.0));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(pick(en, "JSON Rapor", "Export JSON")).clicked() {
                    tx.push(UiCommand::ExportReport {
                        kind: "json".into(),
                        redact: true,
                    });
                }
                if ui.button(pick(en, "HTML Rapor", "Export HTML")).clicked() {
                    tx.push(UiCommand::ExportReport {
                        kind: "html".into(),
                        redact: true,
                    });
                }
            });
        });

        ui.add_space(10.0);
        ui.columns(4, |cols| {
            theme::card_frame(p)
                .fill(p.card_alt)
                .show(&mut cols[0], |ui| {
                    ui.label(
                        RichText::new(pick(en, "Sağlık Skoru", "Health Score"))
                            .size(12.0)
                            .color(p.muted),
                    );
                    let text = pct.map_or_else(|| "—".into(), |v| format!("{v:.0}%"));
                    ui.label(RichText::new(text).size(26.0).strong().color(h_color));
                    if let Some(v) = pct {
                        ui.add(egui::ProgressBar::new((v / 100.0) as f32).fill(h_color));
                    }
                });
            theme::card_frame(p)
                .fill(p.card_alt)
                .show(&mut cols[1], |ui| {
                    ui.label(
                        RichText::new(pick(en, "Sıcaklık", "Temperature"))
                            .size(12.0)
                            .color(p.muted),
                    );
                    let tc = match temp {
                        Some(t) if t >= 55.0 => p.crit,
                        Some(t) if t >= 48.0 => p.warn,
                        Some(_) => p.ok,
                        None => p.muted,
                    };
                    ui.label(
                        RichText::new(temp.map_or_else(|| "—".into(), |t| format!("{t:.0} °C")))
                            .size(26.0)
                            .strong()
                            .color(tc),
                    );
                    ui.label(
                        RichText::new(pick(en, "Throttle sınırı: 53°C+", "Throttle guard: 53°C+"))
                            .size(11.0)
                            .color(p.muted),
                    );
                });
            theme::card_frame(p)
                .fill(p.card_alt)
                .show(&mut cols[2], |ui| {
                    ui.label(
                        RichText::new(pick(en, "Kapasite / Yazılan", "Capacity / Written"))
                            .size(12.0)
                            .color(p.muted),
                    );
                    ui.label(RichText::new(theme::fmt_bytes(cap)).size(22.0).strong());
                    ui.label(
                        RichText::new(format!(
                            "{}: {} · {}: {}",
                            pick(en, "Yazım", "Writes"),
                            theme::fmt_bytes(written),
                            pick(en, "Saat", "Hours"),
                            poh.map_or_else(|| "—".into(), |h| h.to_string())
                        ))
                        .size(11.5)
                        .color(p.muted),
                    );
                });
            theme::card_frame(p)
                .fill(p.card_alt)
                .show(&mut cols[3], |ui| {
                    ui.label(
                        RichText::new(pick(en, "Veriyolu & Hizalama", "Bus Link & 4K"))
                            .size(12.0)
                            .color(p.muted),
                    );
                    let bus = disk["bus_link"]["current"].as_str().unwrap_or("—");
                    ui.label(RichText::new(bus).size(18.0).strong());
                    let aligned = disk["optimization"]["aligned_4k"].as_bool();
                    let bottleneck = disk["bus_link"]["bottleneck"] == true;
                    ui.horizontal(|ui| {
                        if let Some(a) = aligned {
                            theme::badge(
                                ui,
                                if a { "4K OK" } else { "4K Unaligned" },
                                if a { p.ok } else { p.warn },
                            );
                        }
                        if bottleneck {
                            theme::badge(ui, pick(en, "Darboğaz", "Bottleneck"), p.warn);
                        }
                    });
                });
        });
    });

    ui.add_space(10.0);

    // Volumes + Forecast Row
    ui.columns(2, |cols| {
        theme::card_frame(p).show(&mut cols[0], |ui| {
            ui.label(RichText::new(pick(en, "Bağlı Bölümler", "Mounted Volumes")).strong());
            ui.add_space(6.0);
            let vols = disk["volumes"].as_array().cloned().unwrap_or_default();
            if vols.is_empty() {
                ui.label(
                    RichText::new(pick(en, "Bağlı bölüm yok.", "No mounted volumes."))
                        .color(p.muted),
                );
            }
            for v in vols {
                let letter = v["letter"].as_str().unwrap_or("");
                let fs = v["filesystem"].as_str().unwrap_or("—");
                let free = v["free_bytes"].as_u64();
                let size = v["size_bytes"].as_u64();
                let used_frac = match (size, free) {
                    (Some(s), Some(f)) if s > 0 => {
                        Some(((s.saturating_sub(f)) as f32) / (s as f32))
                    }
                    _ => None,
                };
                ui.horizontal(|ui| {
                    theme::badge(ui, &theme::fmt_volume(letter), p.accent);
                    ui.label(RichText::new(v["label"].as_str().unwrap_or(fs)).strong());
                    ui.label(RichText::new(fs).size(11.5).color(p.muted));
                });
                if let Some(uf) = used_frac {
                    let bar_col = if uf > 0.90 { p.warn } else { p.accent };
                    ui.add(egui::ProgressBar::new(uf).fill(bar_col).text(format!(
                        "{} {} / {}",
                        theme::fmt_bytes(free),
                        pick(en, "boş", "free"),
                        theme::fmt_bytes(size)
                    )));
                }
                ui.add_space(6.0);
            }
        });

        theme::card_frame(p).show(&mut cols[1], |ui| {
            ui.label(
                RichText::new(pick(
                    en,
                    "Sağlık Öngörüsü & Optimizasyon Özeti",
                    "Health Forecast & Tuning",
                ))
                .strong(),
            );
            ui.add_space(6.0);
            if let Some(msg) = ui_state.forecast["summary"]
                .as_str()
                .or_else(|| ui_state.forecast["message"].as_str())
            {
                ui.label(msg);
                ui.add_space(6.0);
            } else {
                ui.label(
                    RichText::new(pick(
                        en,
                        "Yeterli geçmiş örnek toplandıkça yazım öngörüsü hesaplanır.",
                        "Wear forecast computes once sufficient multi-hour history exists.",
                    ))
                    .size(12.0)
                    .color(p.muted),
                );
                ui.add_space(6.0);
            }
            let opt = &disk["optimization"];
            small_kv(
                ui,
                pick(en, "Yazma Önbelleği", "Write Cache"),
                opt["write_cache"]
                    .as_bool()
                    .map_or("—", |b| if b { "Enabled" } else { "Disabled" }),
                p.muted,
            );
            small_kv(
                ui,
                pick(en, "Over-Provisioning", "Over-Provisioning"),
                &opt["over_provisioning_percent"]
                    .as_f64()
                    .map_or_else(|| "—".into(), |v| format!("{v:.1}%")),
                p.muted,
            );
            small_kv(
                ui,
                "PCIe ASPM",
                opt["pcie_aspm_name"].as_str().unwrap_or("—"),
                p.muted,
            );
            small_kv(
                ui,
                pick(en, "TRIM Zamanlayıcı", "fstrim.timer"),
                opt["trim_timer"]
                    .as_bool()
                    .map_or("—", |b| if b { "Active" } else { "Inactive" }),
                p.muted,
            );
            if ui_state.update_info.is_object() {
                ui.add_space(6.0);
                let has_u = ui_state.update_info["has_update"] == true;
                let latest = ui_state.update_info["latest"].as_str().unwrap_or("—");
                ui.horizontal(|ui| {
                    theme::badge(
                        ui,
                        if has_u {
                            "Update Available"
                        } else {
                            "Up to date"
                        },
                        if has_u { p.warn } else { p.ok },
                    );
                    ui.label(format!("v{latest}"));
                    if let Some(url) = ui_state.update_info["url"].as_str() {
                        if ui.link("Releases").clicked() {
                            let _ = linux_desktop::open_url_or_path(url);
                        }
                    }
                });
            }
        });
    });

    ui.add_space(10.0);

    // Live Perf + SMART History Charts
    ui.columns(2, |cols| {
        theme::card_frame(p).show(&mut cols[0], |ui| {
            ui.label(
                RichText::new(pick(
                    en,
                    "Canlı I/O Performansı (MB/sn)",
                    "Live Throughput (/proc/diskstats, MB/s)",
                ))
                .strong(),
            );
            let r_pts: PlotPoints = ui_state
                .perf_read_mb
                .iter()
                .enumerate()
                .map(|(i, v)| [i as f64, *v])
                .collect();
            let w_pts: PlotPoints = ui_state
                .perf_write_mb
                .iter()
                .enumerate()
                .map(|(i, v)| [i as f64, *v])
                .collect();
            Plot::new("live_io_plot")
                .height(175.0)
                .legend(Legend::default().position(Corner::LeftTop))
                .allow_drag(false)
                .allow_zoom(false)
                .show(ui, |plot_ui| {
                    plot_ui.line(
                        Line::new(r_pts)
                            .name(pick(en, "Okuma", "Read"))
                            .color(p.accent),
                    );
                    plot_ui.line(
                        Line::new(w_pts)
                            .name(pick(en, "Yazma", "Write"))
                            .color(Color32::from_rgb(168, 85, 247)),
                    );
                });
        });

        theme::card_frame(p).show(&mut cols[1], |ui| {
            ui.label(
                RichText::new(pick(en, "SMART Tarihçe Trendi", "SMART History Trend")).strong(),
            );
            let rows = ui_state.history.as_array().cloned().unwrap_or_default();
            let temp_pts: PlotPoints = rows
                .iter()
                .enumerate()
                .filter_map(|(i, r)| r["temperature_c"].as_f64().map(|t| [i as f64, t]))
                .collect();
            let hp_pts: PlotPoints = rows
                .iter()
                .enumerate()
                .filter_map(|(i, r)| r["health_percent"].as_f64().map(|h| [i as f64, h]))
                .collect();
            Plot::new("smart_hist_plot")
                .height(175.0)
                .legend(Legend::default().position(Corner::LeftTop))
                .allow_drag(false)
                .allow_zoom(false)
                .show(ui, |plot_ui| {
                    plot_ui.line(
                        Line::new(hp_pts)
                            .name(pick(en, "Sağlık %", "Health %"))
                            .color(p.ok),
                    );
                    plot_ui.line(
                        Line::new(temp_pts)
                            .name(pick(en, "Sıcaklık °C", "Temp °C"))
                            .color(p.warn),
                    );
                });
        });
    });
}
