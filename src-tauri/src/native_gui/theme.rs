use egui::{
    style::Widgets, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Margin, Stroke,
};
use std::{path::Path, sync::Arc};

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub card_alt: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub crit: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: Color32::from_rgb(12, 15, 20),
            sidebar: Color32::from_rgb(16, 21, 29),
            card: Color32::from_rgb(21, 27, 36),
            card_alt: Color32::from_rgb(27, 35, 47),
            border: Color32::from_rgb(38, 49, 66),
            text: Color32::from_rgb(232, 238, 246),
            muted: Color32::from_rgb(142, 156, 178),
            accent: Color32::from_rgb(34, 211, 238),
            ok: Color32::from_rgb(34, 197, 94),
            warn: Color32::from_rgb(245, 158, 11),
            crit: Color32::from_rgb(239, 68, 68),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            bg: Color32::from_rgb(244, 246, 250),
            sidebar: Color32::from_rgb(235, 239, 246),
            card: Color32::WHITE,
            card_alt: Color32::from_rgb(241, 245, 249),
            border: Color32::from_rgb(209, 217, 228),
            text: Color32::from_rgb(15, 23, 42),
            muted: Color32::from_rgb(82, 96, 117),
            accent: Color32::from_rgb(2, 132, 199),
            ok: Color32::from_rgb(22, 163, 74),
            warn: Color32::from_rgb(217, 119, 6),
            crit: Color32::from_rgb(220, 38, 38),
        }
    }
}

pub fn install_system_fonts(ctx: &egui::Context) {
    const FONT_PATHS: &[&str] = &[
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
        "/usr/share/fonts/cantarell/Cantarell-Regular.otf",
        "/usr/share/fonts/inter/Inter-Regular.ttf",
    ];
    let mut defs = FontDefinitions::default();
    for path in FONT_PATHS {
        if Path::new(path).is_file() {
            if let Ok(bytes) = std::fs::read(path) {
                defs.font_data
                    .insert("sys_ui".to_owned(), Arc::new(FontData::from_owned(bytes)));
                if let Some(fam) = defs.families.get_mut(&FontFamily::Proportional) {
                    fam.insert(0, "sys_ui".to_owned());
                }
                break;
            }
        }
    }
    const MONO_PATHS: &[&str] = &[
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
    ];
    for path in MONO_PATHS {
        if Path::new(path).is_file() {
            if let Ok(bytes) = std::fs::read(path) {
                defs.font_data
                    .insert("sys_mono".to_owned(), Arc::new(FontData::from_owned(bytes)));
                if let Some(fam) = defs.families.get_mut(&FontFamily::Monospace) {
                    fam.insert(0, "sys_mono".to_owned());
                }
                break;
            }
        }
    }
    ctx.set_fonts(defs);
}

pub fn apply_visuals(ctx: &egui::Context, p: &Palette) {
    let mut visuals = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = p.bg;
    visuals.window_fill = p.card;
    visuals.extreme_bg_color = p.card_alt;
    visuals.faint_bg_color = p.card_alt;
    visuals.override_text_color = Some(p.text);
    visuals.selection.bg_fill = p.accent.linear_multiply(0.28);
    visuals.selection.stroke = Stroke::new(1.0_f32, p.accent);
    let tweak = |w: &mut Widgets| {
        w.noninteractive.bg_fill = p.card;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
        w.noninteractive.corner_radius = CornerRadius::same(8);
        w.inactive.bg_fill = p.card_alt;
        w.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
        w.inactive.corner_radius = CornerRadius::same(7);
        w.hovered.bg_fill = p.card_alt;
        w.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent);
        w.hovered.corner_radius = CornerRadius::same(7);
        w.active.bg_fill = p.accent.linear_multiply(0.2);
        w.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
        w.active.corner_radius = CornerRadius::same(7);
    };
    tweak(&mut visuals.widgets);
    ctx.set_visuals(visuals);
}

pub fn card_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
}

pub fn badge(ui: &mut egui::Ui, label: &str, color: Color32) {
    let bg = color.linear_multiply(0.16);
    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, color.linear_multiply(0.55)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).size(11.5).strong().color(color));
        });
}

pub fn health_color(status: &str, percent: Option<f64>, p: &Palette) -> Color32 {
    match status {
        "critical" | "failing" => p.crit,
        "warning" => p.warn,
        "unknown" => p.muted,
        _ => match percent {
            Some(v) if v < 50.0 => p.crit,
            Some(v) if v < 80.0 => p.warn,
            Some(_) => p.ok,
            None => p.muted,
        },
    }
}

pub fn fmt_bytes(bytes: Option<u64>) -> String {
    let Some(b) = bytes else {
        return "—".into();
    };
    let units = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut val = b as f64;
    let mut idx = 0usize;
    while val >= 1000.0 && idx + 1 < units.len() {
        val /= 1000.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{b} B")
    } else {
        format!("{val:.1} {}", units[idx])
    }
}

pub fn fmt_volume(letter: &str) -> String {
    let t = letter.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.starts_with('/') {
        t.to_owned()
    } else {
        format!("{}:", t.trim_end_matches(':'))
    }
}
