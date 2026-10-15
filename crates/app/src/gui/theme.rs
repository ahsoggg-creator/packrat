//! Look and feel: one dark palette, rounded cards, system fonts.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Margin, RichText, Stroke, Ui, vec2,
};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub warn: Color32,
}

pub const PALETTE: Palette = Palette {
    bg: Color32::from_rgb(19, 22, 27),
    panel: Color32::from_rgb(25, 29, 36),
    card: Color32::from_rgb(32, 37, 46),
    card_hover: Color32::from_rgb(40, 46, 57),
    border: Color32::from_rgb(52, 59, 72),
    text: Color32::from_rgb(230, 234, 240),
    dim: Color32::from_rgb(143, 152, 168),
    accent: Color32::from_rgb(62, 201, 167),
    warn: Color32::from_rgb(235, 178, 70),
};

pub fn install(ctx: &egui::Context) {
    let p = PALETTE;
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |st| {
        st.spacing.item_spacing = vec2(8.0, 7.0);
        st.spacing.button_padding = vec2(12.0, 6.0);
        st.spacing.interact_size.y = 28.0;
        st.spacing.scroll.bar_width = 8.0;
        let v = &mut st.visuals;
        v.panel_fill = p.bg;
        v.window_fill = p.card;
        v.extreme_bg_color = p.panel;
        v.faint_bg_color = p.panel;
        v.window_corner_radius = CornerRadius::same(14);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_stroke = Stroke::new(1.0, p.border);
        v.override_text_color = Some(p.text);
        v.selection.bg_fill = p.accent.gamma_multiply(0.28);
        v.selection.stroke = Stroke::new(1.0, p.accent);
        v.hyperlink_color = p.accent;
        let r = CornerRadius::same(6);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = r;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
        v.widgets.inactive.bg_fill = Color32::from_rgb(52, 59, 74);
        v.widgets.inactive.weak_bg_fill = p.card;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
        v.widgets.hovered.bg_fill = p.card_hover;
        v.widgets.hovered.weak_bg_fill = p.card_hover;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.7));
        v.widgets.active.bg_fill = p.accent.gamma_multiply(0.35);
        v.widgets.active.weak_bg_fill = p.accent.gamma_multiply(0.35);
    });
}

fn windows_font(file: &str) -> Option<Vec<u8>> {
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    std::fs::read(format!(r"{windir}\Fonts\{file}")).ok()
}

/// Segoe UI in front of egui's built-in fonts (good Cyrillic), Segoe UI Symbol as a glyph fallback.
pub fn install_fonts(ctx: &egui::Context, with_cjk: bool) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: Vec<u8>, front: bool| {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_owned(bytes)));
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            let list = fonts.families.entry(fam.clone()).or_default();
            if front && fam == FontFamily::Proportional {
                list.insert(0, name.into());
            } else {
                list.push(name.into());
            }
        }
    };
    for (name, file, front) in [("segoe", "segoeui.ttf", true), ("segoe-symbols", "seguisym.ttf", false)] {
        if let Some(b) = windows_font(file) {
            add(name, b, front);
        }
    }
    if with_cjk {
        // Japanese, Simplified Chinese, Korean: only loaded when a name needs them (the files are large)
        for (name, file) in [
            ("cjk-jp", "YuGothM.ttc"),
            ("cjk-sc", "msyh.ttc"),
            ("cjk-kr", "malgun.ttf"),
        ] {
            if let Some(b) = windows_font(file) {
                add(name, b, false);
            }
        }
    }
    ctx.set_fonts(fonts);
}

pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(PALETTE.card)
        .stroke(Stroke::new(1.0, PALETTE.border))
        .corner_radius(14)
        .inner_margin(Margin::same(16))
}

pub fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(17.0).strong());
}

pub fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(PALETTE.dim)
}

/// Primary (accent-filled) button.
pub fn primary(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).strong().color(Color32::from_rgb(10, 30, 26))).fill(PALETTE.accent))
}

pub fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    const UE: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let units = if crate::i18n::lang() == crate::i18n::Lang::Ru {
        U
    } else {
        UE
    };
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} {}", units[0])
    } else {
        format!("{v:.1} {}", units[i])
    }
}
