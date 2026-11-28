use crate::core::JobRequest;
use crate::native_gui::theme::{self, Palette};
use crate::optimize::OptimizeRequest;
use egui::{Color32, RichText};
use serde_json::Value;

#[derive(Clone, Debug)]
pub enum ConfirmedAction {
    StartJob(JobRequest),
    DiskAction {
        disk_id: String,
        action: String,
        confirmation: String,
        letter: Option<String>,
    },
    Optimize(OptimizeRequest),
    TierRelocate {
        disk_id: String,
        source_path: String,
        target_letter: String,
        direction: String,
        confirmation: String,
    },
    TierRollback {
        move_id: String,
        confirmation: String,
    },
    DevClean {
        disk_id: String,
        path: String,
        confirmation: String,
    },
}

#[derive(Clone, Debug)]
pub struct PendingManifest {
    pub title: String,
    pub subtitle: String,
    pub rows: Vec<(String, String)>,
    pub warning: String,
    pub required_token: String,
    pub typed_input: String,
    pub fast_confirm: bool,
    pub action: ConfirmedAction,
}

impl PendingManifest {
    pub fn disk_action(disk: &Value, action: &str, letter: Option<String>, en: bool) -> Self {
        let disk_id = disk["id"].as_str().unwrap_or("").to_owned();
        let model = disk["model"].as_str().unwrap_or("Disk");
        let serial = disk["serial"].as_str().unwrap_or("—");
        let token = format!("{action}:{disk_id}");
        let mut rows = vec![
            (theme_label(en, "Disk", "Drive"), model.to_owned()),
            (theme_label(en, "Seri", "Serial"), serial.to_owned()),
            (theme_label(en, "İşlem", "Operation"), action.to_owned()),
        ];
        if let Some(ref l) = letter {
            rows.push((theme_label(en, "Bölüm", "Volume"), theme::fmt_volume(l)));
        }
        Self {
            title: theme_label(en, "Donanım İşlemi Onayı", "Confirm Hardware Action"),
            subtitle: format!("Token: {token}"),
            rows,
            warning: if action == "retrim" {
                theme_label(
                    en,
                    "Kullanılmayan dosya sistemi bloklarına TRIM / fstrim gönderilir; mevcut verilere dokunulmaz.",
                    "Dispatches TRIM / fstrim to unused filesystem blocks without touching live files.",
                )
            } else {
                theme_label(
                    en,
                    "Bu komut doğrudan fiziksel diskin SMART denetleyicisine gönderilecektir.",
                    "This command is sent directly to the drive's SMART controller.",
                )
            },
            required_token: token.clone(),
            typed_input: String::new(),
            fast_confirm: action == "retrim" || action == "abort_test",
            action: ConfirmedAction::DiskAction {
                disk_id,
                action: action.to_owned(),
                confirmation: token,
                letter,
            },
        }
    }

    pub fn optimize(disk: &Value, action: &str, target: &str, value: &str, en: bool) -> Self {
        let token = format!("optimize:{action}:{target}");
        Self {
            title: theme_label(en, "Sistem Optimizasyon Bildirimi", "Dry-Run Execution Manifest"),
            subtitle: format!("{action} -> {target}"),
            rows: vec![
                (
                    theme_label(en, "Disk", "Drive"),
                    disk["model"].as_str().unwrap_or("System").to_owned(),
                ),
                (theme_label(en, "Eylem", "Action"), action.to_owned()),
                (theme_label(en, "Hedef", "Target"), target.to_owned()),
                (theme_label(en, "Değer", "Value"), value.to_owned()),
            ],
            warning: theme_label(
                en,
                "Değişiklik öncesi geri alma kaydı SQLite veritabanına yazılır (Read-Only Guard kapalı olmalıdır).",
                "A rollback state is saved before applying changes (requires Read-Only Guard unlocked).",
            ),
            required_token: token.clone(),
            typed_input: String::new(),
            fast_confirm: true,
            action: ConfirmedAction::Optimize(OptimizeRequest {
                action: action.to_owned(),
                target: target.to_owned(),
                value: value.to_owned(),
                confirmation: token,
            }),
        }
    }

    pub fn job(disk: &Value, mut req: JobRequest, en: bool) -> Self {
        let mode = req.options["mode"].as_str().unwrap_or("");
        let token = match req.kind.as_str() {
            "benchmark" | "autopilot" => format!("{}:{}:{}", req.kind, req.disk_id, req.letter),
            _ => format!("{}:{}:{}:{}", req.kind, req.disk_id, req.letter, mode),
        };
        req.confirmation = token.clone();
        let is_destructive = crate::core::Core::is_mutating_job(&req.kind, &req.options);
        Self {
            title: theme_label(
                en,
                "Disk Görevi Yürütme Bildirimi",
                "Disk Job Execution Manifest",
            ),
            subtitle: format!("{} · {}", req.kind, theme::fmt_volume(&req.letter)),
            rows: vec![
                (
                    theme_label(en, "Disk", "Drive"),
                    disk["model"].as_str().unwrap_or("Disk").to_owned(),
                ),
                (
                    theme_label(en, "Aygıt / Seri", "Device / Serial"),
                    format!(
                        "{} · {}",
                        disk["device"].as_str().unwrap_or("—"),
                        disk["serial"].as_str().unwrap_or("—")
                    ),
                ),
                (
                    theme_label(en, "Bölüm", "Mount / Volume"),
                    theme::fmt_volume(&req.letter),
                ),
                (
                    theme_label(en, "Görev", "Job"),
                    format!("{} ({mode})", req.kind),
                ),
            ],
            warning: if is_destructive {
                theme_label(
                    en,
                    "Bu görev diske geçici test/onarım verisi yazar. Devam etmek için aşağıdaki onay kodunu doğrulayın.",
                    "This job performs bounded writes/repairs on the target storage. Verify the confirmation token below to proceed.",
                )
            } else {
                theme_label(
                    en,
                    "Salt-okunur teşhis/kurtarma görevi başlatılacaktır.",
                    "A read-only diagnostic/recovery job will run.",
                )
            },
            required_token: token,
            typed_input: String::new(),
            fast_confirm: !is_destructive,
            action: ConfirmedAction::StartJob(req),
        }
    }
}

fn theme_label(en: bool, tr: &str, en_s: &str) -> String {
    if en { en_s } else { tr }.to_owned()
}

pub enum ModalOutcome {
    None,
    Cancelled,
    Confirmed(ConfirmedAction),
}

pub fn show_manifest_modal(
    ctx: &egui::Context,
    manifest: &mut PendingManifest,
    read_only_guard: bool,
    en: bool,
    p: &Palette,
) -> ModalOutcome {
    let mut outcome = ModalOutcome::None;
    egui::Window::new(&manifest.title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .fixed_size([520.0, 0.0])
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(p.card)
                .stroke(egui::Stroke::new(1.2_f32, p.accent)),
        )
        .show(ctx, |ui| {
            ui.label(RichText::new(&manifest.subtitle).color(p.accent).strong());
            ui.add_space(8.0);
            theme::card_frame(p).fill(p.card_alt).show(ui, |ui| {
                egui::Grid::new("manifest_rows")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        for (k, v) in &manifest.rows {
                            ui.label(RichText::new(k).color(p.muted).size(12.0));
                            ui.label(RichText::new(v).monospace().size(12.0));
                            ui.end_row();
                        }
                    });
            });
            ui.add_space(8.0);
            ui.label(RichText::new(&manifest.warning).color(p.warn).size(12.5));

            let needs_unlock = match &manifest.action {
                ConfirmedAction::StartJob(req) => {
                    crate::core::Core::is_mutating_job(&req.kind, &req.options) && read_only_guard
                }
                ConfirmedAction::DiskAction { action, .. } => {
                    action != "logs" && read_only_guard
                }
                _ => read_only_guard,
            };
            if needs_unlock {
                ui.add_space(6.0);
                ui.colored_label(
                    p.crit,
                    crate::i18n::pick(
                        en,
                        "Salt-Okunur Koruma açık! Yazma/değişiklik işlemi için üst çubuktan kilidi açın.",
                        "Read-Only Guard is active! Unlock it in the top bar before running mutating operations.",
                    ),
                );
            }

            if !manifest.fast_confirm {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!(
                        "{}: {}",
                        crate::i18n::pick(en, "Onay kodu yazın", "Type confirmation token"),
                        manifest.required_token
                    ))
                    .size(12.0)
                    .color(p.muted),
                );
                ui.text_edit_singleline(&mut manifest.typed_input);
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .button(crate::i18n::pick(en, "İptal", "Cancel"))
                    .clicked()
                {
                    outcome = ModalOutcome::Cancelled;
                }
                let token_ok =
                    manifest.fast_confirm || manifest.typed_input.trim() == manifest.required_token;
                let btn = egui::Button::new(
                    RichText::new(crate::i18n::pick(en, "Onayla ve Çalıştır", "Confirm & Execute"))
                        .color(Color32::BLACK)
                        .strong(),
                )
                .fill(if token_ok && !needs_unlock {
                    p.accent
                } else {
                    p.muted
                });
                if ui.add_enabled(token_ok && !needs_unlock, btn).clicked() {
                    outcome = ModalOutcome::Confirmed(manifest.action.clone());
                }
            });
        });
    outcome
}
