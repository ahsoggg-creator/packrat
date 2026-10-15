//! The main window.

mod dialogs;
mod jobs;
mod shots;
pub mod theme;
mod tree;
mod views;

use crate::i18n::{self, Key, t, tf};
use crate::settings::Settings;
use crate::{diag, update};
use eframe::egui::{self, Ui};
use jobs::{Job, Loaded, Outcome};
use packrat_core::create::Format;
use packrat_core::extract::{ExtractOptions, Plan, Resolution};
use packrat_core::zip::ZipMethod;
use packrat_core::{codec::stream::Compression, timeutil};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use tree::Sort;

pub struct StartOptions {
    pub open: Vec<PathBuf>,
    pub demo: bool,
    pub screenshots: Option<PathBuf>,
}

pub fn run(opts: StartOptions) -> eframe::Result {
    let icon = egui::IconData {
        rgba: include_bytes!("../../assets/icon64.rgba").to_vec(),
        width: 64,
        height: 64,
    };
    let shots = opts.screenshots.is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Packrat")
        .with_inner_size([1100.0, 700.0])
        .with_min_inner_size([760.0, 480.0])
        .with_icon(Arc::new(icon))
        .with_drag_and_drop(true);
    if shots {
        viewport = viewport.with_resizable(false);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Packrat",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            theme::install_fonts(&cc.egui_ctx, false);
            Ok(Box::new(App::new(&cc.egui_ctx, opts)))
        }),
    )
}

pub(crate) struct ExtractDialog {
    pub dest: String,
    pub smart: bool,
    pub selected_only: bool,
    pub selected_count: usize,
}

pub(crate) struct ConflictDialog {
    pub plan: Box<Plan>,
    pub opts: Box<ExtractOptions>,
    pub choice: Resolution,
}

pub(crate) struct CreateDialog {
    pub items: Vec<PathBuf>,
    pub format: usize,
    pub level: u32,
    pub output: String,
    pub output_edited: bool,
}

pub(crate) struct ResultInfo {
    pub ok: bool,
    pub lines: Vec<String>,
    pub details: Vec<String>,
    pub folder: Option<PathBuf>,
}

pub(crate) enum UpdateState {
    Idle,
    Checking,
    Available(update::Release),
    UpToDate,
    Downloading,
    Failed(String),
}

pub(crate) enum UpdateMsg {
    Checked(Result<Option<update::Release>, String>),
    Downloaded(Result<PathBuf, String>),
}

pub(crate) const FORMATS: [(Key, &str); 7] = [
    (Key::FormatZip, "zip"),
    (Key::FormatZipZstd, "zip-zstd"),
    (Key::Format7z, "7z"),
    (Key::FormatTarZst, "tar.zst"),
    (Key::FormatTarLz4, "tar.lz4"),
    (Key::FormatTarGz, "tar.gz"),
    (Key::FormatTarXz, "tar.xz"),
];

pub(crate) struct App {
    pub settings: Settings,
    pub loaded: Option<Loaded>,
    pub job: Option<Job>,
    pub extract: Option<ExtractDialog>,
    pub conflicts: Option<ConflictDialog>,
    pub create: Option<CreateDialog>,
    pub result: Option<ResultInfo>,
    pub settings_open: bool,
    pub update_open: bool,
    pub update: UpdateState,
    pub update_rx: Option<Receiver<UpdateMsg>>,
    pub download: Arc<(AtomicU64, AtomicU64)>,
    pub sort: Sort,
    pub demo: bool,
    pub cjk_loaded: bool,
    pub toast: Option<(String, std::time::Instant)>,
    pub crash_note: Option<String>,
    pub shots: Option<shots::Runner>,
    pub logo: Option<egui::TextureHandle>,
}

impl App {
    fn new(ctx: &egui::Context, opts: StartOptions) -> Self {
        let settings = Settings::load();
        i18n::set_lang(i18n::Lang::from_code(&settings.language));
        let crash_note = diag::last_crash().and_then(|p| {
            let fresh = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .map(|t| {
                    timeutil::system_time_to_unix(t)
                        > timeutil::system_time_to_unix(std::time::SystemTime::now()) - 3 * 86_400
                })
                .unwrap_or(false);
            fresh.then(|| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
        });
        let mut app = App {
            settings,
            loaded: None,
            job: None,
            extract: None,
            conflicts: None,
            create: None,
            result: None,
            settings_open: false,
            update_open: false,
            update: UpdateState::Idle,
            update_rx: None,
            download: Arc::new((AtomicU64::new(0), AtomicU64::new(0))),
            sort: Sort::default(),
            demo: opts.demo || opts.screenshots.is_some(),
            cjk_loaded: false,
            toast: None,
            crash_note,
            shots: opts.screenshots.map(shots::Runner::new),
            logo: load_logo(ctx),
        };
        if app.demo {
            app.settings.recent = crate::demo::recent_files();
            if app.shots.is_none() {
                app.load_demo(ctx);
            }
        } else if let Some(p) = opts.open.into_iter().next() {
            app.open_path(ctx, p);
        } else if update::due(&app.settings, now_unix()) {
            app.check_updates(ctx, false);
        }
        app
    }

    pub fn load_demo(&mut self, ctx: &egui::Context) {
        let (archive, name, len) = crate::demo::archive();
        let loaded = Loaded::new(archive, None, name, len);
        self.adopt(ctx, loaded);
    }

    fn adopt(&mut self, ctx: &egui::Context, loaded: Loaded) {
        let wants_cjk = loaded
            .archive
            .entries()
            .iter()
            .any(|e| e.path.chars().any(|c| (c as u32) >= 0x2E80));
        if wants_cjk && !self.cjk_loaded {
            theme::install_fonts(ctx, true);
            self.cjk_loaded = true;
        }
        if let Some(p) = &loaded.path {
            self.settings.remember(&p.to_string_lossy());
            self.save_settings();
        }
        self.loaded = Some(loaded);
    }

    pub fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        if self.job.is_some() {
            return;
        }
        self.job = Some(Job::spawn(ctx, t(Key::Working).to_owned(), move |_| {
            Outcome::Opened(jobs::open_loaded(path))
        }));
    }

    pub fn open_or_create(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        if self.job.is_some() || paths.is_empty() {
            return;
        }
        self.job = Some(Job::spawn(ctx, t(Key::Working).to_owned(), move |_| {
            if paths.len() == 1
                && let Ok(l) = jobs::open_loaded(paths[0].clone())
            {
                return Outcome::Opened(Ok(l));
            }
            Outcome::NotAnArchive(paths)
        }));
    }

    /// Settings are never written in demo mode (they hold made-up data there).
    pub fn save_settings(&self) {
        if !self.demo {
            self.settings.save();
        }
    }

    pub fn threads(&self) -> usize {
        self.settings.threads
    }

    pub fn check_updates(&mut self, ctx: &egui::Context, user_initiated: bool) {
        if matches!(self.update, UpdateState::Checking | UpdateState::Downloading) {
            return;
        }
        self.update = UpdateState::Checking;
        self.settings.last_update_check = now_unix();
        self.save_settings();
        let (tx, rx) = channel();
        self.update_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = update::check_latest();
            if let Err(e) = &r {
                diag::warn(&format!("update check failed: {e}"));
            }
            let _ = tx.send(UpdateMsg::Checked(r));
            ctx.request_repaint();
        });
        if user_initiated {
            self.update_open = true;
        }
    }

    pub fn download_update(&mut self, ctx: &egui::Context, rel: update::Release) {
        self.update = UpdateState::Downloading;
        self.download.0.store(0, Ordering::Relaxed);
        self.download.1.store(0, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.update_rx = Some(rx);
        let ctx = ctx.clone();
        let dl = self.download.clone();
        std::thread::spawn(move || {
            let result = std::env::current_exe().map_err(|e| e.to_string()).and_then(|cur| {
                let new = update::download_verified(&rel, &cur, &|done, total| {
                    dl.0.store(done, Ordering::Relaxed);
                    dl.1.store(total, Ordering::Relaxed);
                    ctx.request_repaint();
                })?;
                update::apply(&new, &cur)?;
                Ok(cur)
            });
            match &result {
                Ok(_) => diag::info(&format!("updated to {}", rel.tag)),
                Err(e) => diag::error(&format!("update failed: {e}")),
            }
            let _ = tx.send(UpdateMsg::Downloaded(result));
            ctx.request_repaint();
        });
    }

    fn poll_update(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.update_rx else { return };
        let Ok(msg) = rx.try_recv() else { return };
        self.update_rx = None;
        match msg {
            UpdateMsg::Checked(Ok(Some(rel))) => self.update = UpdateState::Available(rel),
            UpdateMsg::Checked(Ok(None)) => self.update = UpdateState::UpToDate,
            UpdateMsg::Checked(Err(e)) => self.update = UpdateState::Failed(e),
            UpdateMsg::Downloaded(Ok(exe)) => {
                let _ = std::process::Command::new(exe).spawn();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            UpdateMsg::Downloaded(Err(e)) => self.update = UpdateState::Failed(e),
        }
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let Ok(outcome) = job.rx.try_recv() else { return };
        self.job = None;
        match outcome {
            Outcome::Opened(Ok(l)) => self.adopt(ctx, *l),
            Outcome::Opened(Err(e)) => {
                diag::warn(&format!("open failed: {e}"));
                self.result = Some(ResultInfo {
                    ok: false,
                    lines: vec![tf(Key::ErrorGeneric, &[&e])],
                    details: vec![],
                    folder: None,
                });
            }
            Outcome::NotAnArchive(paths) => self.begin_create(paths),
            Outcome::Planned { plan, opts } => {
                if plan.conflicts.is_empty() {
                    self.run_execute(ctx, *plan, *opts);
                } else {
                    self.conflicts = Some(ConflictDialog {
                        plan,
                        opts,
                        choice: Resolution::Skip,
                    });
                }
            }
            Outcome::Extracted { report, secs, dest } => self.finish_extract(report, secs, dest),
            Outcome::Created { result, output, secs } => self.finish_create(result, output, secs),
        }
    }

    pub fn format_from_index(i: usize) -> Format {
        match FORMATS[i].1 {
            "zip" => Format::Zip(ZipMethod::Deflate),
            "zip-zstd" => Format::Zip(ZipMethod::Zstd),
            "7z" => Format::SevenZ,
            "tar.zst" => Format::Tar(Compression::Zst),
            "tar.lz4" => Format::Tar(Compression::Lz4),
            "tar.gz" => Format::Tar(Compression::Gz),
            _ => Format::Tar(Compression::Xz),
        }
    }

    pub fn show_toast(&mut self, text: String) {
        self.toast = Some((text, std::time::Instant::now()));
    }
}

fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(include_bytes!("../../assets/icon.png"))
        .ok()?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(ctx.load_texture(
        "logo",
        egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
        egui::TextureOptions::LINEAR,
    ))
}

pub(crate) fn now_unix() -> i64 {
    timeutil::system_time_to_unix(std::time::SystemTime::now())
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job(&ctx);
        self.poll_update(&ctx);
        self.handle_drops(&ctx);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) && self.job.is_none() {
            self.pick_archive(&ctx);
        }
        if self.job.is_some() || matches!(self.update, UpdateState::Downloading | UpdateState::Checking) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        self.top_bar(ui);
        self.status_bar(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::PALETTE.bg))
            .show(ui, |ui| {
                ui.add_enabled_ui(self.job.is_none(), |ui| {
                    if self.loaded.is_some() {
                        self.archive_view(ui);
                    } else {
                        self.welcome_view(ui);
                    }
                });
            });

        self.dialogs(&ctx);
        self.drop_overlay(&ctx);
        shots::tick(self, &ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use packrat_core::create::{CreateOptions, create_archive};

    fn wait_for_job(app: &mut App, ctx: &egui::Context) {
        for _ in 0..4000 {
            app.poll_job(ctx);
            if app.job.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the job did not finish");
    }

    fn make_archive(dir: &std::path::Path, format: Format) -> PathBuf {
        let src = dir.join("proj");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "alpha").unwrap();
        std::fs::write(src.join("sub").join("b.txt"), "beta").unwrap();
        let archive = dir.join(format!("proj.{}", format.extension()));
        create_archive(
            &archive,
            &[src],
            &CreateOptions::new(format),
            &packrat_core::Progress::new(),
        )
        .unwrap();
        archive
    }

    #[test]
    fn open_extract_and_conflicts_through_the_app_logic() {
        crate::settings::use_test_data_dir();
        let dir = tempfile::tempdir().unwrap();
        let archive = make_archive(dir.path(), Format::Zip(ZipMethod::Deflate));
        let ctx = egui::Context::default();
        let mut app = App::new(
            &ctx,
            StartOptions {
                open: vec![archive],
                demo: false,
                screenshots: None,
            },
        );
        wait_for_job(&mut app, &ctx);
        assert!(app.loaded.is_some(), "archive opened");

        let dest = dir.path().join("out");
        std::fs::create_dir(&dest).unwrap();
        app.start_extract(&ctx, dest.clone(), true, false);
        wait_for_job(&mut app, &ctx); // plan
        assert!(app.conflicts.is_none());
        wait_for_job(&mut app, &ctx); // execute
        let r = app.result.take().expect("result is shown");
        assert!(r.ok, "{:?}", r.lines);
        assert_eq!(std::fs::read_to_string(dest.join("proj/sub/b.txt")).unwrap(), "beta");
        assert!(!dest.join("proj/proj").exists());

        // second run: files exist, so the user is asked
        std::fs::write(dest.join("proj/a.txt"), "edited").unwrap();
        app.start_extract(&ctx, dest.clone(), true, false);
        wait_for_job(&mut app, &ctx);
        let c = app.conflicts.as_ref().expect("conflict dialog");
        assert_eq!(c.plan.conflicts.len(), 2);
        assert_eq!(
            std::fs::read_to_string(dest.join("proj/a.txt")).unwrap(),
            "edited",
            "nothing written before the decision"
        );
        let c = app.conflicts.take().unwrap();
        let mut opts = c.opts;
        opts.default_resolution = Resolution::KeepBoth;
        app.run_execute(&ctx, *c.plan, *opts);
        wait_for_job(&mut app, &ctx);
        assert_eq!(std::fs::read_to_string(dest.join("proj/a.txt")).unwrap(), "edited");
        assert_eq!(std::fs::read_to_string(dest.join("proj/a (2).txt")).unwrap(), "alpha");
    }

    #[test]
    fn dropped_non_archive_opens_the_create_dialog() {
        crate::settings::use_test_data_dir();
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("notes.txt");
        std::fs::write(&plain, "just text").unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(
            &ctx,
            StartOptions {
                open: vec![],
                demo: true,
                screenshots: None,
            },
        );
        app.demo = false;
        app.loaded = None;
        app.open_or_create(&ctx, vec![plain.clone()]);
        wait_for_job(&mut app, &ctx);
        let c = app.create.as_ref().expect("create dialog");
        assert_eq!(c.items, vec![plain]);
    }

    #[test]
    fn create_through_the_app_logic_produces_a_readable_archive() {
        crate::settings::use_test_data_dir();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data.txt");
        std::fs::write(&file, "x".repeat(10_000)).unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(
            &ctx,
            StartOptions {
                open: vec![],
                demo: true,
                screenshots: None,
            },
        );
        app.demo = false;
        app.begin_create(vec![file]);
        let out = dir.path().join("data.tar.zst");
        {
            let c = app.create.as_mut().unwrap();
            c.format = FORMATS.iter().position(|(_, n)| *n == "tar.zst").unwrap();
            c.output = out.to_string_lossy().into_owned();
            c.output_edited = true;
        }
        app.start_create_for_test(&ctx);
        wait_for_job(&mut app, &ctx);
        assert!(app.result.as_ref().is_some_and(|r| r.ok));
        assert!(
            packrat_core::open::open(&out)
                .unwrap()
                .entries()
                .iter()
                .any(|e| e.path == "data.txt")
        );
    }
}
