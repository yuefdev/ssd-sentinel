use crate::core::JobRequest;
use crate::dev_workspace;
use crate::i18n::pick;
use crate::native_gui::{
    modal::{ConfirmedAction, PendingManifest},
    theme::{self, Palette},
    views::{active_job, selected_volume, UiAppState, UiCommand},
};
use egui::RichText;
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
    let is_hdd = disk["type"].as_str() == Some("HDD");
    let busy = active_job(state);
    let vol = selected_volume(disk, ui_state);

    // 1. System & Kernel Tuning
    theme::card_frame(p).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading(
                RichText::new(pick(
                    en,
                    "Linux Sistem ve Disk Optimizasyonu",
                    "Linux System & Storage Optimization",
                ))
                .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !busy && vol.is_some(),
                        egui::Button::new(pick(en, "Tek Tıkla Otopilot", "One-Click Autopilot")),
                    )
                    .clicked()
                {
                    if let Some(ref letter) = vol {
                        ui_state.modal = Some(PendingManifest::job(
                            disk,
                            JobRequest {
                                kind: "autopilot".into(),
                                disk_id: disk_id.clone(),
                                letter: letter.clone(),
                                options: json!({"profile": "auto"}),
                                confirmation: String::new(),
                            },
                            en,
                        ));
                    }
                }
                if is_hdd {
                    if ui
                        .add_enabled(
                            !busy && vol.is_some(),
                            egui::Button::new(pick(en, "HDD Birleştir (Defrag)", "HDD Defrag")),
                        )
                        .clicked()
                    {
                        if let Some(ref letter) = vol {
                            ui_state.modal = Some(PendingManifest::job(
                                disk,
                                JobRequest {
                                    kind: "defrag".into(),
                                    disk_id: disk_id.clone(),
                                    letter: letter.clone(),
                                    options: json!({"mode": "defrag"}),
                                    confirmation: String::new(),
                                },
                                en,
                            ));
                        }
                    }
                } else if let Some(ref letter) = vol {
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(pick(en, "Şimdi ReTrim (fstrim)", "Run ReTrim Now")),
                        )
                        .clicked()
                    {
                        ui_state.modal = Some(PendingManifest::disk_action(
                            disk,
                            "retrim",
                            Some(letter.clone()),
                            en,
                        ));
                    }
                }
            });
        });

        ui.add_space(8.0);
        let opt = &disk["optimization"];
        let wc = opt["write_cache"].as_bool().unwrap_or(false);
        let fstrim_on = opt["trim_timer"].as_bool().unwrap_or(false);
        let cur_sched = opt["io_scheduler"].as_str().unwrap_or("none");
        let avail_scheds: Vec<String> = opt["available_schedulers"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![
                    "none".into(),
                    "mq-deadline".into(),
                    "bfq".into(),
                    "kyber".into(),
                ]
            });

        ui.columns(2, |cols| {
            theme::card_frame(p)
                .fill(p.card_alt)
                .show(&mut cols[0], |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(pick(en, "Yazma Önbelleği", "Write Cache")).strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let next_val = if wc { "disable" } else { "enable" };
                            let btn = if wc {
                                pick(en, "Devre Dışı Bırak", "Disable")
                            } else {
                                pick(en, "Etkinleştir", "Enable")
                            };
                            if ui.button(btn).clicked() {
                                ui_state.modal = Some(PendingManifest::optimize(
                                    disk,
                                    "write_cache",
                                    &disk_id,
                                    next_val,
                                    en,
                                ));
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("systemd fstrim.timer").strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let next_val = if fstrim_on { "disable" } else { "enable" };
                            let btn = if fstrim_on {
                                pick(en, "Kapat", "Disable")
                            } else {
                                pick(en, "Aç", "Enable")
                            };
                            if ui.button(btn).clicked() {
                                ui_state.modal = Some(PendingManifest::optimize(
                                    disk,
                                    "ntfs_trim",
                                    "system",
                                    next_val,
                                    en,
                                ));
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(pick(en, "Zamanlayıcı (I/O Scheduler)", "I/O Scheduler"))
                                .strong(),
                        );
                        ui.label(RichText::new(cur_sched).color(p.accent).monospace());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for s in &avail_scheds {
                                if s != cur_sched && ui.small_button(s).clicked() {
                                    ui_state.modal = Some(PendingManifest::optimize(
                                        disk,
                                        "io_scheduler",
                                        &disk_id,
                                        s,
                                        en,
                                    ));
                                }
                            }
                        });
                    });
                });

            theme::card_frame(p)
                .fill(p.card_alt)
                .show(&mut cols[1], |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("PCIe ASPM").strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("performance").clicked() {
                                ui_state.modal = Some(PendingManifest::optimize(
                                    disk,
                                    "pcie_aspm",
                                    "system",
                                    "performance",
                                    en,
                                ));
                            }
                            if ui.small_button("default").clicked() {
                                ui_state.modal = Some(PendingManifest::optimize(
                                    disk,
                                    "pcie_aspm",
                                    "system",
                                    "default",
                                    en,
                                ));
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(pick(
                                en,
                                "Bellek Takası (vm.swappiness)",
                                "Swap Tuning (vm.swappiness)",
                            ))
                            .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("SSD (10)").clicked() {
                                ui_state.modal = Some(PendingManifest::optimize(
                                    disk,
                                    "hibernation",
                                    "system",
                                    "disable",
                                    en,
                                ));
                            }
                            if ui.small_button("Default (60)").clicked() {
                                ui_state.modal = Some(PendingManifest::optimize(
                                    disk,
                                    "hibernation",
                                    "system",
                                    "enable",
                                    en,
                                ));
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(pick(
                                en,
                                "Otomatik Başlatma (.desktop)",
                                "Autostart (.desktop)",
                            ))
                            .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(pick(en, "Aç", "Enable")).clicked() {
                                ui_state.modal = Some(PendingManifest::optimize(
                                    disk,
                                    "autostart",
                                    "system",
                                    "enable",
                                    en,
                                ));
                            }
                            if ui.small_button(pick(en, "Kapat", "Disable")).clicked() {
                                ui_state.modal = Some(PendingManifest::optimize(
                                    disk,
                                    "autostart",
                                    "system",
                                    "disable",
                                    en,
                                ));
                            }
                        });
                    });
                });
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if let Some(ref letter) = vol {
                if ui
                    .small_button(pick(
                        en,
                        "OP & Bölüm Sınırlarını Sorgula",
                        "Inspect OP / Partition Bounds",
                    ))
                    .clicked()
                {
                    tx.push(UiCommand::QueryOpBounds {
                        disk_id: disk_id.clone(),
                        letter: letter.clone(),
                    });
                }
            }
            if let Some(msg) = ui_state.op_bounds["message"].as_str() {
                ui.label(RichText::new(msg).size(12.0).color(p.muted));
            }
        });
    });

    ui.add_space(10.0);

    // 2. Smart Storage Tiering & Dev Workspace Cleaner
    ui.columns(2, |cols| {
        theme::card_frame(p).show(&mut cols[0], |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(pick(en, "Akıllı Depolama Katmanlama", "Smart Storage Tiering")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(ref letter) = vol {
                        if ui.button(pick(en, "Bölümü Analiz Et", "Analyze Tiering")).clicked() {
                            tx.push(UiCommand::AnalyzeTier {
                                disk_id: disk_id.clone(),
                                letter: letter.clone(),
                            });
                        }
                    }
                });
            });
            ui.add_space(6.0);
            if let Some(sum) = ui_state.tier_analysis["summary_tr"].as_str().or_else(|| ui_state.tier_analysis["summary"].as_str()) {
                ui.label(RichText::new(sum).size(12.0).color(p.muted));
            }
            let companion_letter = ui_state.tier_analysis["companion"]["letter"].as_str().map(str::to_owned);
            for c in ui_state.tier_analysis["candidates"].as_array().into_iter().flatten().take(8) {
                let path = c["path"].as_str().unwrap_or("");
                let name = c["name"].as_str().unwrap_or("");
                let sz = c["size_bytes"].as_u64();
                let dir = c["recommended_direction"].as_str().unwrap_or("to_hdd");
                ui.horizontal(|ui| {
                    ui.label(name);
                    ui.label(RichText::new(theme::fmt_bytes(sz)).size(11.5).color(p.muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(ref tgt) = companion_letter {
                            if ui.small_button(format!("-> {}", theme::fmt_volume(tgt))).clicked() {
                                let token = format!("tier_relocate:{disk_id}:{}", crate::platform::normalize_volume_id(tgt));
                                ui_state.modal = Some(PendingManifest {
                                    title: pick(en, "Katman Taşıma Onayı (Symlink + SHA-256)", "Confirm Tier Relocation").to_owned(),
                                    subtitle: path.to_owned(),
                                    rows: vec![
                                        (pick(en, "Kaynak", "Source").into(), path.to_owned()),
                                        (pick(en, "Hedef", "Target").into(), theme::fmt_volume(tgt)),
                                        (pick(en, "Yön", "Direction").into(), dir.to_owned()),
                                    ],
                                    warning: pick(
                                        en,
                                        "Dosya hedef bölüme SHA-256 doğrulamalı kopyalanır ve orijinal konumda sembolik bağlantı bırakılır.",
                                        "Copies with SHA-256 verification and leaves a POSIX symlink in place.",
                                    ).into(),
                                    required_token: token.clone(),
                                    typed_input: String::new(),
                                    fast_confirm: true,
                                    action: ConfirmedAction::TierRelocate {
                                        disk_id: disk_id.clone(),
                                        source_path: path.to_owned(),
                                        target_letter: tgt.clone(),
                                        direction: dir.to_owned(),
                                        confirmation: token,
                                    },
                                });
                            }
                        }
                    });
                });
            }
            let moves = state["tier_moves"].as_array().cloned().unwrap_or_default();
            if !moves.is_empty() {
                ui.separator();
                ui.label(RichText::new(pick(en, "Aktif Taşımalar (Geri Alınabilir)", "Active Moves (Rollbackable)")).size(12.0).color(p.muted));
                for m in moves.iter().take(4) {
                    if m["status"].as_str() == Some("rolled_back") {
                        continue;
                    }
                    let mid = m["id"].as_str().unwrap_or("");
                    let src = m["source_path"].as_str().unwrap_or("");
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(src).size(11.5));
                        if ui.small_button(pick(en, "Geri Al", "Rollback")).clicked() {
                            let token = format!("tier_rollback:{mid}");
                            ui_state.modal = Some(PendingManifest {
                                title: pick(en, "Taşımayı Geri Al", "Rollback Tier Move").into(),
                                subtitle: src.into(),
                                rows: vec![("Move ID".into(), mid.into())],
                                warning: pick(en, "Veri orijinal bölüme geri taşınır.", "Restores data back to original location.").into(),
                                required_token: token.clone(),
                                typed_input: String::new(),
                                fast_confirm: true,
                                action: ConfirmedAction::TierRollback {
                                    move_id: mid.into(),
                                    confirmation: token,
                                },
                            });
                        }
                    });
                }
            }
        });

        theme::card_frame(p).show(&mut cols[1], |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(pick(en, "Geliştirici Çalışma Alanı Temizleyici", "Developer Workspace Cleaner")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(ref letter) = vol {
                        if ui.button(pick(en, "Klasör Seç...", "Pick Folder...")).clicked() {
                            tx.push(UiCommand::PickDevFolder {
                                disk_id: disk_id.clone(),
                                letter: letter.clone(),
                            });
                        }
                        if ui.button(pick(en, "Tara", "Scan")).clicked() {
                            let custom = (!ui_state.custom_dev_root.trim().is_empty())
                                .then(|| ui_state.custom_dev_root.trim().to_owned());
                            tx.push(UiCommand::AnalyzeDev {
                                disk_id: disk_id.clone(),
                                letter: letter.clone(),
                                custom_root: custom,
                            });
                        }
                    }
                });
            });
            ui.add_space(6.0);
            for art in ui_state.dev_analysis["artifacts"].as_array().into_iter().flatten().take(10) {
                let path = art["path"].as_str().unwrap_or("");
                let label = art["kind"].as_str().unwrap_or("artifact");
                let sz = art["size_bytes"].as_u64();
                let can_clean = art["can_clean"] != false;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).strong());
                    ui.label(RichText::new(theme::fmt_bytes(sz)).size(11.5).color(p.muted));
                    ui.label(RichText::new(path).size(11.0).color(p.muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if can_clean && ui.small_button(pick(en, "Temizle", "Clean")).clicked() {
                            let token = dev_workspace::clean_token(&disk_id, path);
                            ui_state.modal = Some(PendingManifest {
                                title: pick(en, "Derleme Artığı Temizliği", "Clean Build Artifact").into(),
                                subtitle: path.into(),
                                rows: vec![
                                    (pick(en, "Yol", "Path").into(), path.into()),
                                    (pick(en, "Boyut", "Size").into(), theme::fmt_bytes(sz)),
                                ],
                                warning: pick(
                                    en,
                                    "Kaynak kod, .git ve AI modelleri korunur; yalnızca seçili derleme önbelleği silinir.",
                                    "Source files, .git and AI model weights remain untouched; only the selected build cache is removed.",
                                ).into(),
                                required_token: token.clone(),
                                typed_input: String::new(),
                                fast_confirm: false,
                                action: ConfirmedAction::DevClean {
                                    disk_id: disk_id.clone(),
                                    path: path.into(),
                                    confirmation: token,
                                },
                            });
                        }
                    });
                });
            }
        });
    });
}
