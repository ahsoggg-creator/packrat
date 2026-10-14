use packrat_core::codec::stream::Compression;
use packrat_core::create::{CreateOptions, Format, create_archive};
use packrat_core::extract::{ExtractOptions, Resolution, extract};
use packrat_core::open::open_source;
use packrat_core::progress::Progress;
use packrat_core::source::MemSource;
use packrat_core::testkit::{RawEntry, raw_zip};
use packrat_core::zip::ZipMethod;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

fn tree(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                out.insert(rel, None);
                walk(base, &p, out);
            } else {
                out.insert(rel, Some(fs::read(&p).unwrap()));
            }
        }
    }
    let mut m = BTreeMap::new();
    if root.exists() {
        walk(root, root, &mut m);
    }
    m
}

fn run(names: &[&str], archive_name: &str, smart: bool) -> (tempfile::TempDir, std::path::PathBuf) {
    let entries: Vec<RawEntry> = names
        .iter()
        .map(|n| {
            if n.ends_with('/') {
                RawEntry::dir(n)
            } else {
                RawEntry::file(n, format!("content of {n}"))
            }
        })
        .collect();
    let zip = raw_zip(&entries);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let a = open_source(Arc::new(MemSource(zip)), archive_name).unwrap();
    let mut opts = ExtractOptions::new(&dest, archive_name);
    opts.smart = smart;
    extract(&*a, &opts, &Progress::new()).unwrap();
    (dir, dest)
}

#[test]
fn smart_single_root_goes_straight_into_destination() {
    let (_g, dest) = run(&["App/", "App/a.txt", "App/sub/", "App/sub/b.txt"], "App.zip", true);
    let t = tree(&dest);
    assert_eq!(
        t.keys().cloned().collect::<Vec<_>>(),
        ["App", "App/a.txt", "App/sub", "App/sub/b.txt"]
    );
}

#[test]
fn smart_single_root_with_dot_slash_and_mixed_case() {
    let (_g, dest) = run(
        &["./", "./App/", "./App/a.txt", "app/b.txt", "APP/c/d.txt"],
        "whatever.zip",
        true,
    );
    let keys: Vec<_> = tree(&dest).keys().cloned().collect();
    assert_eq!(
        keys,
        ["App", "App/a.txt", "App/b.txt", "App/c", "App/c/d.txt"],
        "one folder, no duplicates by case"
    );
}

#[test]
fn smart_single_file_lands_next_to_the_archive() {
    let (_g, dest) = run(&["readme.txt"], "notes.zip", true);
    assert_eq!(tree(&dest).keys().cloned().collect::<Vec<_>>(), ["readme.txt"]);
    let (_g, dest) = run(&["./readme.txt"], "notes.zip", true);
    assert_eq!(tree(&dest).keys().cloned().collect::<Vec<_>>(), ["readme.txt"]);
}

#[test]
fn smart_loose_items_get_a_folder_named_after_the_archive() {
    let (_g, dest) = run(&["a.txt", "b.txt", "sub/c.txt"], "My Files.zip", true);
    let keys: Vec<_> = tree(&dest).keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "My Files",
            "My Files/a.txt",
            "My Files/b.txt",
            "My Files/sub",
            "My Files/sub/c.txt"
        ]
    );
    let (_g, dest) = run(&["x/a.txt", "y/b.txt"], "pair.tar.gz.zip", true);
    assert!(tree(&dest).contains_key("pair.tar.gz/x/a.txt"));
}

#[test]
fn plain_extract_keeps_the_stored_layout() {
    let (_g, dest) = run(&["a.txt", "b.txt"], "x.zip", false);
    assert_eq!(tree(&dest).keys().cloned().collect::<Vec<_>>(), ["a.txt", "b.txt"]);
}

#[test]
fn existing_files_are_never_overwritten_by_default() {
    let zip = raw_zip(&[RawEntry::file("a.txt", "new"), RawEntry::file("b.txt", "new b")]);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("d");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("a.txt"), "old").unwrap();
    let a = open_source(Arc::new(MemSource(zip)), "p.zip").unwrap();

    let mut opts = ExtractOptions::new(&dest, "p.zip");
    opts.smart = false;
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "old");
    assert_eq!(fs::read_to_string(dest.join("b.txt")).unwrap(), "new b");
    assert_eq!(report.skipped_existing.len(), 1);

    opts.default_resolution = Resolution::KeepBoth;
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "old");
    assert_eq!(fs::read_to_string(dest.join("a (2).txt")).unwrap(), "new");
    assert_eq!(report.kept_both.len(), 2);

    // a per-file choice beats the default
    opts.default_resolution = Resolution::Skip;
    opts.resolutions.insert(0, Resolution::Replace);
    extract(&*a, &opts, &Progress::new()).unwrap();
    assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "new");
}

#[test]
fn plan_lists_conflicts_before_anything_is_written() {
    let zip = raw_zip(&[RawEntry::file("a.txt", "new"), RawEntry::file("b.txt", "new b")]);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("d");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("b.txt"), "old").unwrap();
    let a = open_source(Arc::new(MemSource(zip)), "p.zip").unwrap();
    let mut opts = ExtractOptions::new(&dest, "p.zip");
    opts.smart = false;
    let plan = packrat_core::extract::plan(&*a, &opts);
    assert_eq!(plan.conflicts.len(), 1);
    assert_eq!(plan.conflicts[0].index, 1);
    assert!(!dest.join("a.txt").exists());
}

#[test]
fn hostile_names_never_escape_the_destination() {
    let hostile: Vec<&str> = vec![
        "../evil1.txt",
        "..\\evil2.txt",
        "a/../../evil3.txt",
        "/evil4.txt",
        "\\evil5.txt",
        "C:\\evil6.txt",
        "c:evil7.txt",
        "\\\\server\\share\\evil8.txt",
        "CON",
        "nul.txt",
        "good/aux.log",
        "stream.txt::$DATA",
        "file.txt:hidden",
        "trailing.",
        "trailing ",
        "bad<name>.txt",
    ];
    let mut entries: Vec<RawEntry> = hostile.iter().map(|n| RawEntry::file(n, "x")).collect();
    entries.push(RawEntry::file("ok.txt", "fine"));
    entries.push(RawEntry::file("also/ok.txt", "fine"));
    let outer = tempfile::tempdir().unwrap();
    let dest = outer.path().join("inner").join("dest");
    fs::create_dir_all(&dest).unwrap();
    let a = open_source(Arc::new(MemSource(raw_zip(&entries))), "evil.zip").unwrap();
    let mut opts = ExtractOptions::new(&dest, "evil.zip");
    opts.smart = false;
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    assert_eq!(report.blocked.len(), hostile.len(), "{:?}", report.blocked);
    assert_eq!(
        tree(&dest).keys().cloned().collect::<Vec<_>>(),
        ["also", "also/ok.txt", "ok.txt"]
    );
    // nothing appeared anywhere else
    assert_eq!(
        tree(outer.path())
            .keys()
            .filter(|k| !k.starts_with("inner/dest"))
            .cloned()
            .collect::<Vec<_>>(),
        ["inner"]
    );
}

#[test]
fn symbolic_links_in_archives_are_not_created() {
    let entries = vec![
        RawEntry::file("link", "../../outside").mode(0o120_777),
        RawEntry::file("real.txt", "ok").mode(0o100_644),
    ];
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("d");
    fs::create_dir(&dest).unwrap();
    let a = open_source(Arc::new(MemSource(raw_zip(&entries))), "l.zip").unwrap();
    let mut opts = ExtractOptions::new(&dest, "l.zip");
    opts.smart = false;
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    assert_eq!(report.blocked.len(), 1);
    assert_eq!(tree(&dest).keys().cloned().collect::<Vec<_>>(), ["real.txt"]);
}

fn sample_tree(root: &Path) {
    fs::create_dir_all(root.join("data/docs/deep")).unwrap();
    fs::create_dir_all(root.join("data/empty")).unwrap();
    fs::write(root.join("data/a.txt"), "alpha").unwrap();
    fs::write(root.join("data/zero.bin"), b"").unwrap();
    fs::write(root.join("data/docs/readme.md"), "Packrat ".repeat(20_000)).unwrap();
    let mut x = 5u64;
    let noise: Vec<u8> = (0..300_000)
        .map(|_| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (x >> 33) as u8
        })
        .collect();
    fs::write(root.join("data/docs/deep/noise.bin"), &noise).unwrap();
    fs::write(root.join("data/имя файла.txt"), "юникод").unwrap();
}

#[test]
fn create_then_extract_is_byte_identical_for_every_format() {
    let formats = [
        Format::Zip(ZipMethod::Store),
        Format::Zip(ZipMethod::Deflate),
        Format::Zip(ZipMethod::Zstd),
        Format::SevenZ,
        Format::Tar(Compression::None),
        Format::Tar(Compression::Gz),
        Format::Tar(Compression::Zst),
        Format::Tar(Compression::Xz),
        Format::Tar(Compression::Lz4),
    ];
    for format in formats {
        for threads in [1, 4] {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("src");
            sample_tree(&src);
            let archive = dir.path().join(format!("out.{}", format.extension()));
            let mut copts = CreateOptions::new(format);
            copts.threads = threads;
            let rep = create_archive(&archive, &[src.join("data")], &copts, &Progress::new()).unwrap();
            assert!(rep.entries >= 7, "{format:?}: {} entries", rep.entries);

            let a = packrat_core::open::open(&archive).unwrap();
            let dest = dir.path().join("dest");
            fs::create_dir(&dest).unwrap();
            let mut eopts = ExtractOptions::new(&dest, archive.file_name().unwrap().to_string_lossy());
            eopts.smart = false;
            eopts.threads = threads;
            let report = extract(&*a, &eopts, &Progress::new()).unwrap();
            assert!(
                report.failed.is_empty() && report.blocked.is_empty(),
                "{format:?}: {report:?}"
            );
            assert!(tree(&src) == tree(&dest), "{format:?} threads={threads}: trees differ");
        }
    }
}

#[test]
fn bare_compressed_files_open_as_single_entry_archives() {
    let payload = b"just one file ".repeat(5000);
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(&payload).unwrap();
        cases.push(("data.txt.gz", e.finish().unwrap()));
    }
    {
        let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        e.write_all(&payload).unwrap();
        cases.push(("data.txt.bz2", e.finish().unwrap()));
    }
    cases.push(("data.txt.zst", packrat_core::codec::zstd::compress_frame(&payload)));
    for (name, bytes) in cases {
        let a = open_source(Arc::new(MemSource(bytes)), name).unwrap();
        assert_eq!(a.entries().len(), 1, "{name}");
        assert_eq!(a.entries()[0].path, "data.txt");
        let dir = tempfile::tempdir().unwrap();
        let mut opts = ExtractOptions::new(dir.path(), name);
        opts.smart = true;
        extract(&*a, &opts, &Progress::new()).unwrap();
        assert_eq!(fs::read(dir.path().join("data.txt")).unwrap(), payload, "{name}");
    }
}

#[test]
fn rar_is_refused_with_a_clear_message() {
    let err = open_source(Arc::new(MemSource(b"Rar!\x1a\x07\x01\x00....".to_vec())), "x.rar")
        .err()
        .unwrap();
    assert!(err.to_string().contains("RAR"), "{err}");
}
