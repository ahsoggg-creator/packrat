//! Dialogs about trust: the name encoding of old archives, passwords, and bomb warnings.

use super::{AfterOpen, App, EncodingDialog, PasswordDialog, theme};
use crate::i18n::{Key, t, tf};
use eframe::egui::{self, Align2, RichText};
use packrat_core::OpenOptions;
use packrat_core::encoding::NameEncoding;
use packrat_core::safety::Risk;

fn window(title: &str) -> egui::Window<'_> {
    egui::Window::new(RichText::new(title).strong())
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(theme::card())
}

/// A risk in the user's language.
pub fn risk_text(r: &Risk) -> String {
    let ratio = |x: f64| format!("{x:.0}");
    match r {
        Risk::HighRatio { path, ratio: x, size } => {
            tf(Key::RiskHighRatio, &[path, &ratio(*x), &theme::human_size(*size)])
        }
        Risk::HighTotalRatio { ratio: x, size } => {
            tf(Key::RiskHighTotalRatio, &[&ratio(*x), &theme::human_size(*size)])
        }
        Risk::TotalTooLarge { bytes, limit } => tf(
            Key::RiskTotalTooLarge,
            &[&theme::human_size(*bytes), &theme::human_size(*limit)],
        ),
        Risk::NotEnoughSpace { need, free } => tf(
            Key::RiskNotEnoughSpace,
            &[&theme::human_size(*need), &theme::human_size(*free)],
        ),
        Risk::TooManyEntries { count, limit } => tf(Key::RiskTooManyEntries, &[&count.to_string(), &limit.to_string()]),
        Risk::OverlappingEntries { count } => tf(Key::RiskOverlap, &[&count.to_string()]),
        Risk::DeepNesting { depth, limit } => tf(Key::RiskNesting, &[&depth.to_string(), &limit.to_string()]),
        Risk::NestedExpansion { bytes } => tf(Key::RiskNestedExpansion, &[&theme::human_size(*bytes)]),
    }
}

impl App {
    /// The archive on screen needs a (different) password.
    pub(super) fn ask_password_for_open_archive(&mut self) {
        let Some(l) = &self.loaded else { return };
        self.password_dialog = Some(PasswordDialog {
            text: String::new(),
            show: false,
            wrong: l.opts.password.is_some(),
            path: l.path.clone(),
            then: AfterOpen::Nothing,
        });
    }

    pub(super) fn open_encoding_dialog(&mut self) {
        let Some(legacy) = self
            .loaded
            .as_ref()
            .and_then(|l| l.archive.legacy_names().map(|n| n.current))
        else {
            return;
        };
        self.encoding_dialog = Some(EncodingDialog { choice: legacy });
    }

    pub(super) fn pump_preview(&mut self, ctx: &egui::Context) {
        let want = self.loaded.as_ref().and_then(|l| {
            let mut it = l.selected.iter();
            let (Some(&id), None) = (it.next(), it.next()) else {
                return None;
            };
            let n = &l.tree.nodes[id];
            n.entry.filter(|_| !n.is_dir)
        });
        self.preview.set_want(want);
        let archive = self.loaded.as_ref().map(|l| l.archive.clone());
        self.preview.pump(ctx, archive.as_ref());
        if self.preview.is_loading() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    pub(super) fn risk_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.risk_dialog else { return };
        let (mut go, mut cancel) = (false, false);
        window(t(Key::RiskTitle)).show(ctx, |ui| {
            ui.set_min_width(520.0);
            ui.label(RichText::new(t(Key::RiskText)).color(theme::PALETTE.warn));
            ui.add_space(4.0);
            for r in &d.plan.risks {
                ui.add(egui::Label::new(format!("•  {}", risk_text(r))).wrap());
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if theme::primary(ui, t(Key::Cancel)).clicked() {
                    cancel = true;
                }
                if ui.button(t(Key::RiskContinue)).clicked() {
                    go = true;
                }
            });
        });
        if go && let Some(d) = self.risk_dialog.take() {
            let mut opts = d.opts;
            opts.accept_risks = true;
            self.after_risks(ctx, d.plan, opts);
        } else if cancel {
            self.risk_dialog = None;
        }
    }

    pub(super) fn password_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(d) = self.password_dialog.as_mut() else { return };
        let (mut ok, mut cancel) = (false, false);
        window(t(Key::PasswordTitle)).show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.label(t(Key::PasswordText));
            if d.wrong {
                ui.label(RichText::new(t(Key::PasswordWrong)).color(theme::PALETTE.warn));
            }
            ui.horizontal(|ui| {
                ui.label(t(Key::PasswordLabel));
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut d.text)
                        .password(!d.show)
                        .desired_width(240.0),
                );
                if !edit.has_focus() && d.text.is_empty() {
                    edit.request_focus();
                }
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ok = true;
                }
                ui.checkbox(&mut d.show, t(Key::PasswordShow));
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!d.text.is_empty(), |ui| {
                    if theme::primary(ui, t(Key::Ok)).clicked() {
                        ok = true;
                    }
                });
                if ui.button(t(Key::Cancel)).clicked() {
                    cancel = true;
                }
            });
        });
        if ok && !d.text.is_empty() {
            let Some(d) = self.password_dialog.take() else { return };
            let mut opts = OpenOptions::default().with_password(&d.text);
            if let Some(l) = &self.loaded {
                opts.encoding = l.opts.encoding;
            }
            self.after_open = d.then;
            let on_screen = self.loaded.as_ref().and_then(|l| l.path.clone());
            match d.path {
                Some(p) if on_screen.as_ref() == Some(&p) => self.reopen_with(ctx, opts),
                Some(p) => self.open_path_with(ctx, p, opts),
                None => {}
            }
        } else if cancel {
            self.password_dialog = None;
            self.after_open = AfterOpen::Nothing;
        }
    }

    pub(super) fn encoding_dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(d) = self.encoding_dialog.as_mut() else { return };
        let Some(legacy) = self.loaded.as_ref().and_then(|l| l.archive.legacy_names()) else {
            self.encoding_dialog = None;
            return;
        };
        let (mut apply, mut cancel) = (false, false);
        window(t(Key::EncodingTitle)).show(ctx, |ui| {
            ui.set_min_width(560.0);
            ui.label(tf(Key::EncodingText, &[legacy.detection.best.label()]));
            ui.horizontal(|ui| {
                ui.label(t(Key::EncodingChoose));
                egui::ComboBox::from_id_salt("enc")
                    .selected_text(d.choice.label())
                    .show_ui(ui, |ui| {
                        for (enc, _) in &legacy.detection.ranked {
                            let mut label = enc.label().to_owned();
                            if *enc == legacy.detection.best {
                                label.push_str(&format!(" — {}", t(Key::EncodingDetected)));
                            }
                            ui.selectable_value(&mut d.choice, *enc, label);
                        }
                        ui.selectable_value(&mut d.choice, NameEncoding::Utf8, NameEncoding::Utf8.label());
                    });
            });
            ui.add_space(4.0);
            ui.label(theme::dim(t(Key::EncodingPreview)));
            egui::Frame::new()
                .fill(theme::PALETTE.panel)
                .corner_radius(8)
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    for name in legacy.preview(d.choice, 8) {
                        ui.add(egui::Label::new(RichText::new(name).monospace()).truncate());
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::primary(ui, t(Key::EncodingApply)).clicked() {
                    apply = true;
                }
                if ui.button(t(Key::Cancel)).clicked() {
                    cancel = true;
                }
            });
        });
        if apply {
            let choice = d.choice;
            self.encoding_dialog = None;
            let mut opts = OpenOptions::default();
            if let Some(l) = &self.loaded {
                opts.password = l.opts.password.clone();
            }
            opts.encoding = Some(choice);
            self.reopen_with(ctx, opts);
        } else if cancel {
            self.encoding_dialog = None;
        }
    }
}
