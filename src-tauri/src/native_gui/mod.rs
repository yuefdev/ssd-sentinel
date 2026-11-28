pub mod linux_desktop;
pub mod modal;
pub mod theme;
pub mod views;

use crate::{
    core::{Config, Core},
    i18n::pick,
    platform, report,
};
use eframe::egui;
use modal::{ConfirmedAction, ModalOutcome};
use serde_json::Value;
use std::{
    fs,
    io::{Read as _, Write as _},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use views::{Tab, UiAppState, UiCommand, WorkerEvent};

const ICON_128_PNG: &[u8] = include_bytes!("../../icons/128x128.png");

struct StoragePaths {
    data_dir: PathBuf,
    portable: bool,
}

fn resolve_linux_storage_paths(
    exe_dir: Option<&Path>,
    explicit_data_dir: Option<PathBuf>,
    cli_portable: bool,
) -> StoragePaths {
    let env_portable = std::env::var("SSD_SENTINEL_PORTABLE")
        .ok()
        .is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        });
    let marker =
        exe_dir.is_some_and(|d| d.join("portable.txt").is_file() || d.join("portable").is_file());
    let portable = cli_portable || env_portable || marker;

    if let Some(dir) = explicit_data_dir {
        return StoragePaths {
            data_dir: dir,
            portable,
        };
    }
    if portable {
        if let Some(dir) = exe_dir {
            return StoragePaths {
                data_dir: dir.join("data"),
                portable: true,
            };
        }
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    StoragePaths {
        data_dir: base.join("ssd-sentinel"),
        portable: false,
    }
}

fn socket_path() -> PathBuf {
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        PathBuf::from(rt).join("ssd-sentinel.sock")
    } else {
        let uid = unsafe { libc::geteuid() };
        PathBuf::from(format!("/tmp/ssd-sentinel-{uid}.sock"))
    }
}

fn try_single_instance(restore_requested: Arc<AtomicBool>) -> Option<PathBuf> {
    let sock = socket_path();
    if let Ok(mut stream) = UnixStream::connect(&sock) {
        let _ = stream.write_all(b"SHOW\n");
        return None;
    }
    let _ = fs::remove_file(&sock);
    if let Ok(listener) = UnixListener::bind(&sock) {
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                for mut stream in listener.incoming().flatten() {
                    let mut buf = [0u8; 32];
                    if stream.read(&mut buf).is_ok() {
                        restore_requested.store(true, Ordering::SeqCst);
                    }
                }
            })
            .ok();
        return Some(sock);
    }
    Some(sock)
}

fn load_app_icon() -> Option<Arc<egui::IconData>> {
    let img = image::load_from_memory(ICON_128_PNG).ok()?;
    let rgba = img.to_rgba8();
    Some(Arc::new(egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    }))
}

struct SentinelApp {
    core: Arc<Core>,
    ui_state: UiAppState,
    ev_rx: mpsc::Receiver<WorkerEvent>,
    ev_tx: mpsc::Sender<WorkerEvent>,
    perf_prev: Value,
    perf_last: Instant,
    restore_requested: Arc<AtomicBool>,
    quit_requested: Arc<AtomicBool>,
    allow_close: bool,
    sock_path: Option<PathBuf>,
    fonts_ready: bool,
    last_dark: Option<bool>,
}

impl SentinelApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        core: Arc<Core>,
        tray_available: bool,
        restore_requested: Arc<AtomicBool>,
        quit_requested: Arc<AtomicBool>,
        sock_path: Option<PathBuf>,
    ) -> Self {
        theme::install_system_fonts(&cc.egui_ctx);
        let dark = linux_desktop::detect_portal_dark_mode().unwrap_or(true);
        let palette = if dark {
            theme::Palette::dark()
        } else {
            theme::Palette::light()
        };
        theme::apply_visuals(&cc.egui_ctx, &palette);
        let (ev_tx, ev_rx) = mpsc::channel();
        let mut ui_state = UiAppState {
            dark_mode: dark,
            tray_available,
            ..UiAppState::default()
        };
        let state = core.state();
        if let Some(first_id) = state["disks"]
            .as_array()
            .and_then(|d| d.first())
            .and_then(|d| d["id"].as_str())
        {
            ui_state.selected_disk_id = Some(first_id.to_owned());
        }
        Self {
            core,
            ui_state,
            ev_rx,
            ev_tx,
            perf_prev: Value::Null,
            perf_last: Instant::now() - Duration::from_secs(2),
            restore_requested,
            quit_requested,
            allow_close: false,
            sock_path,
            fonts_ready: true,
            last_dark: Some(dark),
        }
    }

    fn dispatch(&mut self, cmd: UiCommand, ctx: egui::Context) {
        let core = self.core.clone();
        let tx = self.ev_tx.clone();
        let en = core.is_en();

        match cmd {
            UiCommand::Refresh => {
                core.refresh();
                ctx.request_repaint();
            }
            UiCommand::ElevatePolkit => {
                self.ui_state.busy_status =
                    Some(pick(en, "Polkit yetkilendirmesi...", "Polkit prompt...").into());
                std::thread::spawn(move || {
                    #[cfg(target_os = "linux")]
                    let res = crate::linux_priv::authenticate().map(|()| core.refresh());
                    #[cfg(not(target_os = "linux"))]
                    let res: Result<(), String> = Ok(());
                    let msg = match res {
                        Ok(()) => WorkerEvent::Toast {
                            is_error: false,
                            msg: pick(
                                en,
                                "Yetki yükseltildi; donanım yenileniyor.",
                                "Elevated via Polkit; refreshing telemetry.",
                            )
                            .into(),
                        },
                        Err(e) => WorkerEvent::Toast {
                            is_error: true,
                            msg: e,
                        },
                    };
                    let _ = tx.send(msg);
                    ctx.request_repaint();
                });
            }
            UiCommand::SetPreferences {
                minimise_to_tray,
                health_mode,
                language,
                read_only_guard,
            } => {
                if let Err(e) = core.set_preferences(
                    minimise_to_tray,
                    health_mode.as_deref(),
                    language.as_deref(),
                    read_only_guard,
                ) {
                    self.ui_state.toast = Some((true, e));
                }
                ctx.request_repaint();
            }
            UiCommand::Confirmed(action) => {
                std::thread::spawn(move || {
                    let res: Result<Value, String> = match action {
                        ConfirmedAction::StartJob(req) => core.start_job(req),
                        ConfirmedAction::DiskAction {
                            disk_id,
                            action,
                            confirmation,
                            letter,
                        } => core.disk_action(
                            &disk_id,
                            &action,
                            Some(&confirmation),
                            letter.as_deref(),
                        ),
                        ConfirmedAction::Optimize(req) => core.optimize_action(req),
                        ConfirmedAction::TierRelocate {
                            disk_id,
                            source_path,
                            target_letter,
                            direction,
                            confirmation,
                        } => core.relocate_tier_item(
                            &disk_id,
                            &source_path,
                            &target_letter,
                            &direction,
                            &confirmation,
                        ),
                        ConfirmedAction::TierRollback {
                            move_id,
                            confirmation,
                        } => core.rollback_tier_move(&move_id, &confirmation),
                        ConfirmedAction::DevClean {
                            disk_id,
                            path,
                            confirmation,
                        } => core.clean_dev_artifact(&disk_id, &path, &confirmation),
                    };
                    let ev = match res {
                        Ok(_) => WorkerEvent::Toast {
                            is_error: false,
                            msg: pick(
                                en,
                                "İşlem başlatıldı / tamamlandı.",
                                "Operation started / completed.",
                            )
                            .into(),
                        },
                        Err(e) => WorkerEvent::Toast {
                            is_error: true,
                            msg: e,
                        },
                    };
                    let _ = tx.send(ev);
                    ctx.request_repaint();
                });
            }
            UiCommand::CancelJob(id) => {
                if let Err(e) = core.cancel_job(&id) {
                    self.ui_state.toast = Some((true, e));
                }
            }
            UiCommand::QueryOpBounds { disk_id, letter } => {
                std::thread::spawn(move || {
                    let req = crate::optimize::OptimizeRequest {
                        action: "query_op_bounds".into(),
                        target: format!("{disk_id}:{letter}"),
                        value: String::new(),
                        confirmation: format!("optimize:query_op_bounds:{disk_id}:{letter}"),
                    };
                    match core.optimize_action(req) {
                        Ok(v) => {
                            let _ = tx.send(WorkerEvent::OpBounds(v));
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerEvent::Toast {
                                is_error: true,
                                msg: e,
                            });
                        }
                    }
                    ctx.request_repaint();
                });
            }
            UiCommand::AnalyzeTier { disk_id, letter } => {
                self.ui_state.busy_status =
                    Some(pick(en, "Katmanlama analiz ediliyor...", "Analyzing tiering...").into());
                std::thread::spawn(move || {
                    match core.analyze_storage_tiering(&disk_id, &letter) {
                        Ok(v) => {
                            let _ = tx.send(WorkerEvent::TierAnalysis(v));
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerEvent::Toast {
                                is_error: true,
                                msg: e,
                            });
                        }
                    }
                    ctx.request_repaint();
                });
            }
            UiCommand::AnalyzeDev {
                disk_id,
                letter,
                custom_root,
            } => {
                self.ui_state.busy_status =
                    Some(pick(en, "Projeler taranıyor...", "Scanning dev workspace...").into());
                std::thread::spawn(move || {
                    match core.analyze_dev_workspace(&disk_id, &letter, custom_root.as_deref()) {
                        Ok(v) => {
                            let _ = tx.send(WorkerEvent::DevAnalysis(v));
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerEvent::Toast {
                                is_error: true,
                                msg: e,
                            });
                        }
                    }
                    ctx.request_repaint();
                });
            }
            UiCommand::PickDevFolder { disk_id, letter } => {
                std::thread::spawn(move || {
                    if let Some(folder) = rfd::FileDialog::new()
                        .set_title(pick(en, "Geliştirici Klasörü Seç", "Select Dev Folder"))
                        .pick_folder()
                    {
                        let p_str = folder.to_string_lossy().into_owned();
                        match core.analyze_dev_workspace(&disk_id, &letter, Some(&p_str)) {
                            Ok(v) => {
                                let _ = tx.send(WorkerEvent::DevAnalysis(v));
                            }
                            Err(e) => {
                                let _ = tx.send(WorkerEvent::Toast {
                                    is_error: true,
                                    msg: e,
                                });
                            }
                        }
                    }
                    ctx.request_repaint();
                });
            }
            UiCommand::ExportReport { kind, redact } => {
                let state = core.state();
                std::thread::spawn(move || {
                    let ext = if kind == "json" { "json" } else { "html" };
                    let Some(dest) = rfd::FileDialog::new()
                        .set_file_name(format!("ssd-sentinel-report.{ext}"))
                        .add_filter(ext, &[ext])
                        .save_file()
                    else {
                        return;
                    };
                    let body = if kind == "json" {
                        let mut s = state;
                        if redact {
                            report::redact_value(&mut s);
                        }
                        serde_json::to_string_pretty(&s).unwrap_or_default()
                    } else {
                        report::render_html_report("snapshot", &state, None, redact)
                    };
                    let ev = match fs::write(&dest, body) {
                        Ok(()) => WorkerEvent::Toast {
                            is_error: false,
                            msg: format!(
                                "{}: {}",
                                pick(en, "Rapor kaydedildi", "Report saved"),
                                dest.display()
                            ),
                        },
                        Err(e) => WorkerEvent::Toast {
                            is_error: true,
                            msg: e.to_string(),
                        },
                    };
                    let _ = tx.send(ev);
                    ctx.request_repaint();
                });
            }
            UiCommand::CheckUpdates => {
                let ver = core.state()["version"]
                    .as_str()
                    .unwrap_or("1.11.0")
                    .to_owned();
                std::thread::spawn(move || {
                    match linux_desktop::check_github_updates(&ver, en) {
                        Ok(v) => {
                            let _ = tx.send(WorkerEvent::UpdateInfo(v));
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerEvent::Toast {
                                is_error: true,
                                msg: e,
                            });
                        }
                    }
                    ctx.request_repaint();
                });
            }
            UiCommand::PreviewRecovery { job_id, item_id } => {
                std::thread::spawn(move || {
                    match core.preview_recovered_item(&job_id, &item_id) {
                        Ok(v) => {
                            let _ = tx.send(WorkerEvent::RecoveryPreview(v));
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerEvent::Toast {
                                is_error: true,
                                msg: e,
                            });
                        }
                    }
                    ctx.request_repaint();
                });
            }
            UiCommand::RestoreRecoveryBatch { job_id, item_ids } => {
                std::thread::spawn(move || {
                    let Some(dest) = rfd::FileDialog::new()
                        .set_title(pick(
                            en,
                            "Kurtarma Hedef Klasörünü Seç (Farklı Disk)",
                            "Select Recovery Destination (Different Drive)",
                        ))
                        .pick_folder()
                    else {
                        return;
                    };
                    match core.restore_recovered_batch_to(&job_id, &item_ids, &dest) {
                        Ok(v) => {
                            let _ = tx.send(WorkerEvent::RecoveryRestoreResult(v));
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerEvent::Toast {
                                is_error: true,
                                msg: e,
                            });
                        }
                    }
                    ctx.request_repaint();
                });
            }
            UiCommand::ClearCheckpoint(disk_id) => {
                if let Err(e) = core.clear_surface_checkpoint(&disk_id) {
                    self.ui_state.toast = Some((true, e));
                } else {
                    self.ui_state.checkpoint = Value::Null;
                }
            }
            UiCommand::ReadLogs(disk_id) => {
                std::thread::spawn(move || {
                    match core.disk_action(&disk_id, "logs", None, None) {
                        Ok(v) => {
                            let _ = tx.send(WorkerEvent::DiskLogs(v));
                        }
                        Err(e) => {
                            let _ = tx.send(WorkerEvent::Toast {
                                is_error: true,
                                msg: e,
                            });
                        }
                    }
                    ctx.request_repaint();
                });
            }
            UiCommand::LoadDiskDetails(disk_id) => {
                self.ui_state.details_disk_id = Some(disk_id.clone());
                std::thread::spawn(move || {
                    let history = core.history(&disk_id).unwrap_or(Value::Null);
                    let forecast = core.health_forecast(&disk_id).unwrap_or(Value::Null);
                    let events = core.events(Some(&disk_id)).unwrap_or(Value::Null);
                    let bench_jobs = core
                        .saved_jobs("benchmark", Some(&disk_id))
                        .unwrap_or(Value::Null);
                    let surface_jobs = core
                        .saved_jobs("surface", Some(&disk_id))
                        .unwrap_or(Value::Null);
                    let checkpoint = core
                        .surface_checkpoint(&disk_id)
                        .unwrap_or(None)
                        .unwrap_or(Value::Null);
                    let _ = tx.send(WorkerEvent::DiskHistory {
                        disk_id,
                        history: Box::new(history),
                        forecast: Box::new(forecast),
                        events: Box::new(events),
                        bench_jobs: Box::new(bench_jobs),
                        surface_jobs: Box::new(surface_jobs),
                        checkpoint: Box::new(checkpoint),
                    });
                    ctx.request_repaint();
                });
            }
        }
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.ev_rx.try_recv() {
            self.ui_state.busy_status = None;
            match ev {
                WorkerEvent::Toast { is_error, msg } => {
                    self.ui_state.toast = Some((is_error, msg));
                }
                WorkerEvent::OpBounds(v) => self.ui_state.op_bounds = v,
                WorkerEvent::TierAnalysis(v) => self.ui_state.tier_analysis = v,
                WorkerEvent::DevAnalysis(v) => self.ui_state.dev_analysis = v,
                WorkerEvent::DiskHistory {
                    disk_id,
                    history,
                    forecast,
                    events,
                    bench_jobs,
                    surface_jobs,
                    checkpoint,
                } => {
                    if self.ui_state.selected_disk_id.as_deref() == Some(&disk_id) {
                        self.ui_state.history = *history;
                        self.ui_state.forecast = *forecast;
                        self.ui_state.events = *events;
                        self.ui_state.bench_jobs = *bench_jobs;
                        self.ui_state.surface_jobs = *surface_jobs;
                        self.ui_state.checkpoint = *checkpoint;
                    }
                }
                WorkerEvent::DiskLogs(v) => self.ui_state.smart_logs = v,
                WorkerEvent::UpdateInfo(v) => {
                    if v["has_update"] == true {
                        let latest = v["latest"].as_str().unwrap_or("");
                        linux_desktop::send_desktop_notification(
                            "SSD Sentinel",
                            &format!("Update available: v{latest}"),
                        );
                    }
                    self.ui_state.update_info = v;
                }
                WorkerEvent::RecoveryPreview(v) => {
                    self.ui_state.rec_preview_tex = None;
                    self.ui_state.rec_preview = v;
                }
                WorkerEvent::RecoveryRestoreResult(v) => {
                    let ok_n = v["restored"].as_u64().unwrap_or(0);
                    let err_n = v["failed"].as_u64().unwrap_or(0);
                    self.ui_state.toast = Some((
                        err_n > 0 && ok_n == 0,
                        format!("Restored: {ok_n} files · Failed: {err_n}"),
                    ));
                }
            }
        }
    }
}

impl eframe::App for SentinelApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.fonts_ready {
            theme::install_system_fonts(ctx);
            self.fonts_ready = true;
        }
        if self.quit_requested.load(Ordering::SeqCst) {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.restore_requested.swap(false, Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        self.drain_events();
        let state = self.core.state();
        let en = state["language"].as_str() == Some("en");

        // Vanilla GNOME 50 Tray Guard on CloseRequested
        if ctx.input(|i| i.viewport().close_requested()) {
            let wants_tray = state["minimise_to_tray"] == true;
            if self.ui_state.tray_available && wants_tray && !self.allow_close {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        if self.last_dark != Some(self.ui_state.dark_mode) {
            let p = if self.ui_state.dark_mode {
                theme::Palette::dark()
            } else {
                theme::Palette::light()
            };
            theme::apply_visuals(ctx, &p);
            self.last_dark = Some(self.ui_state.dark_mode);
        }
        let palette = if self.ui_state.dark_mode {
            theme::Palette::dark()
        } else {
            theme::Palette::light()
        };

        // Ensure first disk & details are loaded
        if self.ui_state.selected_disk_id.is_none() {
            if let Some(id) = state["disks"]
                .as_array()
                .and_then(|d| d.first())
                .and_then(|d| d["id"].as_str())
            {
                self.ui_state.selected_disk_id = Some(id.to_owned());
            }
        }

        let mut cmds = Vec::new();
        if let Some(ref cur_id) = self.ui_state.selected_disk_id {
            if self.ui_state.details_disk_id.as_deref() != Some(cur_id.as_str()) {
                cmds.push(UiCommand::LoadDiskDetails(cur_id.clone()));
            }
        }

        // Poll lightweight `/proc/diskstats` telemetry every 1s while on Overview tab
        if self.ui_state.tab == Tab::Overview
            && self.perf_last.elapsed() >= Duration::from_millis(950)
        {
            self.perf_last = Instant::now();
            if let Ok(perf) = platform::performance(&mut self.perf_prev, &self.core.stop) {
                let mut r_mb = 0.0;
                let mut w_mb = 0.0;
                let mut iops = 0.0;
                for d in perf["disks"].as_array().into_iter().flatten() {
                    r_mb += d["read_mb_s"].as_f64().unwrap_or(0.0);
                    w_mb += d["write_mb_s"].as_f64().unwrap_or(0.0);
                    iops += d["iops"].as_f64().unwrap_or(0.0);
                }
                if self.ui_state.perf_read_mb.len() >= 60 {
                    self.ui_state.perf_read_mb.pop_front();
                    self.ui_state.perf_write_mb.pop_front();
                    self.ui_state.perf_iops.pop_front();
                }
                self.ui_state.perf_read_mb.push_back(r_mb);
                self.ui_state.perf_write_mb.push_back(w_mb);
                self.ui_state.perf_iops.push_back(iops);
            }
        }

        // Top Control Strip
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("SSD SENTINEL")
                        .size(16.0)
                        .strong()
                        .color(palette.accent),
                );
                theme::badge(
                    ui,
                    &format!("v{}", state["version"].as_str().unwrap_or("1.11.0")),
                    palette.muted,
                );
                for tab in [
                    Tab::Overview,
                    Tab::Optimize,
                    Tab::Surface,
                    Tab::Recovery,
                    Tab::Smart,
                ] {
                    if ui
                        .selectable_label(self.ui_state.tab == tab, tab.label(en))
                        .clicked()
                    {
                        self.ui_state.tab = tab;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(if self.ui_state.dark_mode {
                            "Light"
                        } else {
                            "Dark"
                        })
                        .clicked()
                    {
                        self.ui_state.dark_mode = !self.ui_state.dark_mode;
                    }
                    if ui.small_button(if en { "TR" } else { "EN" }).clicked() {
                        let next = if en { "tr" } else { "en" }.to_owned();
                        cmds.push(UiCommand::SetPreferences {
                            minimise_to_tray: state["minimise_to_tray"] == true,
                            health_mode: None,
                            language: Some(next),
                            read_only_guard: None,
                        });
                    }
                    let ro = state["read_only_guard"] != false;
                    let ro_lbl = if ro {
                        pick(en, "Salt-Okunur: AÇIK", "Read-Only: ON")
                    } else {
                        pick(en, "Salt-Okunur: KAPALI", "Read-Only: OFF")
                    };
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(ro_lbl).color(if ro {
                                palette.ok
                            } else {
                                palette.warn
                            }),
                        ))
                        .clicked()
                    {
                        cmds.push(UiCommand::SetPreferences {
                            minimise_to_tray: state["minimise_to_tray"] == true,
                            health_mode: None,
                            language: None,
                            read_only_guard: Some(!ro),
                        });
                    }
                    let is_admin = state["is_admin"] == true;
                    if !is_admin {
                        if ui
                            .button(pick(en, "Yetki Yükselt (Polkit)", "Elevate (Polkit)"))
                            .clicked()
                        {
                            cmds.push(UiCommand::ElevatePolkit);
                        }
                    } else {
                        theme::badge(ui, "Elevated", palette.ok);
                    }
                    if ui.button(pick(en, "Yenile", "Refresh")).clicked() {
                        cmds.push(UiCommand::Refresh);
                    }
                    if ui.small_button(pick(en, "Güncelleme", "Updates")).clicked() {
                        cmds.push(UiCommand::CheckUpdates);
                    }
                });
            });
            ui.add_space(4.0);
        });

        // Left Disk Sidebar
        egui::SidePanel::left("disk_sidebar").resizable(true).default_width(250.0).show(ctx, |ui| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(pick(en, "Fiziksel Diskler", "Physical Drives")).strong().size(13.5));
            ui.add_space(4.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for d in state["disks"].as_array().into_iter().flatten() {
                    let id = d["id"].as_str().unwrap_or("");
                    let selected = self.ui_state.selected_disk_id.as_deref() == Some(id);
                    let model = d["model"].as_str().unwrap_or("Disk");
                    let pct = d["health"]["percent"].as_f64();
                    let status = d["health"]["status"].as_str().unwrap_or("unknown");
                    let dot_col = views::status_dot_color(status);
                    let temp = d["metrics"]["temperature_c"].as_f64();

                    let resp = theme::card_frame(&palette)
                        .fill(if selected { palette.card_alt } else { palette.card })
                        .stroke(egui::Stroke::new(
                            if selected { 1.5_f32 } else { 1.0_f32 },
                            if selected { palette.accent } else { palette.border },
                        ))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.colored_label(dot_col, "●");
                                ui.label(egui::RichText::new(model).strong().size(12.5));
                            });
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(d["device"].as_str().unwrap_or("—"))
                                        .size(11.0)
                                        .color(palette.muted),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} · {}",
                                            pct.map_or_else(|| "—".into(), |v| format!("{v:.0}%")),
                                            temp.map_or_else(|| "—".into(), |t| format!("{t:.0}°C"))
                                        ))
                                        .size(11.5)
                                        .color(palette.muted),
                                    );
                                });
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if resp.clicked() {
                        self.ui_state.selected_disk_id = Some(id.to_owned());
                        self.ui_state.smart_logs = Value::Null;
                        cmds.push(UiCommand::LoadDiskDetails(id.to_owned()));
                    }
                    ui.add_space(4.0);
                }

                ui.add_space(10.0);
                ui.separator();
                let mut min_tray = state["minimise_to_tray"] == true;
                if self.ui_state.tray_available {
                    if ui.checkbox(&mut min_tray, pick(en, "Kapatınca tepsiye küçült", "Minimize to tray on close")).changed() {
                        cmds.push(UiCommand::SetPreferences {
                            minimise_to_tray: min_tray,
                            health_mode: None,
                            language: None,
                            read_only_guard: None,
                        });
                    }
                } else {
                    ui.label(
                        egui::RichText::new(pick(
                            en,
                            "Sistem tepsisi uzantısı algılanmadı (GNOME saf Wayland koruması: Kapat çıkış yapar).",
                            "No StatusNotifier tray detected (GNOME Wayland guard: Close exits cleanly).",
                        ))
                        .size(11.0)
                        .color(palette.muted),
                    );
                }
            });
        });

        // Active Job Bar / Toast Bottom Strip
        let active_job = &state["job"];
        let job_running = views::active_job(&state);
        if job_running || self.ui_state.toast.is_some() || self.ui_state.busy_status.is_some() {
            egui::TopBottomPanel::bottom("bottom_status").show(ctx, |ui| {
                ui.add_space(4.0);
                if job_running {
                    let prog = active_job["progress"].as_u64().unwrap_or(0) as f32 / 100.0;
                    let phase = active_job["phase"].as_str().unwrap_or("");
                    let jid = active_job["id"].as_str().unwrap_or("").to_owned();
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {}",
                                active_job["model"].as_str().unwrap_or(""),
                                theme::fmt_volume(active_job["letter"].as_str().unwrap_or(""))
                            ))
                            .strong(),
                        );
                        ui.add(
                            egui::ProgressBar::new(prog)
                                .desired_width(240.0)
                                .text(format!("{}%", (prog * 100.0) as u32)),
                        );
                        ui.label(egui::RichText::new(phase).size(12.0).color(palette.muted));
                        if ui.button(pick(en, "Durdur", "Stop")).clicked() {
                            cmds.push(UiCommand::CancelJob(jid));
                        }
                    });
                }
                if let Some((is_err, msg)) = self.ui_state.toast.clone() {
                    ui.horizontal(|ui| {
                        ui.colored_label(if is_err { palette.crit } else { palette.ok }, msg);
                        if ui.small_button("×").clicked() {
                            self.ui_state.toast = None;
                        }
                    });
                } else if let Some(ref bs) = self.ui_state.busy_status {
                    ui.label(egui::RichText::new(bs).size(12.0).color(palette.accent));
                }
                ui.add_space(4.0);
            });
        }

        // Main Workspace Content
        egui::CentralPanel::default().show(ctx, |ui| {
            let disk_owned = views::selected_disk(&state, &self.ui_state).cloned();
            let disk_ref = disk_owned.as_ref();

            // Volume selector strip
            if let Some(d) = disk_ref {
                let vols = d["volumes"].as_array().cloned().unwrap_or_default();
                if !vols.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(pick(en, "Hedef Bölüm:", "Active Mount:"))
                                .size(12.0)
                                .color(palette.muted),
                        );
                        let cur = views::selected_volume(d, &mut self.ui_state);
                        for v in &vols {
                            let l = v["letter"].as_str().unwrap_or("");
                            let selected = cur.as_deref() == Some(l);
                            if ui
                                .selectable_label(selected, theme::fmt_volume(l))
                                .clicked()
                            {
                                if let Some(id) = d["id"].as_str() {
                                    self.ui_state
                                        .selected_volume_by_disk
                                        .insert(id.to_owned(), l.to_owned());
                                }
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
            }

            egui::ScrollArea::vertical().show(ui, |ui| match self.ui_state.tab {
                Tab::Overview => views::overview::show(
                    ui,
                    &state,
                    disk_ref,
                    &mut self.ui_state,
                    &palette,
                    en,
                    &mut cmds,
                ),
                Tab::Optimize => views::optimize::show(
                    ui,
                    &state,
                    disk_ref,
                    &mut self.ui_state,
                    &palette,
                    en,
                    &mut cmds,
                ),
                Tab::Surface => views::surface::show(
                    ui,
                    &state,
                    disk_ref,
                    &mut self.ui_state,
                    &palette,
                    en,
                    &mut cmds,
                ),
                Tab::Recovery => views::recovery::show(
                    ui,
                    &state,
                    disk_ref,
                    &mut self.ui_state,
                    &palette,
                    en,
                    &mut cmds,
                ),
                Tab::Smart => views::smart::show(
                    ui,
                    &state,
                    disk_ref,
                    &mut self.ui_state,
                    &palette,
                    en,
                    &mut cmds,
                ),
            });
        });

        // Confirmation Manifest Modal
        if let Some(mut manifest) = self.ui_state.modal.take() {
            let ro = state["read_only_guard"] != false;
            match modal::show_manifest_modal(ctx, &mut manifest, ro, en, &palette) {
                ModalOutcome::None => self.ui_state.modal = Some(manifest),
                ModalOutcome::Cancelled => {}
                ModalOutcome::Confirmed(act) => cmds.push(UiCommand::Confirmed(act)),
            }
        }

        for cmd in cmds {
            self.dispatch(cmd, ctx.clone());
        }

        // Reactive low-CPU repaint cadence
        let delay = if job_running || self.ui_state.tab == Tab::Overview {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(2)
        };
        ctx.request_repaint_after(delay);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.core.shutdown();
        #[cfg(target_os = "linux")]
        crate::linux_priv::shutdown_helper();
        if let Some(ref p) = self.sock_path {
            let _ = fs::remove_file(p);
        }
    }
}

pub fn run() -> i32 {
    let mut snapshot_dir: Option<PathBuf> = None;
    let mut explicit_data_dir: Option<PathBuf> = None;
    let mut smartctl_override: Option<PathBuf> = None;
    let mut cli_portable = false;
    let mut start_minimized = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--snapshot-dir" => {
                let Some(p) = args.next() else {
                    eprintln!("--snapshot-dir requires a directory path");
                    return 2;
                };
                snapshot_dir = Some(PathBuf::from(p));
            }
            "--data-dir" => {
                let Some(p) = args.next() else {
                    eprintln!("--data-dir requires a directory path");
                    return 2;
                };
                explicit_data_dir = Some(PathBuf::from(p));
            }
            "--smartctl" => {
                let Some(p) = args.next() else {
                    eprintln!("--smartctl requires a binary path");
                    return 2;
                };
                smartctl_override = Some(PathBuf::from(p));
            }
            "--portable" => cli_portable = true,
            "--minimized" => start_minimized = true,
            "--elevated-relaunch" => {}
            other => {
                eprintln!("Unknown option: {other}");
                return 2;
            }
        }
    }

    let restore_requested = Arc::new(AtomicBool::new(false));
    let quit_requested = Arc::new(AtomicBool::new(false));
    let sock_path = if snapshot_dir.is_none() {
        let Some(p) = try_single_instance(restore_requested.clone()) else {
            return 0;
        };
        Some(p)
    } else {
        None
    };

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    let storage = resolve_linux_storage_paths(exe_dir.as_deref(), explicit_data_dir, cli_portable);
    if let Err(e) = fs::create_dir_all(&storage.data_dir) {
        eprintln!(
            "Could not create data dir {}: {e}",
            storage.data_dir.display()
        );
        return 1;
    }

    let smartctl = smartctl_override.unwrap_or_else(platform::discover_smartctl);
    let core = match Core::new(Config {
        smartctl,
        data_dir: storage.data_dir,
        snapshot_dir,
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to initialize SSD Sentinel core: {e}");
            return 1;
        }
    };
    core.set_portable(storage.portable);
    core.start_background();

    let tray_available = linux_desktop::start_status_notifier_item(
        restore_requested.clone(),
        quit_requested.clone(),
    );

    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id("com.ssdsentinel.desktop")
        .with_title("SSD Sentinel")
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([960.0, 640.0]);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }
    if start_minimized && tray_available {
        viewport = viewport.with_visible(false);
    }

    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let res = eframe::run_native(
        "SSD Sentinel",
        opts,
        Box::new(move |cc| {
            Ok(Box::new(SentinelApp::new(
                cc,
                core,
                tray_available,
                restore_requested,
                quit_requested,
                sock_path,
            )))
        }),
    );
    match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("GUI initialization failed: {e}");
            1
        }
    }
}
