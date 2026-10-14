//! Dialog windows and the operations they start.

use super::jobs::{Job, Outcome};
use super::{App, CreateDialog, ExtractDialog, FORMATS, ResultInfo, UpdateState, theme};
use crate::i18n::{self, Key, Lang, t, tf};
use crate::installer::{self, Env};
use crate::{diag, update, winint};
use eframe::egui::{self, Align2, RichText, Ui, vec2};
use packrat_core::create::{CreateOptions, CreateReport, create_archive};
use packrat_core::extract::{self, ExtractOptions, ExtractReport, Plan, Resolution};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

fn window(title: &str) -> egui::Window<'_> {
    egui::Window::new(RichText::new(title).strong())
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(theme::card())
}

fn default_dest(app: &App) -> String {
    if let Some(d) = &app.settings.last_dest {
        return d.clone();
    }
    if let Some(p) = app
        .loaded
        .as_ref()
        .and_then(|l| l.path.as_ref())
        .and_then(|p| p.parent())
    {
        return p.to_string_lossy().into_owned();
    }
    std::env::var("USERPROFILE")
        .map(|p| format!("{p}\\Documents"))
        .unwrap_or_default()
}

pub(super) fn archive_stem_name(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".into())
}

impl App {
    pub(super) fn begin_extract(&mut self, selected_only: bool) {
        let Some(l) = &self.loaded else { return };
        let selected_count = l.selected.len();
        self.extract = Some(ExtractDialog {
            dest: default_dest(self),
            smart: self.settings.smart_default,
            selected_only: selected_only && selected_count > 0,
            selected_count,
        });
    }

    pub(super) fn extract_smart_here(&mut self, ctx: &egui::Context) {
        let dest = self
            .loaded
            .as_ref()
            .and_then(|l| l.path.as_ref())
            .and_then(|p| p.parent())
            .map(Path::to_path_buf);
        match dest {
            Some(d) => self.start_extract(ctx, d, true, false),
            None => self.show_toast(t(Key::DemoNoWrite).to_owned()),
        }
    }

    pub(super) fn start_extract(&mut self, ctx: &egui::Context, dest: PathBuf, smart: bool, selected_only: bool) {
        if self.demo {
            self.show_toast(t(Key::DemoNoWrite).to_owned());
            return;
        }
        let Some(l) = &self.loaded else { return };
        let mut opts = ExtractOptions::new(&dest, l.name.clone());
        opts.smart = smart;
        opts.threads = self.threads();
        if selected_only {
            opts.selection = Some(l.tree.entries_under(&l.selected));
        }
        self.settings.last_dest = Some(dest.to_string_lossy().into_owned());
        self.save_settings();
        let archive = l.archive.clone();
        diag::info(&format!(
            "extract started: {} entries, smart={smart}",
            opts.selection.as_ref().map_or(archive.entries().len(), Vec::len)
        ));
        self.job = Some(Job::spawn(ctx, t(Key::Working).to_owned(), move |_| {
            let plan = extract::plan(&*archive, &opts);
            Outcome::Planned {
                plan: Box::new(plan),
                opts: Box::new(opts),
            }
        }));
    }

    pub(super) fn run_execute(&mut self, ctx: &egui::Context, plan: Plan, opts: ExtractOptions) {
        let Some(l) = &self.loaded else { return };
        let archive = l.archive.clone();
        let dest = plan.base.clone();
        self.job = Some(Job::spawn(ctx, t(Key::Extract).to_owned(), move |progress| {
            let started = std::time::Instant::now();
            let report = extract::execute(&*archive, &plan, &opts, &progress).map_err(|e| e.to_string());
            Outcome::Extracted {
                report,
                secs: started.elapsed().as_secs_f64(),
                dest,
            }
        }));
    }

    pub(super) fn finish_extract(&mut self, report: Result<ExtractReport, String>, secs: f64, dest: PathBuf) {
        match report {
            Err(e) => {
                diag::error(&format!("extract failed: {e}"));
                self.result = Some(ResultInfo {
                    ok: false,
                    lines: vec![tf(Key::ErrorGeneric, &[&e])],
                    details: vec![],
                    folder: None,
                });
            }
            Ok(r) => {
                diag::info(&format!(
                    "extract finished: {} files, {} blocked, {} failed, {} skipped, cancelled={}",
                    r.files,
                    r.blocked.len(),
                    r.failed.len(),
                    r.skipped_existing.len(),
                    r.cancelled
                ));
                let mut lines = Vec::new();
                if r.cancelled {
                    lines.push(t(Key::Cancelled).to_owned());
                }
                lines.push(tf(
                    Key::DoneExtract,
                    &[&r.files.to_string(), &theme::human_size(r.bytes), &format!("{secs:.1}")],
                ));
                if !r.skipped_existing.is_empty() {
                    lines.push(tf(Key::SkippedExisting, &[&r.skipped_existing.len().to_string()]));
                }
                if !r.kept_both.is_empty() {
                    lines.push(tf(Key::KeptBoth, &[&r.kept_both.len().to_string()]));
                }
                if !r.blocked.is_empty() {
                    lines.push(tf(Key::BlockedEntries, &[&r.blocked.len().to_string()]));
                }
                if !r.failed.is_empty() {
                    lines.push(tf(Key::FailedEntries, &[&r.failed.len().to_string()]));
                }
                let mut details = Vec::new();
                details.extend(r.blocked.iter().map(|i| format!("⛔ {} — {}", i.path, i.message)));
                details.extend(r.failed.iter().map(|i| format!("✖ {} — {}", i.path, i.message)));
                details.extend(r.skipped_existing.iter().map(|p| format!("⏭ {p}")));
                details.extend(r.kept_both.iter().map(|(a, b)| format!("⧉ {a} → {b}")));
                self.result = Some(ResultInfo {
                    ok: r.blocked.is_empty() && r.failed.is_empty() && !r.cancelled,
                    lines,
                    details,
                    folder: Some(dest),
                });
            }
        }
    }

    pub(super) fn begin_create(&mut self, items: Vec<PathBuf>) {
        match &mut self.create {
            Some(c) => c.items.extend(items),
            None => {
                let format = FORMATS
                    .iter()
                    .position(|(_, n)| *n == self.settings.default_format)
                    .unwrap_or(0);
                self.create = Some(CreateDialog {
                    items,
                    format,
                    level: self.settings.level.clamp(1, 9),
                    output: String::new(),
                    output_edited: false,
                });
            }
        }
    }

    #[cfg(test)]
    pub(super) fn start_create_for_test(&mut self, ctx: &egui::Context) {
        self.start_create(ctx);
    }

    fn start_create(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.create else { return };
        if self.demo {
            self.show_toast(t(Key::DemoNoWrite).to_owned());
            return;
        }
        let output = PathBuf::from(&c.output);
        let items = c.items.clone();
        let mut opts = CreateOptions::new(Self::format_from_index(c.format));
        opts.level = c.level;
        opts.threads = self.threads();
        self.settings.default_format = FORMATS[c.format].1.to_owned();
        self.settings.level = c.level;
        self.save_settings();
        self.create = None;
        diag::info(&format!(
            "create started: {} inputs, format {}",
            items.len(),
            self.settings.default_format
        ));
        self.job = Some(Job::spawn(ctx, t(Key::Working).to_owned(), move |progress| {
            let started = std::time::Instant::now();
            let result = create_archive(&output, &items, &opts, &progress).map_err(|e| e.to_string());
            Outcome::Created {
                result,
                output,
                secs: started.elapsed().as_secs_f64(),
            }
        }));
    }

    pub(super) fn finish_create(&mut self, result: Result<CreateReport, String>, output: PathBuf, secs: f64) {
        match result {
            Err(e) => {
                diag::error(&format!("create failed: {e}"));
                self.result = Some(ResultInfo {
                    ok: false,
                    lines: vec![tf(Key::ErrorGeneric, &[&e])],
                    details: vec![],
                    folder: None,
                });
            }
            Ok(r) => {
                diag::info(&format!("create finished: {} entries", r.entries));
                let name = output
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let lines = vec![tf(
                    Key::DoneCreate,
                    &[
                        &name,
                        &theme::human_size(r.bytes_in),
                        &theme::human_size(r.bytes_out),
                        &format!("{secs:.1}"),
                    ],
                )];
                let details = r
                    .skipped
                    .iter()
                    .map(|(p, why)| format!("⏭ {} — {why}", p.display()))
                    .collect();
                self.result = Some(ResultInfo {
                    ok: true,
                    lines,
                    details,
                    folder: output.parent().map(Path::to_path_buf),
                });
            }
        }
    }

    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        self.progress_dialog(ctx);
        self.extract_dialog(ctx);
        self.conflict_dialog(ctx);
        self.create_dialog(ctx);
        self.result_dialog(ctx);
        self.settings_dialog(ctx);
        self.update_dialog(ctx);
        self.crash_note_bar(ctx);
    }

    fn progress_dialog(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let (frac, current, bytes) = (
            job.progress.fraction(),
            job.progress.current(),
            job.progress.bytes_done(),
        );
        let mut cancel = false;
        window(&job.title).show(ctx, |ui| {
            ui.set_min_width(420.0);
            if job.progress.bytes_total() > 0 || job.progress.files_total() > 0 {
                ui.add(egui::ProgressBar::new(frac).show_percentage().animate(true));
                ui.label(theme::dim(theme::human_size(bytes)));
            } else {
                ui.add(egui::Spinner::new());
            }
            if !current.is_empty() {
                ui.add(egui::Label::new(theme::dim(tf(Key::CurrentItem, &[&current]))).truncate());
            }
            if ui.button(t(Key::Cancel)).clicked() {
                cancel = true;
            }
        });
        if cancel {
            job.progress.cancel();
        }
    }

    fn extract_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = self.extract.as_mut() else { return };
        let (mut go, mut close) = (false, false);
        window(t(Key::ExtractTitle)).show(ctx, |ui| {
            ui.set_min_width(520.0);
            ui.label(t(Key::ExtractDestination));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.dest).desired_width(400.0));
                if ui.button(t(Key::Browse)).clicked()
                    && let Some(p) = rfd::FileDialog::new().pick_folder()
                {
                    d.dest = p.to_string_lossy().into_owned();
                }
            });
            ui.checkbox(&mut d.smart, t(Key::SmartExtract));
            if d.selected_count > 0 {
                ui.radio_value(
                    &mut d.selected_only,
                    true,
                    tf(Key::SelectedOnly, &[&d.selected_count.to_string()]),
                );
                ui.radio_value(&mut d.selected_only, false, t(Key::WholeArchive));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!d.dest.trim().is_empty(), |ui| {
                    if theme::primary(ui, t(Key::StartExtract)).clicked() {
                        go = true;
                    }
                });
                if ui.button(t(Key::Cancel)).clicked() {
                    close = true;
                }
            });
        });
        if go {
            let (dest, smart, only) = (PathBuf::from(d.dest.trim()), d.smart, d.selected_only);
            self.extract = None;
            self.start_extract(ctx, dest, smart, only);
        } else if close {
            self.extract = None;
        }
    }

    fn conflict_dialog(&mut self, ctx: &egui::Context) {
        let Some(c) = self.conflicts.as_mut() else { return };
        let (mut go, mut cancel) = (false, false);
        window(t(Key::ConflictsTitle)).show(ctx, |ui| {
            ui.set_min_width(520.0);
            ui.label(tf(Key::ConflictsText, &[&c.plan.conflicts.len().to_string()]));
            ui.add_space(4.0);
            for x in c.plan.conflicts.iter().take(6) {
                ui.add(egui::Label::new(theme::dim(x.target.display().to_string())).truncate());
            }
            if c.plan.conflicts.len() > 6 {
                ui.label(theme::dim(format!("… +{}", c.plan.conflicts.len() - 6)));
            }
            ui.add_space(6.0);
            ui.radio_value(&mut c.choice, Resolution::Skip, t(Key::ConflictSkip));
            ui.radio_value(&mut c.choice, Resolution::Replace, t(Key::ConflictReplace));
            ui.radio_value(&mut c.choice, Resolution::KeepBoth, t(Key::ConflictKeepBoth));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::primary(ui, t(Key::StartExtract)).clicked() {
                    go = true;
                }
                if ui.button(t(Key::Cancel)).clicked() {
                    cancel = true;
                }
            });
        });
        if go && let Some(c) = self.conflicts.take() {
            let mut opts = c.opts;
            opts.default_resolution = c.choice;
            self.run_execute(ctx, *c.plan, *opts);
        } else if cancel {
            self.conflicts = None;
        }
    }

    fn create_dialog(&mut self, ctx: &egui::Context) {
        let Some(c) = self.create.as_mut() else { return };
        if !c.output_edited {
            let ext = App::format_from_index(c.format).extension();
            let (base, parent) = match c.items.first() {
                Some(first) => (
                    if c.items.len() == 1 {
                        archive_stem_name(first)
                    } else {
                        first
                            .parent()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "archive".into())
                    },
                    first.parent().map(Path::to_path_buf).unwrap_or_default(),
                ),
                None => ("archive".into(), PathBuf::new()),
            };
            c.output = parent.join(format!("{base}.{ext}")).to_string_lossy().into_owned();
        }
        let (mut go, mut close) = (false, false);
        let mut remove = None;
        let mut add: Vec<PathBuf> = Vec::new();
        window(t(Key::CreateTitle)).show(ctx, |ui| {
            ui.set_min_width(560.0);
            ui.horizontal(|ui| {
                if ui.button(t(Key::AddFiles)).clicked()
                    && let Some(f) = rfd::FileDialog::new().pick_files()
                {
                    add.extend(f);
                }
                if ui.button(t(Key::AddFolder)).clicked()
                    && let Some(f) = rfd::FileDialog::new().pick_folder()
                {
                    add.push(f);
                }
            });
            egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                if c.items.is_empty() {
                    ui.label(theme::dim(t(Key::NothingAdded)));
                }
                for (i, p) in c.items.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("✕").on_hover_text(t(Key::RemoveItem)).clicked() {
                            remove = Some(i);
                        }
                        ui.add(egui::Label::new(p.display().to_string()).truncate());
                    });
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(t(Key::CreateFormat));
                egui::ComboBox::from_id_salt("format")
                    .width(380.0)
                    .selected_text(t(FORMATS[c.format].0))
                    .show_ui(ui, |ui| {
                        for (i, (key, _)) in FORMATS.iter().enumerate() {
                            ui.selectable_value(&mut c.format, i, t(*key));
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label(t(Key::CreateLevel));
                ui.add(egui::Slider::new(&mut c.level, 1..=9));
            });
            ui.label(t(Key::CreateName));
            ui.horizontal(|ui| {
                if ui
                    .add(egui::TextEdit::singleline(&mut c.output).desired_width(440.0))
                    .changed()
                {
                    c.output_edited = true;
                }
                if ui.button(t(Key::Browse)).clicked()
                    && let Some(p) = rfd::FileDialog::new()
                        .set_file_name(
                            Path::new(&c.output)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        )
                        .save_file()
                {
                    c.output = p.to_string_lossy().into_owned();
                    c.output_edited = true;
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!c.items.is_empty() && !c.output.trim().is_empty(), |ui| {
                    if theme::primary(ui, t(Key::StartCreate)).clicked() {
                        go = true;
                    }
                });
                if ui.button(t(Key::Cancel)).clicked() {
                    close = true;
                }
            });
        });
        if let Some(i) = remove {
            c.items.remove(i);
        }
        c.items.extend(add);
        if go {
            self.start_create(ctx);
        } else if close {
            self.create = None;
        }
    }

    fn result_dialog(&mut self, ctx: &egui::Context) {
        let Some(r) = &self.result else { return };
        let mut close = false;
        let mut open_folder = None;
        let title = if r.ok { "✔" } else { "⚠" };
        window(title).show(ctx, |ui| {
            ui.set_min_width(480.0);
            for (i, l) in r.lines.iter().enumerate() {
                let rt = if i == 0 {
                    RichText::new(l).strong().size(16.0)
                } else {
                    RichText::new(l).color(theme::PALETTE.warn)
                };
                ui.label(rt);
            }
            if !r.details.is_empty() {
                egui::CollapsingHeader::new(t(Key::ShowDetails)).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for d in r.details.iter().take(500) {
                            ui.add(egui::Label::new(theme::dim(d)).wrap());
                        }
                        if r.details.len() > 500 {
                            ui.label(theme::dim(format!("… +{}", r.details.len() - 500)));
                        }
                    });
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if let Some(f) = &r.folder
                    && ui.button(t(Key::OpenFolder)).clicked()
                {
                    open_folder = Some(f.clone());
                }
                if theme::primary(ui, t(Key::Ok)).clicked() {
                    close = true;
                }
            });
        });
        if let Some(f) = open_folder {
            winint::show_in_explorer(&f);
        }
        if close {
            self.result = None;
        }
    }

    fn settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut changed = false;
        let mut open = true;
        let mut check = false;
        let mut install = false;
        let mut note: Option<String> = None;
        egui::Window::new(RichText::new(t(Key::SettingsTitle)).strong())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::card())
            .show(ctx, |ui| {
                ui.set_min_width(500.0);
                let s = &mut self.settings;
                ui.horizontal(|ui| {
                    ui.label(t(Key::SettingLanguage));
                    let mut lang = Lang::from_code(&s.language);
                    egui::ComboBox::from_id_salt("lang")
                        .selected_text(if lang == Lang::Ru { "Русский" } else { "English" })
                        .show_ui(ui, |ui| {
                            changed |= ui.selectable_value(&mut lang, Lang::Ru, "Русский").changed();
                            changed |= ui.selectable_value(&mut lang, Lang::En, "English").changed();
                        });
                    if lang.code() != s.language {
                        s.language = lang.code().to_owned();
                        i18n::set_lang(lang);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(t(Key::SettingThreads));
                    let max = std::thread::available_parallelism().map_or(64, |n| n.get());
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut s.threads)
                                .range(0..=max)
                                .custom_formatter(|v, _| {
                                    if v == 0.0 {
                                        t(Key::ThreadsAuto).to_owned()
                                    } else {
                                        format!("{v}")
                                    }
                                }),
                        )
                        .changed();
                });
                changed |= ui.checkbox(&mut s.smart_default, t(Key::SettingSmartDefault)).changed();
                changed |= ui.checkbox(&mut s.check_updates, t(Key::SettingCheckUpdates)).changed();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button(t(Key::UpdateCheck)).clicked() {
                        check = true;
                    }
                    if let UpdateState::UpToDate = &self.update {
                        ui.label(theme::dim(t(Key::UpdateUpToDate)));
                    }
                });
                ui.separator();
                theme::heading(ui, t(Key::Diagnostics));
                ui.horizontal(|ui| {
                    if ui.button(t(Key::OpenLogsFolder)).clicked() {
                        let _ = std::fs::create_dir_all(diag::logs_dir());
                        winint::show_in_explorer(&diag::logs_dir());
                    }
                    if ui.button(t(Key::CopyDiagnostics)).clicked() {
                        ui.ctx().copy_text(diag::diagnostic_text(s));
                        note = Some(t(Key::Copied).to_owned());
                    }
                });
                let exe = std::env::current_exe().unwrap_or_default();
                if !installer::is_installed_copy(&Env::real(), &exe) && !self.demo {
                    ui.separator();
                    theme::heading(ui, t(Key::InstallSection));
                    if ui.button(t(Key::InstallNow)).clicked() {
                        install = true;
                    }
                }
                ui.separator();
                ui.label(theme::dim(format!(
                    "Packrat {} — {}",
                    env!("CARGO_PKG_VERSION"),
                    t(Key::AboutText)
                )));
            });
        if changed {
            self.save_settings();
        }
        if check {
            self.check_updates(ctx, false);
        }
        if install {
            let exe = std::env::current_exe().unwrap_or_default();
            match installer::install(&Env::real(), &exe) {
                Ok(p) => note = Some(tf(Key::Installed, &[&p.display().to_string()])),
                Err(e) => note = Some(tf(Key::ErrorGeneric, &[&e])),
            }
        }
        if let Some(n) = note {
            self.show_toast(n);
        }
        self.settings_open = open;
    }

    fn update_dialog(&mut self, ctx: &egui::Context) {
        if !self.update_open {
            return;
        }
        let mut open = true;
        let mut start = None;
        let state_label: String;
        let mut progress: Option<f32> = None;
        match &self.update {
            UpdateState::Checking => state_label = t(Key::UpdateChecking).to_owned(),
            UpdateState::Available(rel) => {
                state_label = tf(Key::UpdateAvailable, &[&update::version_text(rel.version)]);
                start = Some(rel.clone());
            }
            UpdateState::UpToDate => state_label = t(Key::UpdateUpToDate).to_owned(),
            UpdateState::Downloading => {
                state_label = t(Key::UpdateDownloading).to_owned();
                let (d, tot) = (
                    self.download.0.load(Ordering::Relaxed),
                    self.download.1.load(Ordering::Relaxed),
                );
                progress = Some(if tot > 0 { d as f32 / tot as f32 } else { 0.0 });
            }
            UpdateState::Failed(e) => state_label = tf(Key::UpdateFailed, &[e]),
            UpdateState::Idle => state_label = String::new(),
        }
        let mut go = None;
        egui::Window::new(RichText::new(t(Key::Update)).strong())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(theme::card())
            .show(ctx, |ui: &mut Ui| {
                ui.set_min_size(vec2(380.0, 60.0));
                ui.label(&state_label);
                if let Some(p) = progress {
                    ui.add(egui::ProgressBar::new(p).show_percentage());
                }
                if let Some(rel) = start
                    && theme::primary(ui, t(Key::UpdateInstall)).clicked()
                {
                    go = Some(rel);
                }
            });
        if let Some(rel) = go {
            self.download_update(ctx, rel);
        }
        self.update_open = open;
    }

    fn crash_note_bar(&mut self, ctx: &egui::Context) {
        let Some(name) = self.crash_note.clone() else { return };
        let mut dismiss = false;
        egui::Window::new("crash-note")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::RIGHT_BOTTOM, [-14.0, -44.0])
            .frame(theme::card())
            .show(ctx, |ui| {
                ui.label(RichText::new(tf(Key::CrashReportFound, &[&name])).color(theme::PALETTE.warn));
                ui.horizontal(|ui| {
                    if ui.button(t(Key::OpenLogsFolder)).clicked() {
                        winint::show_in_explorer(&diag::crashes_dir());
                    }
                    if ui.button(t(Key::Close)).clicked() {
                        dismiss = true;
                    }
                });
            });
        if dismiss {
            self.crash_note = None;
        }
    }
}
