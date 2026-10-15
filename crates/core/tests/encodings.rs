use packrat_core::encoding::NameEncoding;
use packrat_core::extract::{ExtractOptions, extract};
use packrat_core::open::{open_source, open_source_with};
use packrat_core::source::MemSource;
use packrat_core::testkit::{RawEntry, raw_zip};
use packrat_core::{OpenOptions, Progress};
use std::sync::Arc;

const NAMES: [&str; 4] = [
    "Документы/",
    "Документы/Отчёт за 2024 год.txt",
    "Документы/Договор №5.doc",
    "Привет мир.txt",
];

fn legacy_zip(enc: &'static encoding_rs::Encoding) -> Vec<u8> {
    let entries: Vec<RawEntry> = NAMES
        .iter()
        .map(|n| {
            let raw = enc.encode(n).0.into_owned();
            if n.ends_with('/') {
                RawEntry::dir(raw)
            } else {
                RawEntry::file(raw, format!("текст {n}"))
            }
        })
        .collect();
    raw_zip(&entries)
}

fn paths(bytes: Vec<u8>, opts: &OpenOptions) -> Vec<String> {
    let a = open_source_with(Arc::new(MemSource(bytes)), "old.zip", opts).unwrap();
    a.entries().iter().map(|e| e.path.clone()).collect()
}

#[test]
fn cp866_and_cp1251_names_are_detected_automatically() {
    for enc in [encoding_rs::IBM866, encoding_rs::WINDOWS_1251] {
        let got = paths(legacy_zip(enc), &OpenOptions::default());
        assert_eq!(got, NAMES, "{}", enc.name());
    }
    let a = open_source(Arc::new(MemSource(legacy_zip(encoding_rs::IBM866))), "old.zip").unwrap();
    let legacy = a.legacy_names().expect("legacy names are reported");
    assert_eq!(legacy.current, NameEncoding::Cp866);
    assert_eq!(legacy.indices.len(), NAMES.len());
}

#[test]
fn a_manual_choice_overrides_detection_and_previews_show_the_effect() {
    let bytes = legacy_zip(encoding_rs::IBM866);
    let wrong = paths(
        bytes.clone(),
        &OpenOptions::default().with_encoding(NameEncoding::Win1251),
    );
    assert_ne!(wrong, NAMES, "forcing the wrong code page shows mojibake");
    let right = paths(
        bytes.clone(),
        &OpenOptions::default().with_encoding(NameEncoding::Cp866),
    );
    assert_eq!(right, NAMES);

    let a = open_source_with(
        Arc::new(MemSource(bytes)),
        "x.zip",
        &OpenOptions::default().with_encoding(NameEncoding::Win1251),
    )
    .unwrap();
    let legacy = a.legacy_names().unwrap();
    assert_eq!(legacy.current, NameEncoding::Win1251);
    assert_eq!(
        legacy.detection.best,
        NameEncoding::Cp866,
        "detection is reported separately from the choice"
    );
    assert_eq!(
        legacy.preview(NameEncoding::Cp866, 2),
        ["Документы/", "Документы/Отчёт за 2024 год.txt"]
    );
}

#[test]
fn utf8_flag_unicode_path_and_plain_utf8_are_never_guessed() {
    let flagged = raw_zip(&[RawEntry::file("Привет.txt", "x").utf8()]);
    let a = open_source(Arc::new(MemSource(flagged)), "a.zip").unwrap();
    assert_eq!(a.entries()[0].path, "Привет.txt");
    assert!(a.legacy_names().is_none());

    // valid UTF-8 without the flag (Linux archivers) is read as UTF-8
    let unflagged = raw_zip(&[RawEntry::file("Привет.txt", "x")]);
    let a = open_source(Arc::new(MemSource(unflagged)), "a.zip").unwrap();
    assert_eq!(a.entries()[0].path, "Привет.txt");
    assert!(a.legacy_names().is_none());

    // Info-ZIP Unicode Path field wins over the legacy header name
    let cp866_name = encoding_rs::IBM866.encode("Привет.txt").0.into_owned();
    let mut extra = vec![0x75, 0x70];
    let utf8 = "Привет-юникод.txt".as_bytes();
    extra.extend(((1 + 4 + utf8.len()) as u16).to_le_bytes());
    extra.push(1);
    extra.extend(crc32fast::hash(&cp866_name).to_le_bytes());
    extra.extend(utf8);
    let mut e = RawEntry::file(cp866_name, "x");
    e.extra = extra;
    let a = open_source(Arc::new(MemSource(raw_zip(&[e]))), "a.zip").unwrap();
    assert_eq!(a.entries()[0].path, "Привет-юникод.txt");
    assert!(a.legacy_names().is_none());
}

#[test]
fn extraction_creates_the_correct_russian_names_on_disk() {
    let a = open_source(Arc::new(MemSource(legacy_zip(encoding_rs::WINDOWS_1251))), "старый.zip").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut opts = ExtractOptions::new(dir.path(), "старый.zip");
    opts.smart = false;
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    assert!(report.failed.is_empty() && report.blocked.is_empty(), "{report:?}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Документы").join("Договор №5.doc")).unwrap(),
        "текст Документы/Договор №5.doc"
    );
    assert!(dir.path().join("Привет мир.txt").exists());
}
