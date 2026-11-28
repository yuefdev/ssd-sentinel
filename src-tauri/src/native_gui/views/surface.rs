use crate::core::JobRequest;
use crate::i18n::pick;
use crate::native_gui::{
    modal::PendingManifest,
    theme::{self, Palette},
    views::{active_job, selected_volume, UiAppState, UiCommand},
};
use egui::{CornerRadius, Rect, RichText, Sense, Vec2};
use egui_plot::{Bar, BarChart, Plot};
use serde_json::{json, Value};

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
        ui.label(pick(en, "Disk seçilmedi.", "No drive selected."));
        return;
    };
    let disk_id = disk["id"].as_str().unwrap_or("").to_owned();
    let busy = active_job(state);
    let vol = selected_volume(disk, ui_state);

    // 1. Benchmark Runner & Results
    theme::card_frame(p).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading(
                RichText::new(pick(
                    en,
                    "Hız Testi (io_uring / O_DIRECT)",
                    "Storage Benchmark (io_uring / O_DIRECT)",
                ))
                .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !busy && vol.is_some(),
                        egui::Button::new(pick(en, "Hız Testini Başlat", "Run Benchmark")),
                    )
                    .clicked()
                {
                    if let Some(ref letter) = vol {
                        let opts = match ui_state.bench_mode.as_str() {
                            "sustained" => {
                                json!({"profile": "sustained", "mode": "both", "size_mib": 1024})
                            }
                            "peak" => json!({"profile": "peak", "mode": "read", "size_mib": 256}),
                            _ => json!({"profile": "standard", "mode": "read", "size_mib": 256}),
                        };
                        ui_state.modal = Some(PendingManifest::job(
                            disk,
                            JobRequest {
                                kind: "benchmark".into(),
                                disk_id: disk_id.clone(),
                                letter: letter.clone(),
                                options: opts,
                                confirmation: String::new(),
                            },
                            en,
                        ));
                    }
                }
                for (id, lbl) in [
                    ("sustained", pick(en, "Sürekli / SLC", "Sustained / SLC")),
                    ("peak", pick(en, "Tepe Okuma", "Peak NVMe")),
                    ("standard", pick(en, "Standart", "Standard")),
                ] {
                    if ui
                        .selectable_label(ui_state.bench_mode == id, lbl)
                        .clicked()
                    {
                        ui_state.bench_mode = id.to_owned();
                    }
                }
            });
        });

        let cur_job = &state["job"];
        let active_bench = if cur_job["kind"] == "benchmark" {
            Some(cur_job)
        } else {
            ui_state.bench_jobs.as_array().and_then(|a| a.first())
        };
        if let Some(bj) = active_bench {
            let rows = bj["rows"].as_array().cloned().unwrap_or_default();
            if !rows.is_empty() {
                ui.add_space(8.0);
                ui.columns(2, |cols| {
                    egui::Grid::new("bench_rows")
                        .striped(true)
                        .num_columns(4)
                        .spacing([14.0, 4.0])
                        .show(&mut cols[0], |ui| {
                            ui.label(
                                RichText::new(pick(en, "Profil", "Profile"))
                                    .strong()
                                    .size(12.0),
                            );
                            ui.label(RichText::new("MB/s").strong().size(12.0));
                            ui.label(RichText::new("IOPS").strong().size(12.0));
                            ui.label(RichText::new("Lat (ms)").strong().size(12.0));
                            ui.end_row();
                            for r in &rows {
                                ui.label(r["label"].as_str().unwrap_or("—"));
                                ui.label(
                                    r["mb_s"].as_f64().map_or("—".into(), |v| format!("{v:.1}")),
                                );
                                ui.label(
                                    r["iops"].as_f64().map_or("—".into(), |v| format!("{v:.0}")),
                                );
                                ui.label(
                                    r["latency_ms"]
                                        .as_f64()
                                        .map_or("—".into(), |v| format!("{v:.2}")),
                                );
                                ui.end_row();
                            }
                        });
                    let bars: Vec<Bar> = rows
                        .iter()
                        .enumerate()
                        .filter_map(|(i, r)| {
                            let mb = r["mb_s"].as_f64()?;
                            let lbl = r["label"].as_str().unwrap_or("").to_owned();
                            Some(Bar::new(i as f64, mb).name(lbl))
                        })
                        .collect();
                    Plot::new("bench_bar_plot")
                        .height(150.0)
                        .allow_drag(false)
                        .allow_zoom(false)
                        .show(&mut cols[1], |plot_ui| {
                            plot_ui.bar_chart(BarChart::new(bars).color(p.accent).width(0.6));
                        });
                });
            }
        }
    });

    ui.add_space(10.0);

    // 2. Surface Verification, LBA Map (200 zones), Checkpoint & Targeted Repair
    theme::card_frame(p).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading(
                RichText::new(pick(
                    en,
                    "Yüzey Doğrulama ve Nokta Atışı Onarım",
                    "Surface Scan & Targeted Repair",
                ))
                .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !busy && vol.is_some(),
                        egui::Button::new(pick(en, "Taramayı Başlat", "Start Surface Job")),
                    )
                    .clicked()
                {
                    if let Some(ref letter) = vol {
                        ui_state.modal = Some(PendingManifest::job(
                            disk,
                            JobRequest {
                                kind: "surface".into(),
                                disk_id: disk_id.clone(),
                                letter: letter.clone(),
                                options: json!({"mode": ui_state.surface_mode}),
                                confirmation: String::new(),
                            },
                            en,
                        ));
                    }
                }
                for (id, lbl) in [
                    (
                        "targeted_repair",
                        pick(en, "Nokta Onarım", "Targeted Repair"),
                    ),
                    ("data_refresh", pick(en, "Veri Yenile", "Data Refresh")),
                    (
                        "physical_scan",
                        pick(en, "Tam Sektör (R/O)", "Physical Scan"),
                    ),
                    ("read_scan", pick(en, "Hızlı Doğrulama", "Read Scan")),
                ] {
                    if ui
                        .selectable_label(ui_state.surface_mode == id, lbl)
                        .clicked()
                    {
                        ui_state.surface_mode = id.to_owned();
                    }
                }
            });
        });

        if ui_state.checkpoint.is_object() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                theme::badge(
                    ui,
                    pick(
                        en,
                        "Kayıtlı Duraklatma Noktası Var",
                        "Saved Checkpoint Available",
                    ),
                    p.warn,
                );
                if ui
                    .small_button(pick(en, "Noktayı Temizle", "Clear Checkpoint"))
                    .clicked()
                {
                    tx.push(UiCommand::ClearCheckpoint(disk_id.clone()));
                }
            });
        }

        let cur_job = &state["job"];
        let active_surface = if cur_job["kind"] == "surface" {
            Some(cur_job)
        } else {
            ui_state.surface_jobs.as_array().and_then(|a| a.first())
        };
        if let Some(sj) = active_surface {
            let blocks = sj["blocks"].as_array().cloned().unwrap_or_default();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(pick(
                        en,
                        "200-Zon LBA Yüzey Haritası",
                        "200-Zone LBA Surface Heatmap",
                    ))
                    .size(12.5)
                    .strong(),
                );
                theme::badge(ui, pick(en, "Sağlam", "OK"), p.ok);
                theme::badge(ui, pick(en, "Yavaş / Zayıf", "Slow / Weak"), p.warn);
                theme::badge(ui, pick(en, "Bozuk", "Bad"), p.crit);
            });
            ui.add_space(4.0);
            let cols_n = 25usize;
            let rows_n = 8usize;
            let cell = 18.0f32;
            let gap = 3.0f32;
            let total_w = cols_n as f32 * (cell + gap);
            let total_h = rows_n as f32 * (cell + gap);
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, total_h), Sense::hover());
            let painter = ui.painter();
            let mut hovered_tip: Option<String> = None;
            let pointer = resp.hover_pos();

            for idx in 0..200usize {
                let r = idx / cols_n;
                let c = idx % cols_n;
                let x = rect.left() + c as f32 * (cell + gap);
                let y = rect.top() + r as f32 * (cell + gap);
                let crect = Rect::from_min_size(egui::pos2(x, y), Vec2::splat(cell));
                let blk = blocks.get(idx);
                let st = blk
                    .and_then(|b| b["status"].as_str())
                    .unwrap_or("unscanned");
                let fill = match st {
                    "ok" | "repaired" | "healed" => p.ok,
                    "slow" | "weak" | "relocated" => p.warn,
                    "bad" | "error" | "quarantined" | "dead" => p.crit,
                    _ => p.card_alt,
                };
                painter.rect_filled(crect, CornerRadius::same(3), fill);
                if let Some(pos) = pointer {
                    if crect.contains(pos) {
                        let ms = blk
                            .and_then(|b| b["ms"].as_f64())
                            .map_or("—".into(), |m| format!("{m:.2} ms"));
                        hovered_tip = Some(format!("Zone #{idx}: {st} · {ms}"));
                    }
                }
            }
            if let Some(tip) = hovered_tip {
                resp.on_hover_text(tip);
            }
        }
    });
}
