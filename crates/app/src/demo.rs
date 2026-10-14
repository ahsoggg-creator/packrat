//! Made-up data for `--demo` and `--screenshots`: an archive that exists only in memory, so nothing real is
//! read or written and the screenshots contain no personal file names.

use packrat_core::Archive;
use packrat_core::source::MemSource;
use packrat_core::testkit::{RawEntry, raw_zip};
use packrat_core::zip::ZipArchive;
use std::sync::Arc;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn noise(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }

    fn text(&mut self, words: usize) -> Vec<u8> {
        const W: [&str; 24] = [
            "заря",
            "проект",
            "рендер",
            "карта",
            "слой",
            "звук",
            "сцена",
            "кадр",
            "ключ",
            "поток",
            "буфер",
            "узел",
            "тень",
            "свет",
            "шрифт",
            "текстура",
            "level",
            "shader",
            "mesh",
            "buffer",
            "config",
            "player",
            "world",
            "event",
        ];
        let mut s = String::new();
        for i in 0..words {
            s.push_str(W[self.next() as usize % W.len()]);
            s.push(if i % 11 == 10 { '\n' } else { ' ' });
        }
        s.into_bytes()
    }
}

pub fn recent_files() -> Vec<String> {
    vec![
        r"D:\Проекты\Проект-Заря.zip".into(),
        r"D:\Фото\Отпуск-2024.7z".into(),
        r"E:\Данные\dataset.tar.zst".into(),
        r"C:\Загрузки\drivers.zip".into(),
    ]
}

/// The demo archive, its file name and its (pretend) size on disk.
pub fn archive() -> (Arc<dyn Archive>, String, u64) {
    let mut r = Lcg(2024);
    let mut e: Vec<RawEntry> = Vec::new();
    let d = |name: &str| RawEntry::dir(name).utf8().dated(2025, 9, 3, 11, 20);
    let text = |name: &str, words: usize, r: &mut Lcg, date: (u16, u16, u16, u16, u16)| {
        RawEntry::file(name, r.text(words))
            .utf8()
            .deflated()
            .dated(date.0, date.1, date.2, date.3, date.4)
    };
    e.push(d("Заря/"));
    e.push(text("Заря/README.md", 900, &mut r, (2025, 9, 3, 11, 20)));
    e.push(text("Заря/LICENSE", 180, &mut r, (2025, 1, 12, 9, 5)));
    e.push(text("Заря/Cargo.toml", 120, &mut r, (2025, 9, 1, 18, 42)));
    e.push(d("Заря/src/"));
    for (i, n) in ["main.rs", "lib.rs", "config.rs", "world.rs", "camera.rs", "input.rs"]
        .iter()
        .enumerate()
    {
        e.push(text(
            &format!("Заря/src/{n}"),
            1200 + i * 700,
            &mut r,
            (2025, 8, 20 + i as u16, 14, 10 + i as u16),
        ));
    }
    e.push(d("Заря/src/render/"));
    for (i, n) in ["mod.rs", "mesh.rs", "shader.rs", "pipeline.rs"].iter().enumerate() {
        e.push(text(
            &format!("Заря/src/render/{n}"),
            2000 + i * 900,
            &mut r,
            (2025, 8, 29, 10 + i as u16, 33),
        ));
    }
    e.push(d("Заря/assets/"));
    e.push(d("Заря/assets/textures/"));
    for (i, n) in ["grass.png", "stone.png", "water.png", "sky.png"].iter().enumerate() {
        e.push(
            RawEntry::file(format!("Заря/assets/textures/{n}"), r.noise(38_000 + i * 9_000))
                .utf8()
                .deflated()
                .dated(2025, 7, 11, 16, 2),
        );
    }
    e.push(d("Заря/assets/audio/"));
    e.push(
        RawEntry::file("Заря/assets/audio/theme.ogg", r.noise(420_000))
            .utf8()
            .deflated()
            .dated(2025, 6, 2, 21, 44),
    );
    e.push(
        RawEntry::file("Заря/assets/audio/steps.ogg", r.noise(96_000))
            .utf8()
            .deflated()
            .dated(2025, 6, 2, 21, 47),
    );
    e.push(d("Заря/docs/"));
    e.push(text("Заря/docs/guide.md", 3500, &mut r, (2025, 9, 2, 12, 0)));
    e.push(text("Заря/docs/changelog.md", 1500, &mut r, (2025, 9, 3, 10, 55)));
    e.push(d("Заря/отчёты/"));
    for m in 1..=4u16 {
        e.push(text(
            &format!("Заря/отчёты/2025-{m:02}.txt"),
            800 + m as usize * 120,
            &mut r,
            (2025, m, 28, 17, 30),
        ));
    }
    let bytes = raw_zip(&e);
    let len = (bytes.len() as f64 * 0.62) as u64; // the real file would be smaller than this stored-with-headers build
    let zip = ZipArchive::from_source(Arc::new(MemSource(bytes))).expect("demo archive is valid");
    (Arc::new(zip), "Проект-Заря.zip".into(), len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_archive_is_a_valid_single_root_zip() {
        let (a, name, _) = archive();
        assert_eq!(name, "Проект-Заря.zip");
        assert!(a.entries().len() > 30);
        assert!(a.entries().iter().all(|e| e.path.starts_with("Заря/")));
        // contents are readable and deflated
        let e = a.entries().iter().find(|e| e.path.ends_with("README.md")).unwrap();
        assert!(e.packed.unwrap() < e.size);
        let mut v = Vec::new();
        std::io::Read::read_to_end(&mut a.open_entry(e.index).unwrap(), &mut v).unwrap();
        assert_eq!(v.len() as u64, e.size);
    }
}
