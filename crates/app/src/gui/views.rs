//! Top bar, status bar, welcome screen and the archive table.

use super::tree::{Sort, SortKey};
use super::{App, UpdateState, theme};
use crate::i18n::{Key, t, tf};
use eframe::egui::{self, Align, Align2, Color32, Layout, Margin, RichText, Sense, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use packrat_core::timeutil;
use std::collections::HashSet;
use std::path::PathBuf;

enum RowAction {
    Select { node: usize, ctrl: bool, shift: bool },
    Toggle(usize),
    Sort(SortKey),
    ExtractSelected,
    DragOut(usize),
    ExpandAll,
    CollapseAll,
}

impl App {
    /// Extracts the selection to a temporary folder and starts an OS drag with the resulting files.
    fn drag_out(&mut self, node: usize) {
        const LIMIT: u64 = 256 << 20;
        if self.demo {
            self.show_toast(t(Key::DemoNoWrite).to_owned());
            return;
        }
        let Some(l) = self.loaded.as_mut() else { return };
        if !l.selected.contains(&node) {
            l.selected = HashSet::from([node]);
        }
        // only the top-most selected items: their descendants travel inside them
        let tops: Vec<usize> = l
            .selected
            .iter()
            .copied()
            .filter(|&id| {
                let mut p = l.tree.nodes[id].parent;
                while let Some(x) = p {
                    if l.selected.contains(&x) {
                        return false;
                    }
                    p = l.tree.nodes[x].parent;
                }
                true
            })
            .collect();
        let size: u64 = tops.iter().map(|&i| l.tree.nodes[i].size).sum();
        if size > LIMIT {
            self.show_toast(tf(Key::DragTooLarge, &[&theme::human_size(LIMIT)]));
            return;
        }
        let entries = l.tree.entries_under(&tops.iter().copied().collect());
        let dir = crate::dragout::drag_dir();
        let mut opts = packrat_core::extract::ExtractOptions::new(&dir, l.name.clone());
        opts.smart = false;
        opts.selection = Some(entries);
        opts.threads = self.settings.threads;
        let progress = packrat_core::Progress::new();
        let archive = l.archive.clone();
        let report = packrat_core::extract::extract(&*archive, &opts, &progress);
        let paths: Vec<PathBuf> = tops
            .iter()
            .map(|&id| {
                let mut parts = Vec::new();
                let mut cur = Some(id);
                while let Some(x) = cur {
                    parts.push(l.tree.nodes[x].name.clone());
                    cur = l.tree.nodes[x].parent;
                }
                parts.reverse();
                let mut p = dir.clone();
                p.extend(parts);
                p
            })
            .filter(|p| p.exists())
            .collect();
        match (report, paths.is_empty()) {
            (Ok(_), false) => {
                crate::diag::info(&format!("drag out: {} items", paths.len()));
                if let Err(e) = crate::dragout::start(&paths) {
                    crate::diag::warn(&format!("drag out failed: {e}"));
                }
            }
            (Err(e), _) => self.show_toast(tf(Key::ErrorGeneric, &[&e.to_string()])),
            _ => {}
        }
    }

    pub(super) fn pick_archive(&mut self, ctx: &egui::Context) {
        let picked = rfd::FileDialog::new()
            .add_filter(
                "Archives",
                &[
                    "zip", "7z", "tar", "gz", "tgz", "bz2", "xz", "txz", "zst", "tzst", "lz4",
                ],
            )
            .add_filter("All files", &["*"])
            .pick_file();
        if let Some(p) = picked {
            self.open_path(ctx, p);
        }
    }

    pub(super) fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() && self.job.is_none() && self.demo_allows_drop() {
            self.open_or_create(ctx, dropped);
        }
    }

    fn demo_allows_drop(&self) -> bool {
        !self.demo
    }

    pub(super) fn drop_overlay(&mut self, ctx: &egui::Context) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !hovering {
            return;
        }
        let rect = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop-overlay"),
        ));
        painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(10, 14, 18, 200));
        painter.rect_stroke(
            rect.shrink(18.0),
            14.0,
            egui::Stroke::new(2.0, theme::PALETTE.accent),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            t(Key::DropToAdd),
            egui::FontId::proportional(22.0),
            theme::PALETTE.text,
        );
    }

    pub(super) fn top_bar(&mut self, ui: &mut Ui) {
        let p = theme::PALETTE;
        let ctx = ui.ctx().clone();
        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(p.panel).inner_margin(Margin::symmetric(14, 9)))
            .show(ui, |ui| {
                ui.add_enabled_ui(self.job.is_none(), |ui| {
                    ui.horizontal(|ui| {
                        if let Some(logo) = &self.logo {
                            ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(26.0, 26.0)));
                        }
                        ui.label(RichText::new(t(Key::AppTitle)).size(18.0).strong().color(p.accent));
                        ui.add_space(10.0);
                        if ui.button(t(Key::Open)).clicked() {
                            self.pick_archive(&ctx);
                        }
                        let has = self.loaded.is_some();
                        ui.add_enabled_ui(has, |ui| {
                            ui.menu_button(format!("{} ▾", t(Key::Extract)), |ui| {
                                if ui.button(t(Key::ExtractSmartHere)).clicked() {
                                    ui.close();
                                    self.extract_smart_here(&ctx);
                                }
                                if ui.button(t(Key::ExtractTo)).clicked() {
                                    ui.close();
                                    self.begin_extract(false);
                                }
                            });
                        });
                        if ui.button(t(Key::CreateArchive)).clicked() {
                            self.begin_create(Vec::new());
                        }
                        if has && ui.button(t(Key::CloseArchive)).clicked() {
                            self.loaded = None;
                            self.preview.clear();
                        }
                        let legacy = self
                            .loaded
                            .as_ref()
                            .and_then(|l| l.archive.legacy_names().map(|n| n.current));
                        if let Some(enc) = legacy
                            && ui.button(tf(Key::EncodingButton, &[enc.label()])).clicked()
                        {
                            self.open_encoding_dialog();
                        }
                        if self
                            .loaded
                            .as_ref()
                            .is_some_and(|l| l.has_encrypted() && l.path.is_some())
                            && ui.button(format!("🔒 {}", t(Key::PasswordEnter))).clicked()
                        {
                            self.ask_password_for_open_archive();
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(format!("⚙ {}", t(Key::Settings))).clicked() {
                                self.settings_open = true;
                            }
                            if let UpdateState::Available(rel) = &self.update {
                                let label = tf(Key::UpdateAvailable, &[&crate::update::version_text(rel.version)]);
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new(label).color(Color32::from_rgb(10, 30, 26)))
                                            .fill(p.accent),
                                    )
                                    .clicked()
                                {
                                    self.update_open = true;
                                }
                            }
                        });
                    });
                });
            });
    }

    pub(super) fn status_bar(&mut self, ui: &mut Ui) {
        let p = theme::PALETTE;
        egui::Panel::bottom("bottom")
            .frame(egui::Frame::new().fill(p.panel).inner_margin(Margin::symmetric(14, 6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.demo {
                        ui.label(RichText::new(t(Key::DemoBanner)).color(p.warn));
                    } else if let Some(l) = &self.loaded {
                        let entries = l.archive.entries();
                        let dirs = entries.iter().filter(|e| e.is_dir()).count();
                        let files = entries.len() - dirs;
                        let size: u64 = entries.iter().map(|e| e.size).sum();
                        let text = tf(
                            Key::StatusArchive,
                            &[
                                l.archive.kind().label(),
                                &files.to_string(),
                                &dirs.to_string(),
                                &theme::human_size(size),
                                &theme::human_size(l.file_len),
                            ],
                        );
                        ui.label(theme::dim(text));
                        if !l.selected.is_empty() {
                            ui.label(theme::dim(format!(
                                "· {}",
                                tf(Key::StatusSelected, &[&l.selected.len().to_string()])
                            )));
                        }
                        if l.uses_zipcrypto() {
                            ui.label(RichText::new(t(Key::WeakEncryption)).color(p.warn))
                                .on_hover_text(t(Key::WeakEncryptionHint));
                        }
                    }
                    if let Some((msg, at)) = &self.toast
                        && at.elapsed().as_secs() < 6
                    {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(msg).color(p.accent))
                        });
                    }
                });
            });
    }

    pub(super) fn welcome_view(&mut self, ui: &mut Ui) {
        let p = theme::PALETTE;
        let ctx = ui.ctx().clone();
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.18).max(20.0));
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(96.0, 96.0)));
            }
            ui.add_space(10.0);
            ui.label(RichText::new(t(Key::WelcomeTitle)).size(24.0).strong());
            ui.label(theme::dim(t(Key::WelcomeHint)));
            ui.add_space(14.0);
            if theme::primary(ui, t(Key::Open)).clicked() {
                self.pick_archive(&ctx);
            }
            if !self.settings.recent.is_empty() {
                ui.add_space(26.0);
                ui.label(RichText::new(t(Key::Recent)).color(p.dim).size(13.0));
                ui.add_space(4.0);
                let mut to_open = None;
                for r in self.settings.recent.iter().take(6) {
                    let name = std::path::Path::new(r)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| r.clone());
                    let resp = ui
                        .add(egui::Button::new(RichText::new(name).size(14.0)).frame(false))
                        .on_hover_text(r);
                    if resp.clicked() {
                        to_open = Some(PathBuf::from(r));
                    }
                }
                if let Some(path) = to_open
                    && !self.demo
                {
                    self.open_path(&ctx, path);
                }
            }
        });
    }

    pub(super) fn archive_view(&mut self, ui: &mut Ui) {
        let p = theme::PALETTE;
        let sort = self.sort;
        if self.loaded.is_none() {
            return;
        }
        let mut ask_password = false;
        egui::Panel::right("preview")
            .resizable(true)
            .default_size(380.0)
            .size_range(260.0..=720.0)
            .frame(
                egui::Frame::new()
                    .fill(p.panel)
                    .inner_margin(Margin::same(12))
                    .stroke(egui::Stroke::new(1.0, p.border)),
            )
            .show(ui, |ui| {
                ask_password = super::preview::show(&mut self.preview, ui);
            });
        if ask_password {
            self.ask_password_for_open_archive();
        }
        let Some(l) = self.loaded.as_ref() else { return };
        let rows = l.tree.visible(&l.expanded, sort);
        let mut actions: Vec<RowAction> = Vec::new();

        let arrow = |k: SortKey| -> &'static str {
            if sort.key != k {
                ""
            } else if sort.descending {
                " ▼"
            } else {
                " ▲"
            }
        };

        egui::Frame::new().inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(Sense::click_and_drag())
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::remainder().at_least(240.0).clip(true))
                .column(Column::exact(90.0))
                .column(Column::exact(90.0))
                .column(Column::exact(64.0))
                .column(Column::exact(130.0))
                .column(Column::exact(70.0))
                .header(26.0, |mut h| {
                    for (label, key) in [
                        (t(Key::ColName), Some(SortKey::Name)),
                        (t(Key::ColSize), Some(SortKey::Size)),
                        (t(Key::ColPacked), Some(SortKey::Packed)),
                        (t(Key::ColRatio), Some(SortKey::Ratio)),
                        (t(Key::ColModified), Some(SortKey::Modified)),
                        (t(Key::ColMethod), None),
                    ] {
                        h.col(|ui| {
                            let text = RichText::new(format!("{label}{}", key.map_or("", arrow)))
                                .color(p.dim)
                                .strong();
                            if ui.add(egui::Button::new(text).frame(false)).clicked()
                                && let Some(k) = key
                            {
                                actions.push(RowAction::Sort(k));
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(24.0, rows.len(), |mut row| {
                        let (id, depth) = rows[row.index()];
                        let n = &l.tree.nodes[id];
                        row.set_selected(l.selected.contains(&id));
                        let expanded = l.expanded.contains(&id);
                        let mut arrow_clicked = false;
                        row.col(|ui| {
                            ui.add_space(depth as f32 * 18.0);
                            if n.is_dir {
                                let glyph = if expanded { "▾" } else { "▸" };
                                if ui
                                    .add(egui::Label::new(RichText::new(glyph).color(p.dim)).sense(Sense::click()))
                                    .clicked()
                                {
                                    arrow_clicked = true;
                                }
                                let (r, _) = ui.allocate_exact_size(vec2(15.0, 12.0), Sense::hover());
                                ui.painter().rect_filled(r, 3.0, Color32::from_rgb(232, 178, 80));
                            } else {
                                ui.add_space(14.0);
                                let (r, _) = ui.allocate_exact_size(vec2(11.0, 14.0), Sense::hover());
                                ui.painter().rect_stroke(
                                    r,
                                    2.0,
                                    egui::Stroke::new(1.3, p.dim),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            ui.add_space(4.0);
                            ui.label(&n.name);
                        });
                        row.col(|ui| {
                            if !n.is_dir || n.files > 0 {
                                ui.label(theme::human_size(n.size));
                            }
                        });
                        row.col(|ui| {
                            if !n.is_dir || n.files > 0 {
                                ui.label(theme::dim(theme::human_size(n.packed)));
                            }
                        });
                        row.col(|ui| {
                            if n.size > 0 {
                                ui.label(theme::dim(format!("{:.0}%", n.packed as f64 * 100.0 / n.size as f64)));
                            }
                        });
                        row.col(|ui| {
                            if let Some(t) = n.mtime {
                                ui.label(theme::dim(timeutil::format_minutes(timeutil::utc_to_local(t))));
                            }
                        });
                        row.col(|ui| {
                            ui.label(theme::dim(&n.method));
                        });
                        let resp = row.response();
                        if resp.clicked() && !arrow_clicked {
                            let m = resp.ctx.input(|i| i.modifiers);
                            actions.push(RowAction::Select {
                                node: id,
                                ctrl: m.command,
                                shift: m.shift,
                            });
                        }
                        if arrow_clicked || (resp.double_clicked() && n.is_dir) {
                            actions.push(RowAction::Toggle(id));
                        }
                        if resp.drag_started() {
                            actions.push(RowAction::DragOut(id));
                        }
                        resp.context_menu(|ui| {
                            if !l.selected.contains(&id) {
                                actions.push(RowAction::Select {
                                    node: id,
                                    ctrl: false,
                                    shift: false,
                                });
                            }
                            if ui.button(t(Key::ExtractTo)).clicked() {
                                actions.push(RowAction::ExtractSelected);
                                ui.close();
                            }
                            if ui.button(t(Key::Expand)).clicked() {
                                actions.push(RowAction::ExpandAll);
                                ui.close();
                            }
                            if ui.button(t(Key::Collapse)).clicked() {
                                actions.push(RowAction::CollapseAll);
                                ui.close();
                            }
                        });
                    });
                });
        });

        let visible_ids: Vec<usize> = rows.iter().map(|(id, _)| *id).collect();
        for a in actions {
            match a {
                RowAction::Sort(k) => {
                    self.sort = Sort {
                        key: k,
                        descending: self.sort.key == k && !self.sort.descending,
                    };
                }
                RowAction::Toggle(id) => {
                    if let Some(l) = self.loaded.as_mut()
                        && !l.expanded.remove(&id)
                    {
                        l.expanded.insert(id);
                    }
                }
                RowAction::Select { node, ctrl, shift } => {
                    let Some(l) = self.loaded.as_mut() else { continue };
                    if shift && let Some(anchor) = l.last_clicked.and_then(|a| visible_ids.iter().position(|v| *v == a))
                    {
                        let here = visible_ids.iter().position(|v| *v == node).unwrap_or(anchor);
                        let (lo, hi) = (anchor.min(here), anchor.max(here));
                        if !ctrl {
                            l.selected.clear();
                        }
                        l.selected.extend(&visible_ids[lo..=hi]);
                    } else if ctrl {
                        if !l.selected.remove(&node) {
                            l.selected.insert(node);
                        }
                        l.last_clicked = Some(node);
                    } else {
                        l.selected = HashSet::from([node]);
                        l.last_clicked = Some(node);
                    }
                }
                RowAction::ExtractSelected => self.begin_extract(true),
                RowAction::DragOut(node) => self.drag_out(node),
                RowAction::ExpandAll => {
                    if let Some(l) = self.loaded.as_mut() {
                        l.expanded = l.tree.dir_ids().collect();
                    }
                }
                RowAction::CollapseAll => {
                    if let Some(l) = self.loaded.as_mut() {
                        l.expanded.clear();
                    }
                }
            }
        }
    }
}
