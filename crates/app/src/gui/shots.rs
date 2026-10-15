//! `Packrat.exe --screenshots <dir>`: walks through a fixed set of scenes with the demo archive and saves
//! each one as a PNG. Used to regenerate `docs/screenshots`.

use super::{App, CreateDialog, ResultInfo};
use eframe::egui;
use std::path::PathBuf;

const SCENES: [&str; 6] = [
    "01-welcome",
    "02-archive",
    "03-extract",
    "04-create",
    "05-settings",
    "06-result",
];
const SETTLE_FRAMES: u32 = 6;

pub struct Runner {
    dir: PathBuf,
    step: usize,
    frames: u32,
    requested: bool,
}

impl Runner {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self {
            dir,
            step: 0,
            frames: 0,
            requested: false,
        }
    }
}

fn setup(app: &mut App, ctx: &egui::Context, step: usize) {
    app.extract = None;
    app.create = None;
    app.result = None;
    app.settings_open = false;
    if step == 0 {
        app.loaded = None;
        return;
    }
    if app.loaded.is_none() {
        app.load_demo(ctx);
    }
    if let Some(l) = app.loaded.as_mut() {
        l.expanded = l.tree.dir_ids().collect();
        if step >= 1 {
            let wanted: Vec<usize> = l
                .tree
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.name == "changelog.md" || n.name == "guide.md")
                .map(|(i, _)| i)
                .collect();
            l.selected = wanted.into_iter().collect();
        }
    }
    match step {
        2 => app.begin_extract(true),
        3 => {
            app.begin_create(vec![
                PathBuf::from(r"D:\Фото\Отпуск-2024"),
                PathBuf::from(r"D:\Фото\паспорт-скан.jpg"),
                PathBuf::from(r"D:\Документы\договор.pdf"),
            ]);
            if let Some(CreateDialog { level, .. }) = app.create.as_mut() {
                *level = 6;
            }
        }
        4 => app.settings_open = true,
        5 => {
            app.result = Some(ResultInfo {
                ok: false,
                lines: vec![
                    crate::i18n::tf(crate::i18n::Key::DoneExtract, &["31", "1,1 МБ", "0.4"]),
                    crate::i18n::tf(crate::i18n::Key::BlockedEntries, &["3"]),
                ],
                details: vec![
                    "⛔ ../../Windows/System32/evil.dll — '..' component (path traversal)".into(),
                    "⛔ C:\\autoexec.bat — drive letter in the path".into(),
                    "⛔ CON — reserved Windows device name 'CON'".into(),
                ],
                folder: None,
                wrong_password: false,
            });
        }
        _ => {}
    }
}

pub(super) fn tick(app: &mut App, ctx: &egui::Context) {
    let Some(mut r) = app.shots.take() else { return };
    ctx.request_repaint();
    ctx.global_style_mut(|s| s.animation_time = 0.0);
    if r.step >= SCENES.len() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        app.shots = Some(r);
        return;
    }
    if r.frames == 0 {
        setup(app, ctx, r.step);
    }
    r.frames += 1;
    if r.frames == SETTLE_FRAMES && !r.requested {
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        r.requested = true;
    }
    let image = ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    });
    if let Some(img) = image {
        let [w, h] = img.size;
        let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
        let path = r.dir.join(format!("{}.png", SCENES[r.step]));
        match image::save_buffer(&path, &rgba, w as u32, h as u32, image::ColorType::Rgba8) {
            Ok(()) => crate::winint::console_print(&format!("saved {}\n", path.display())),
            Err(e) => crate::winint::console_print(&format!("cannot save {}: {e}\n", path.display())),
        }
        r.step += 1;
        r.frames = 0;
        r.requested = false;
    }
    app.shots = Some(r);
}
