use crate::core::JobRequest;
use crate::i18n::pick;
use crate::native_gui::{
    modal::PendingManifest,
    theme::{self, Palette},
    views::{active_job, selected_volume, UiAppState, UiCommand},
};
use egui::{ColorImage, RichText};
use serde_json::{json, Value};

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity((input.len() * 3) / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for &b in input.as_bytes() {
        if b.is_ascii_whitespace() || b == b'=' {
            continue;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

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

    theme::card_frame(p).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading(
                RichText::new(pick(
                    en,
                    "Veri Kurtarma ve Adli Tarama",
                    "Data Recovery & Linux Forensics",
                ))
                .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !busy && vol.is_some(),
                        egui::Button::new(pick(en, "Taramayı Başlat", "Start Scan")),
                    )
                    .clicked()
                {
                    if let Some(ref letter) = vol {
                        ui_state.rec_selected_ids.clear();
                        ui_state.modal = Some(PendingManifest::job(
                            disk,
                            JobRequest {
                                kind: "recovery".into(),
                                disk_id: disk_id.clone(),
                                letter: letter.clone(),
                                options: json!({"mode": ui_state.rec_mode}),
                                confirmation: String::new(),
                            },
                            en,
                        ));
                    }
                }
                for (id, lbl) in [
                    ("dev", pick(en, "Geliştirici / .env", "Dev / .env")),
                    ("apps_only", pick(en, "Kayıp Uygulamalar", "Lost Apps")),
                    ("deep", pick(en, "Derin Kazıma", "Deep Carving")),
                    (
                        "quick",
                        pick(en, "Hızlı (Çöp + Adli)", "Quick (Trash + Snapshots)"),
                    ),
                ] {
                    if ui.selectable_label(ui_state.rec_mode == id, lbl).clicked() {
                        ui_state.rec_mode = id.to_owned();
                    }
                }
            });
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .selectable_label(
                    !ui_state.rec_subtab_apps,
                    pick(en, "Kurtarılabilir Dosyalar", "Recoverable Files"),
                )
                .clicked()
            {
                ui_state.rec_subtab_apps = false;
            }
            if ui
                .selectable_label(
                    ui_state.rec_subtab_apps,
                    pick(en, "Silinen Uygulamalar", "Deleted Apps"),
                )
                .clicked()
            {
                ui_state.rec_subtab_apps = true;
            }
            ui.add_space(12.0);
            ui.label(pick(en, "Ara:", "Filter:"));
            ui.text_edit_singleline(&mut ui_state.rec_search);
        });
    });

    ui.add_space(10.0);
    let job = &state["job"];
    let job_id = job["id"].as_str().unwrap_or("").to_owned();
    let needle = ui_state.rec_search.trim().to_ascii_lowercase();

    if ui_state.rec_subtab_apps {
        theme::card_frame(p).show(ui, |ui| {
            let apps = job["deleted_apps"].as_array().cloned().unwrap_or_default();
            if apps.is_empty() {
                ui.label(
                    RichText::new(pick(
                        en,
                        "Kayıp uygulama tarama sonucu yok.",
                        "No deleted app bundles in current scan.",
                    ))
                    .color(p.muted),
                );
                return;
            }
            for app in apps.iter().take(60) {
                let name = app["name"].as_str().unwrap_or("App");
                if !needle.is_empty() && !name.to_ascii_lowercase().contains(&needle) {
                    continue;
                }
                theme::card_frame(p).fill(p.card_alt).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(name).strong());
                        theme::badge(ui, app["category"].as_str().unwrap_or("App"), p.accent);
                        theme::badge(
                            ui,
                            &format!("{}%", app["confidence"].as_u64().unwrap_or(0)),
                            p.ok,
                        );
                    });
                    ui.label(
                        RichText::new(app["exe_path"].as_str().unwrap_or("—"))
                            .size(11.5)
                            .color(p.muted),
                    );
                    if let Some(cmd) = app["winget_hint"].as_str() {
                        ui.label(
                            RichText::new(format!("{} {cmd}", pick(en, "Önerilen:", "Suggested:")))
                                .monospace()
                                .size(11.5),
                        );
                    }
                });
                ui.add_space(4.0);
            }
        });
        return;
    }

    let items = job["recovery_items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    ui.columns(2, |cols| {
        theme::card_frame(p).show(&mut cols[0], |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} ({})",
                        pick(en, "Bulunan Öğeler", "Discovered Items"),
                        items.len()
                    ))
                    .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_batch = !job_id.is_empty() && !ui_state.rec_selected_ids.is_empty();
                    if ui
                        .add_enabled(
                            can_batch,
                            egui::Button::new(pick(
                                en,
                                "Seçilenleri Kurtar...",
                                "Restore Selected...",
                            )),
                        )
                        .clicked()
                    {
                        tx.push(UiCommand::RestoreRecoveryBatch {
                            job_id: job_id.clone(),
                            item_ids: ui_state.rec_selected_ids.clone(),
                        });
                    }
                });
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(380.0)
                .show(ui, |ui| {
                    for item in items.iter().take(250) {
                        let id = item["id"].as_str().unwrap_or("").to_owned();
                        let name = item["name"].as_str().unwrap_or("file");
                        let orig = item["original_path"].as_str().unwrap_or("");
                        if !needle.is_empty()
                            && !name.to_ascii_lowercase().contains(&needle)
                            && !orig.to_ascii_lowercase().contains(&needle)
                        {
                            continue;
                        }
                        let mut checked = ui_state.rec_selected_ids.contains(&id);
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut checked, "").changed() {
                                if checked {
                                    ui_state.rec_selected_ids.push(id.clone());
                                } else {
                                    ui_state.rec_selected_ids.retain(|x| x != &id);
                                }
                            }
                            if ui.selectable_label(false, name).clicked() && !job_id.is_empty() {
                                tx.push(UiCommand::PreviewRecovery {
                                    job_id: job_id.clone(),
                                    item_id: id.clone(),
                                });
                            }
                            ui.label(
                                RichText::new(theme::fmt_bytes(item["size_bytes"].as_u64()))
                                    .size(11.5)
                                    .color(p.muted),
                            );
                            let conf = item["confidence"].as_u64().unwrap_or(0);
                            let col = if item["trim_zeroed"] == true {
                                p.crit
                            } else if conf >= 75 {
                                p.ok
                            } else {
                                p.warn
                            };
                            theme::badge(ui, &format!("{conf}%"), col);
                        });
                    }
                });
        });

        theme::card_frame(p).show(&mut cols[1], |ui| {
            ui.label(RichText::new(pick(en, "Salt-Okunur Önizleme", "Read-Only Preview")).strong());
            ui.add_space(6.0);
            let kind = ui_state.rec_preview["kind"].as_str().unwrap_or("");
            match kind {
                "image" => {
                    if let Some(url) = ui_state.rec_preview["data_url"].as_str() {
                        let need_load = ui_state
                            .rec_preview_tex
                            .as_ref()
                            .is_none_or(|(k, _)| k != url);
                        if need_load {
                            if let Some((_, b64)) = url.split_once("base64,") {
                                if let Some(bytes) = decode_base64(b64) {
                                    if let Ok(dyn_img) = image::load_from_memory(&bytes) {
                                        let rgba = dyn_img.to_rgba8();
                                        let size = [rgba.width() as usize, rgba.height() as usize];
                                        let cimg = ColorImage::from_rgba_unmultiplied(size, &rgba);
                                        let tex = ui.ctx().load_texture(
                                            "rec_preview_img",
                                            cimg,
                                            egui::TextureOptions::LINEAR,
                                        );
                                        ui_state.rec_preview_tex = Some((url.to_owned(), tex));
                                    }
                                }
                            }
                        }
                        if let Some((_, ref tex)) = ui_state.rec_preview_tex {
                            let max_w = ui.available_width().min(360.0);
                            let ratio = tex.size()[1] as f32 / (tex.size()[0].max(1) as f32);
                            ui.image(egui::load::SizedTexture::new(
                                tex.id(),
                                [max_w, (max_w * ratio).min(300.0)],
                            ));
                        }
                    }
                }
                "text" => {
                    let txt = ui_state.rec_preview["text"].as_str().unwrap_or("");
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(txt).monospace().size(11.5)));
                        });
                }
                "binary" => {
                    let meta = &ui_state.rec_preview["metadata"];
                    ui.label(serde_json::to_string_pretty(meta).unwrap_or_default());
                }
                "unavailable" => {
                    ui.label(
                        RichText::new(
                            ui_state.rec_preview["reason"]
                                .as_str()
                                .unwrap_or("Unavailable"),
                        )
                        .color(p.muted),
                    );
                }
                _ => {
                    ui.label(
                        RichText::new(pick(
                            en,
                            "Önizlemek için soldan bir dosya seçin.",
                            "Click a file on the left to inspect it read-only.",
                        ))
                        .color(p.muted),
                    );
                }
            }
        });
    });
}
