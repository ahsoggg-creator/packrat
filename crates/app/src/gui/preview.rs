//! The preview pane: images, text and code with syntax highlighting, and a hex dump for everything else. The data
//! comes from `packrat_core::preview` (no extraction to disk); decoding and highlighting run on a worker thread.

use super::theme;
use crate::i18n::{Key, t, tf};
use eframe::egui::{self, Color32, FontId, RichText, Ui, text::LayoutJob};
use packrat_core::preview::{ContentKind, Limits, classify, read_preview};
use packrat_core::{Archive, Error};
use std::io::Cursor;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, OnceLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

const TEXT_LIMIT: u64 = 256 << 10;
const IMAGE_LIMIT: u64 = 16 << 20;
const HEX_BYTES: usize = 4096;
const MAX_LINES: usize = 2500;
const MAX_LINE_CHARS: usize = 2000;
const THUMB_EDGE: u32 = 640;
const FULL_EDGE: u32 = 3072;

pub enum Body {
    Image {
        thumb: egui::ColorImage,
        full: egui::ColorImage,
        width: u32,
        height: u32,
    },
    Text(Vec<Vec<(String, [u8; 3])>>),
    Hex(String),
    NeedPassword,
    TooFar(String),
    Failed(String),
}

pub struct PreviewData {
    pub entry: usize,
    pub name: String,
    pub truncated: bool,
    pub slow: bool,
    pub body: Body,
}

fn syntaxes() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn themes() -> &'static ThemeSet {
    static T: OnceLock<ThemeSet> = OnceLock::new();
    T.get_or_init(ThemeSet::load_defaults)
}

fn looks_like_image(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp"]
        .iter()
        .any(|e| n.ends_with(e))
}

fn highlight(name: &str, text: &str) -> Vec<Vec<(String, [u8; 3])>> {
    let ss = syntaxes();
    let ext = name.rsplit('.').next().filter(|e| *e != name);
    let syntax = ext
        .and_then(|e| ss.find_syntax_by_extension(e))
        .or_else(|| text.lines().next().and_then(|l| ss.find_syntax_by_first_line(l)))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let theme = &themes().themes["base16-ocean.dark"];
    let mut h = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    for line in LinesWithEndings::from(text).take(MAX_LINES) {
        let spans = h.highlight_line(line, ss).unwrap_or_default();
        let mut row: Vec<(String, [u8; 3])> = Vec::new();
        let mut chars = 0;
        for (style, s) in spans {
            let s = s.trim_end_matches(['\n', '\r']);
            if s.is_empty() {
                continue;
            }
            let room = MAX_LINE_CHARS.saturating_sub(chars);
            if room == 0 {
                break;
            }
            let piece: String = s.chars().take(room).collect();
            chars += piece.chars().count();
            row.push((piece, [style.foreground.r, style.foreground.g, style.foreground.b]));
        }
        out.push(row);
    }
    out
}

fn hex_dump(data: &[u8]) -> String {
    let mut s = String::new();
    for (i, chunk) in data.chunks(16).take(HEX_BYTES / 16).enumerate() {
        s.push_str(&format!("{:08x}  ", i * 16));
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => s.push_str(&format!("{b:02x} ")),
                None => s.push_str("   "),
            }
            if j == 7 {
                s.push(' ');
            }
        }
        s.push(' ');
        s.extend(
            chunk
                .iter()
                .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }),
        );
        s.push('\n');
    }
    s
}

fn decode_image(data: &[u8]) -> Result<Body, String> {
    let mut reader = image::ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 << 20);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| e.to_string())?;
    let (width, height) = (img.width(), img.height());
    let fit = |edge: u32| {
        let scaled = if width.max(height) > edge {
            img.resize(edge, edge, image::imageops::FilterType::Triangle)
        } else {
            img.clone()
        };
        let rgba = scaled.to_rgba8();
        egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
    };
    Ok(Body::Image {
        thumb: fit(THUMB_EDGE),
        full: fit(FULL_EDGE),
        width,
        height,
    })
}

/// Reads and prepares entry `index` (runs on a worker thread).
pub fn build(archive: &dyn Archive, index: usize) -> PreviewData {
    let entry = &archive.entries()[index];
    let name = entry.path.rsplit(['/', '\\']).next().unwrap_or(&entry.path).to_owned();
    let limits = Limits {
        max_bytes: if looks_like_image(&entry.path) {
            IMAGE_LIMIT
        } else {
            TEXT_LIMIT
        },
        ..Limits::default()
    };
    let mut data = PreviewData {
        entry: index,
        name: name.clone(),
        truncated: false,
        slow: false,
        body: Body::Hex(String::new()),
    };
    let p = match read_preview(archive, index, &limits) {
        Ok(p) => p,
        Err(Error::PasswordRequired | Error::WrongPassword) => {
            data.body = Body::NeedPassword;
            return data;
        }
        Err(Error::Unsupported(m)) => {
            data.body = Body::TooFar(m);
            return data;
        }
        Err(e) => {
            data.body = Body::Failed(e.to_string());
            return data;
        }
    };
    data.truncated = p.truncated;
    data.slow = p.slow;
    data.body = match classify(&entry.path, &p.data) {
        ContentKind::Image if !p.truncated => match decode_image(&p.data) {
            Ok(b) => b,
            Err(e) => Body::Failed(e),
        },
        ContentKind::Image => Body::TooFar(format!("the image is larger than {} MiB", IMAGE_LIMIT >> 20)),
        ContentKind::Text => {
            let text =
                String::from_utf8_lossy(p.data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&p.data)).into_owned();
            Body::Text(highlight(&name, &text))
        }
        ContentKind::Binary => Body::Hex(hex_dump(&p.data)),
    };
    data
}

/// What the window keeps about previews: the selected entry, the running job, the finished result and textures.
#[derive(Default)]
pub struct PreviewPane {
    /// Entry the user wants to see.
    pub want: Option<usize>,
    running: Option<(usize, Receiver<PreviewData>)>,
    pub shown: Option<PreviewData>,
    thumb: Option<egui::TextureHandle>,
    pub full: Option<egui::TextureHandle>,
    pub full_open: bool,
}

impl PreviewPane {
    pub fn clear(&mut self) {
        *self = PreviewPane::default();
    }

    pub fn set_want(&mut self, entry: Option<usize>) {
        if self.want != entry {
            self.want = entry;
        }
    }

    /// Collects a finished job and starts the next one.
    pub fn pump(&mut self, ctx: &egui::Context, archive: Option<&Arc<dyn Archive>>) {
        if let Some((entry, rx)) = &self.running
            && let Ok(mut data) = rx.try_recv()
        {
            let entry = *entry;
            self.running = None;
            if self.want == Some(entry) {
                if let Body::Image { thumb, full, .. } = &mut data.body {
                    let thumb = std::mem::take(thumb);
                    let full = std::mem::take(full);
                    self.thumb = Some(ctx.load_texture("preview-thumb", thumb, egui::TextureOptions::LINEAR));
                    self.full = Some(ctx.load_texture("preview-full", full, egui::TextureOptions::LINEAR));
                } else {
                    self.thumb = None;
                    self.full = None;
                }
                self.shown = Some(data);
            }
        }
        let shown = self.shown.as_ref().map(|d| d.entry);
        if self.running.is_none()
            && let (Some(want), Some(archive)) = (self.want, archive)
            && shown != Some(want)
        {
            let archive = archive.clone();
            let (tx, rx) = channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(build(&*archive, want));
                ctx.request_repaint();
            });
            self.running = Some((want, rx));
        }
    }

    pub fn is_loading(&self) -> bool {
        self.running.is_some() || self.want != self.shown.as_ref().map(|d| d.entry)
    }
}

/// Draws the pane. Returns `true` when the user asks for a password.
pub fn show(pane: &mut PreviewPane, ui: &mut Ui) -> bool {
    let mut ask_password = false;
    theme::heading(ui, t(Key::PreviewTitle));
    ui.separator();
    let Some(want) = pane.want else {
        ui.label(theme::dim(t(Key::PreviewNothing)));
        return false;
    };
    let current = pane.shown.as_ref().filter(|d| d.entry == want);
    let Some(data) = current else {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new());
            ui.label(theme::dim(t(Key::PreviewLoading)));
        });
        return false;
    };
    ui.add(egui::Label::new(RichText::new(&data.name).strong()).truncate());
    if data.truncated {
        ui.label(
            RichText::new(t(Key::PreviewTruncated))
                .color(theme::PALETTE.warn)
                .size(12.0),
        );
    }
    if data.slow {
        ui.label(RichText::new(t(Key::PreviewSlow)).color(theme::PALETTE.warn).size(12.0));
    }
    ui.add_space(4.0);
    match &data.body {
        Body::Image { width, height, .. } => {
            ui.label(theme::dim(tf(
                Key::PreviewDimensions,
                &[&width.to_string(), &height.to_string()],
            )));
            if let Some(tex) = &pane.thumb {
                let avail = ui.available_width();
                let size = tex.size_vec2();
                let scale = (avail / size.x).min(1.0).min(360.0 / size.y.max(1.0)).max(0.05);
                ui.add(egui::Image::new(tex).fit_to_exact_size(size * scale));
            }
            if ui.button(t(Key::PreviewImageOpen)).clicked() {
                pane.full_open = true;
            }
        }
        Body::Text(lines) => {
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                let mut job = LayoutJob::default();
                let font = FontId::monospace(12.5);
                for line in lines {
                    for (text, [r, g, b]) in line {
                        job.append(
                            text,
                            0.0,
                            egui::TextFormat::simple(font.clone(), Color32::from_rgb(*r, *g, *b)),
                        );
                    }
                    job.append("\n", 0.0, egui::TextFormat::simple(font.clone(), Color32::WHITE));
                }
                ui.add(egui::Label::new(job).extend());
            });
        }
        Body::Hex(dump) => {
            ui.label(theme::dim(t(Key::PreviewHex)));
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(dump).monospace().size(12.0)).extend());
            });
        }
        Body::NeedPassword => {
            ui.label(RichText::new(t(Key::PreviewNeedPassword)).color(theme::PALETTE.warn));
            if ui.button(t(Key::PasswordEnter)).clicked() {
                ask_password = true;
            }
        }
        Body::TooFar(m) => {
            ui.label(RichText::new(tf(Key::PreviewTooFar, &[m])).color(theme::PALETTE.warn));
        }
        Body::Failed(m) => {
            ui.label(RichText::new(tf(Key::PreviewError, &[m])).color(theme::PALETTE.warn));
        }
    }
    ask_password
}

/// The full-size image window.
pub fn full_window(pane: &mut PreviewPane, ctx: &egui::Context) {
    if !pane.full_open {
        return;
    }
    let (Some(tex), Some(data)) = (pane.full.clone(), pane.shown.as_ref().map(|d| d.name.clone())) else {
        pane.full_open = false;
        return;
    };
    let mut open = true;
    egui::Window::new(data)
        .open(&mut open)
        .default_size([900.0, 640.0])
        .resizable(true)
        .frame(theme::card())
        .show(ctx, |ui| {
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.add(egui::Image::new(&tex).fit_to_original_size(1.0));
            });
        });
    pane.full_open = open;
}

#[cfg(test)]
mod tests {
    use super::*;
    use packrat_core::source::MemSource;
    use packrat_core::testkit::{RawEntry, raw_zip};

    fn png_bytes() -> Vec<u8> {
        let mut img = image::RgbaImage::new(40, 30);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 6) as u8, (y * 8) as u8, 128, 255]);
        }
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn archive(entries: &[RawEntry]) -> Arc<dyn Archive> {
        Arc::from(packrat_core::open::open_source(Arc::new(MemSource(raw_zip(entries))), "t.zip").unwrap())
    }

    #[test]
    fn images_text_and_binary_are_prepared() {
        let rust = "fn main() {\n    let answer = 42; // the answer\n    println!(\"{answer}\");\n}\n";
        let a = archive(&[
            RawEntry::file("pic.png", png_bytes()).deflated(),
            RawEntry::file("src/main.rs", rust),
            RawEntry::file("blob.bin", (0..=255u8).cycle().take(5000).collect::<Vec<_>>()),
            RawEntry::file("Заметки.txt", "привет, мир\nвторая строка").utf8(),
        ]);
        let p = build(&*a, 0);
        assert!(matches!(
            p.body,
            Body::Image {
                width: 40,
                height: 30,
                ..
            }
        ));

        let p = build(&*a, 1);
        match p.body {
            Body::Text(lines) => {
                assert_eq!(lines.len(), 4);
                let colors: std::collections::HashSet<[u8; 3]> = lines.iter().flatten().map(|(_, c)| *c).collect();
                assert!(colors.len() > 2, "code is highlighted with several colours: {colors:?}");
                let joined: String = lines[1].iter().map(|(s, _)| s.as_str()).collect();
                assert!(joined.contains("let answer = 42;"));
            }
            _ => panic!("expected text"),
        }

        match build(&*a, 2).body {
            Body::Hex(h) => {
                assert!(h.starts_with("00000000  00 01 02 03"), "{h}");
                assert_eq!(h.lines().count(), HEX_BYTES / 16);
            }
            _ => panic!("expected a hex dump"),
        }

        match build(&*a, 3).body {
            Body::Text(lines) => assert_eq!(lines[0][0].0, "привет, мир"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn broken_images_and_encrypted_files_report_instead_of_crashing() {
        let a = archive(&[RawEntry::file("bad.png", b"\x89PNG\r\n\x1a\nnot really a png")]);
        assert!(matches!(build(&*a, 0).body, Body::Failed(_)));
    }
}
