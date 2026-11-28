use crate::i18n::pick;
use crate::native_gui::{
    modal::PendingManifest,
    theme::{self, Palette},
    views::{active_job, UiAppState, UiCommand},
};
use egui::{Color32, RichText};
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
        ui.label(pick(en, "Disk seçilmedi.", "No drive selected."));
        return;
    };
    let disk_id = disk["id"].as_str().unwrap_or("").to_owned();
    let busy = active_job(state);

    theme::card_frame(p).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading(RichText::new(pick(en, "SMART Öznitelikleri ve Donanım Günlükleri", "SMART Attributes & Device Self-Tests")).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(pick(en, "Günlükleri Oku", "Read Device Logs")).clicked() {
                    tx.push(UiCommand::ReadLogs(disk_id.clone()));
                }
                let has_st = disk["capabilities"]["self_test"] == true;
                if ui.add_enabled(!busy && has_st, egui::Button::new(pick(en, "Testi İptal Et", "Abort Test"))).clicked() {
                    ui_state.modal = Some(PendingManifest::disk_action(disk, "abort_test", None, en));
                }
                if ui.add_enabled(!busy && has_st, egui::Button::new(pick(en, "Uzun Self-Test", "Long Self-Test"))).clicked() {
                    ui_state.modal = Some(PendingManifest::disk_action(disk, "long_test", None, en));
                }
                if ui.add_enabled(!busy && has_st, egui::Button::new(pick(en, "Kısa Self-Test", "Short Self-Test"))).clicked() {
                    ui_state.modal = Some(PendingManifest::disk_action(disk, "short_test", None, en));
                }
            });
        });

        ui.add_space(8.0);
        let smart_rows = disk["smart"].as_array().cloned().unwrap_or_default();
        if smart_rows.is_empty() {
            ui.label(RichText::new(pick(en, "Bu aygıttan SMART tablosu alınamadı (UDisks2/smartctl veya Polkit yetkisi gerekebilir).", "No SMART attribute table reported (UDisks2/smartctl or Polkit authentication may be needed).")).color(p.muted));
        } else {
            egui::Grid::new("smart_attr_grid")
                .striped(true)
                .num_columns(6)
                .spacing([16.0, 5.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("ID").strong().size(12.0));
                    ui.label(RichText::new(pick(en, "Öznitelik", "Attribute")).strong().size(12.0));
                    ui.label(RichText::new(pick(en, "Ham Değer", "Raw Value")).strong().size(12.0));
                    ui.label(RichText::new("Value").strong().size(12.0));
                    ui.label(RichText::new("Worst").strong().size(12.0));
                    ui.label(RichText::new("Threshold").strong().size(12.0));
                    ui.end_row();
                    for row in smart_rows {
                        let lvl = row["level"].as_str().unwrap_or("ok");
                        let c = match lvl {
                            "critical" => p.crit,
                            "warning" => p.warn,
                            _ => p.text,
                        };
                        ui.label(RichText::new(row["id"].as_str().unwrap_or("—")).color(p.muted).monospace());
                        let name = row["name"].as_str().or_else(|| row["original"].as_str()).unwrap_or("—");
                        ui.label(RichText::new(name).color(c));
                        ui.label(RichText::new(row["raw"].as_str().unwrap_or("—")).monospace().color(c));
                        ui.label(row["value"].as_u64().map_or("—".into(), |v| v.to_string()));
                        ui.label(row["worst"].as_u64().map_or("—".into(), |v| v.to_string()));
                        ui.label(row["threshold"].as_u64().map_or("—".into(), |v| v.to_string()));
                        ui.end_row();
                    }
                });
        }
    });

    ui.add_space(10.0);

    ui.columns(2, |cols| {
        theme::card_frame(p).show(&mut cols[0], |ui| {
            ui.label(
                RichText::new(pick(
                    en,
                    "Aktif Uyarılar ve Olaylar",
                    "Active Alerts & Recent Events",
                ))
                .strong(),
            );
            ui.add_space(6.0);
            for a in disk["alerts"].as_array().into_iter().flatten() {
                let lvl = a["level"].as_str().unwrap_or("info");
                let col: Color32 = if lvl == "critical" { p.crit } else { p.warn };
                ui.colored_label(
                    col,
                    format!(
                        "[{}] {}",
                        a["title"].as_str().unwrap_or(""),
                        a["detail"].as_str().unwrap_or("")
                    ),
                );
            }
            ui.separator();
            for ev in ui_state.events.as_array().into_iter().flatten().take(12) {
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        ev["created_at"].as_str().unwrap_or(""),
                        ev["detail"]
                            .as_str()
                            .or_else(|| ev["title"].as_str())
                            .unwrap_or("")
                    ))
                    .size(11.5)
                    .color(p.muted),
                );
            }
        });

        theme::card_frame(p).show(&mut cols[1], |ui| {
            ui.label(
                RichText::new(pick(
                    en,
                    "NVMe / ATA Hata ve Test Günlükleri",
                    "NVMe / ATA Log Viewer",
                ))
                .strong(),
            );
            ui.add_space(6.0);
            if ui_state.smart_logs.is_null() {
                ui.label(
                    RichText::new(pick(
                        en,
                        "Aygıt günlüklerini okumak için sağ üstteki düğmeyi kullanın.",
                        "Use 'Read Device Logs' above to query controller logs.",
                    ))
                    .size(12.0)
                    .color(p.muted),
                );
            } else {
                let pretty = serde_json::to_string_pretty(&ui_state.smart_logs["logs"])
                    .unwrap_or_else(|_| ui_state.smart_logs.to_string());
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(
                            RichText::new(pretty).monospace().size(11.5),
                        ));
                    });
            }
        });
    });
}
