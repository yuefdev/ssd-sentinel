pub mod optimize;
pub mod overview;
pub mod recovery;
pub mod smart;
pub mod surface;

use crate::native_gui::modal::PendingManifest;
use egui::{Color32, RichText, TextureHandle};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Optimize,
    Surface,
    Recovery,
    Smart,
}

impl Tab {
    pub fn label(self, en: bool) -> &'static str {
        match (self, en) {
            (Tab::Overview, false) => "Genel Bakış",
            (Tab::Overview, true) => "Overview",
            (Tab::Optimize, false) => "Optimize Et",
            (Tab::Optimize, true) => "Optimize",
            (Tab::Surface, false) => "Yüzey & Hız",
            (Tab::Surface, true) => "Surface & Bench",
            (Tab::Recovery, false) => "Veri Kurtarma",
            (Tab::Recovery, true) => "Data Recovery",
            (Tab::Smart, false) => "SMART & Günlük",
            (Tab::Smart, true) => "SMART & Logs",
        }
    }
}

pub enum UiCommand {
    Refresh,
    ElevatePolkit,
    SetPreferences {
        minimise_to_tray: bool,
        health_mode: Option<String>,
        language: Option<String>,
        read_only_guard: Option<bool>,
    },
    Confirmed(crate::native_gui::modal::ConfirmedAction),
    CancelJob(String),
    QueryOpBounds {
        disk_id: String,
        letter: String,
    },
    AnalyzeTier {
        disk_id: String,
        letter: String,
    },
    AnalyzeDev {
        disk_id: String,
        letter: String,
        custom_root: Option<String>,
    },
    PickDevFolder {
        disk_id: String,
        letter: String,
    },
    ExportReport {
        kind: String,
        redact: bool,
    },
    CheckUpdates,
    PreviewRecovery {
        job_id: String,
        item_id: String,
    },
    RestoreRecoveryBatch {
        job_id: String,
        item_ids: Vec<String>,
    },
    ClearCheckpoint(String),
    ReadLogs(String),
    LoadDiskDetails(String),
}

pub enum WorkerEvent {
    Toast {
        is_error: bool,
        msg: String,
    },
    OpBounds(Value),
    TierAnalysis(Value),
    DevAnalysis(Value),
    DiskHistory {
        disk_id: String,
        history: Box<Value>,
        forecast: Box<Value>,
        events: Box<Value>,
        bench_jobs: Box<Value>,
        surface_jobs: Box<Value>,
        checkpoint: Box<Value>,
    },
    DiskLogs(Value),
    UpdateInfo(Value),
    RecoveryPreview(Value),
    RecoveryRestoreResult(Value),
}

pub struct UiAppState {
    pub tab: Tab,
    pub dark_mode: bool,
    pub selected_disk_id: Option<String>,
    pub selected_volume_by_disk: HashMap<String, String>,
    pub tray_available: bool,
    pub busy_status: Option<String>,
    pub toast: Option<(bool, String)>,
    pub modal: Option<PendingManifest>,

    // Live perf chart ring buffers
    pub perf_read_mb: VecDeque<f64>,
    pub perf_write_mb: VecDeque<f64>,
    pub perf_iops: VecDeque<f64>,
    pub perf_sample_tick: u64,

    // Cached per-disk data
    pub details_disk_id: Option<String>,
    pub history: Value,
    pub forecast: Value,
    pub events: Value,
    pub bench_jobs: Value,
    pub surface_jobs: Value,
    pub checkpoint: Value,
    pub smart_logs: Value,
    pub update_info: Value,

    // Optimize / Tiering / Dev cache
    pub op_bounds: Value,
    pub tier_analysis: Value,
    pub dev_analysis: Value,
    pub custom_dev_root: String,

    // Surface / Bench options
    pub bench_mode: String,
    pub bench_size_gib: u64,
    pub surface_mode: String,

    // Recovery state
    pub rec_mode: String,
    pub rec_search: String,
    pub rec_subtab_apps: bool,
    pub rec_selected_ids: Vec<String>,
    pub rec_preview: Value,
    pub rec_preview_tex: Option<(String, TextureHandle)>,
}

impl Default for UiAppState {
    fn default() -> Self {
        Self {
            tab: Tab::Overview,
            dark_mode: true,
            selected_disk_id: None,
            selected_volume_by_disk: HashMap::new(),
            tray_available: false,
            busy_status: None,
            toast: None,
            modal: None,
            perf_read_mb: VecDeque::with_capacity(60),
            perf_write_mb: VecDeque::with_capacity(60),
            perf_iops: VecDeque::with_capacity(60),
            perf_sample_tick: 0,
            details_disk_id: None,
            history: Value::Null,
            forecast: Value::Null,
            events: Value::Null,
            bench_jobs: Value::Null,
            surface_jobs: Value::Null,
            checkpoint: Value::Null,
            smart_logs: Value::Null,
            update_info: Value::Null,
            op_bounds: Value::Null,
            tier_analysis: Value::Null,
            dev_analysis: Value::Null,
            custom_dev_root: String::new(),
            bench_mode: "standard".into(),
            bench_size_gib: 1,
            surface_mode: "read_scan".into(),
            rec_mode: "quick".into(),
            rec_search: String::new(),
            rec_subtab_apps: false,
            rec_selected_ids: Vec::new(),
            rec_preview: Value::Null,
            rec_preview_tex: None,
        }
    }
}

pub fn selected_disk<'a>(state: &'a Value, ui_state: &UiAppState) -> Option<&'a Value> {
    let disks = state["disks"].as_array()?;
    if let Some(ref id) = ui_state.selected_disk_id {
        if let Some(found) = disks.iter().find(|d| d["id"].as_str() == Some(id.as_str())) {
            return Some(found);
        }
    }
    disks.first()
}

pub fn selected_volume(disk: &Value, ui_state: &mut UiAppState) -> Option<String> {
    let disk_id = disk["id"].as_str()?.to_owned();
    let vols = disk["volumes"].as_array()?;
    if let Some(cur) = ui_state.selected_volume_by_disk.get(&disk_id) {
        if vols
            .iter()
            .any(|v| v["letter"].as_str() == Some(cur.as_str()))
        {
            return Some(cur.clone());
        }
    }
    let first = vols.first()?.get("letter")?.as_str()?.to_owned();
    ui_state
        .selected_volume_by_disk
        .insert(disk_id, first.clone());
    Some(first)
}

pub fn status_dot_color(status: &str) -> Color32 {
    match status {
        "critical" | "failing" => Color32::from_rgb(239, 68, 68),
        "warning" => Color32::from_rgb(245, 158, 11),
        "unknown" => Color32::from_rgb(148, 163, 184),
        _ => Color32::from_rgb(34, 197, 94),
    }
}

pub fn active_job(state: &Value) -> bool {
    matches!(
        state["job"]["status"].as_str(),
        Some("running" | "cancelling")
    )
}

pub fn small_kv(ui: &mut egui::Ui, k: &str, v: &str, muted: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(k).size(12.0).color(muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(v).size(12.0).strong());
        });
    });
}
