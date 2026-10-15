//! Hostile archives built on the fly. Whatever they contain, extraction must not write outside the destination,
//! must not fill the disk, and must ask before doing anything that looks like an archive bomb.

use packrat_core::codec::deflate;
use packrat_core::extract::{ExtractOptions, execute, extract, plan};
use packrat_core::open::open_source;
use packrat_core::safety::{Limits, Risk};
use packrat_core::source::MemSource;
use packrat_core::testkit::{RawEntry, raw_zip};
use packrat_core::{Error, Progress};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

fn tree(root: &Path) -> BTreeMap<String, u64> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, u64>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                out.insert(rel, 0);
                walk(base, &p, out);
            } else {
                out.insert(rel, e.metadata().map(|m| m.len()).unwrap_or(0));
            }
        }
    }
    let mut m = BTreeMap::new();
    walk(root, root, &mut m);
    m
}

/// A ZIP with one deflated entry of `size` zero bytes: honest sizes, tiny archive.
fn zeros_zip(name: &str, size: usize) -> Vec<u8> {
    let zeros = vec![0u8; size];
    raw_zip(&[RawEntry::file(name, zeros).deflated()])
}

fn open(bytes: Vec<u8>, name: &str) -> Box<dyn packrat_core::Archive> {
    open_source(Arc::new(MemSource(bytes)), name).unwrap()
}

#[test]
fn nothing_hostile_leaves_the_destination_or_fills_the_disk() {
    let hostile: Vec<(&str, Vec<u8>)> = vec![
        (
            "zip slip",
            raw_zip(&[
                RawEntry::file("../../escaped1.txt", "x"),
                RawEntry::file("a/../../escaped2.txt", "x"),
                RawEntry::file("..\\..\\escaped3.txt", "x"),
            ]),
        ),
        (
            "absolute and drive paths",
            raw_zip(&[
                RawEntry::file("/escaped4.txt", "x"),
                RawEntry::file("C:\\escaped5.txt", "x"),
                RawEntry::file("\\\\server\\share\\escaped6.txt", "x"),
            ]),
        ),
        (
            "reserved names and streams",
            raw_zip(&[
                RawEntry::file("NUL", "x"),
                RawEntry::file("a.txt::$DATA", "x"),
                RawEntry::file("b.", "x"),
            ]),
        ),
        (
            "symlink",
            raw_zip(&[RawEntry::file("link", "../../outside").mode(0o120_777)]),
        ),
        ("bomb", zeros_zip("zeros.bin", 80 << 20)),
        ("overlap", {
            let mut e = vec![RawEntry::file("big.bin", vec![7u8; 1 << 20])];
            for i in 0..200 {
                e.push(RawEntry::file(format!("copy{i}.bin"), vec![7u8; 1 << 20]).sharing_data_with(0));
            }
            raw_zip(&e)
        }),
    ];

    let mut limits = Limits::default();
    limits.ratio_min_size = 8 << 20;
    limits.max_ratio = 100.0;
    limits.total_ratio_min_size = 64 << 20;
    limits.max_total_ratio = 100.0;

    for (name, bytes) in hostile {
        let outer = tempfile::tempdir().unwrap();
        let dest = outer.path().join("inner").join("dest");
        fs::create_dir_all(&dest).unwrap();
        let a = open(bytes, "evil.zip");
        let mut opts = ExtractOptions::new(&dest, "evil.zip");
        opts.smart = false;
        opts.limits = limits.clone();
        opts.archive_len = 300_000;
        // default behaviour: no acceptance of risks
        let _ = extract(&*a, &opts, &Progress::new());
        let outside: Vec<String> = tree(outer.path())
            .keys()
            .filter(|k| *k != "inner" && !k.starts_with("inner/dest"))
            .cloned()
            .collect();
        assert!(outside.is_empty(), "{name}: wrote outside the destination: {outside:?}");
        let written: u64 = tree(&dest).values().sum();
        assert!(written < (4 << 20), "{name}: wrote {written} bytes without being asked");
    }
}

#[test]
fn a_huge_ratio_entry_needs_confirmation_and_nothing_is_written_before() {
    let a = open(zeros_zip("zeros.bin", 80 << 20), "bomb.zip");
    let dir = tempfile::tempdir().unwrap();
    let mut opts = ExtractOptions::new(dir.path(), "bomb.zip");
    opts.smart = false;
    opts.limits.ratio_min_size = 8 << 20;
    opts.limits.max_ratio = 100.0;
    let p = plan(&*a, &opts);
    assert!(
        p.risks.iter().any(|r| matches!(r, Risk::HighRatio { .. })),
        "{:?}",
        p.risks
    );

    let err = execute(&*a, &p, &opts, &Progress::new()).unwrap_err();
    assert!(matches!(err, Error::Blocked(_)), "{err}");
    assert!(
        tree(dir.path()).is_empty(),
        "nothing may be written before confirmation"
    );

    opts.accept_risks = true;
    let report = execute(&*a, &p, &opts, &Progress::new()).unwrap();
    assert_eq!(report.bytes, 80 << 20, "continuing at your own risk extracts it");
}

#[test]
fn overlapping_entries_are_detected() {
    let mut e = vec![RawEntry::file("big.bin", vec![1u8; 100_000])];
    for i in 0..20 {
        e.push(RawEntry::file(format!("c{i}"), vec![1u8; 100_000]).sharing_data_with(0));
    }
    let a = open(raw_zip(&e), "q.zip");
    assert_eq!(a.overlapping_entries(), 20);
    let dir = tempfile::tempdir().unwrap();
    let opts = ExtractOptions::new(dir.path(), "q.zip");
    assert!(
        plan(&*a, &opts)
            .risks
            .iter()
            .any(|r| matches!(r, Risk::OverlappingEntries { count: 20 }))
    );

    let clean = open(raw_zip(&[RawEntry::file("a", "1"), RawEntry::file("b", "2")]), "ok.zip");
    assert_eq!(clean.overlapping_entries(), 0);
}

#[test]
fn deeply_nested_archives_are_reported() {
    let mut data = raw_zip(&[RawEntry::file("payload.txt", "deep")]);
    for level in 0..6 {
        data = raw_zip(&[RawEntry::file(format!("level{level}.zip"), data)]);
    }
    let a = open(data, "matryoshka.zip");
    let dir = tempfile::tempdir().unwrap();
    let opts = ExtractOptions::new(dir.path(), "matryoshka.zip");
    let p = plan(&*a, &opts);
    assert!(
        p.risks.iter().any(|r| matches!(r, Risk::DeepNesting { limit: 3, .. })),
        "{:?}",
        p.risks
    );

    // two levels are normal
    let shallow = raw_zip(&[RawEntry::file("inner.zip", raw_zip(&[RawEntry::file("x", "1")]))]);
    let a = open(shallow, "s.zip");
    assert!(plan(&*a, &opts).risks.is_empty());
}

#[test]
fn entry_count_total_size_and_free_space_limits() {
    let many: Vec<RawEntry> = (0..50).map(|i| RawEntry::file(format!("f{i}"), "x")).collect();
    let a = open(raw_zip(&many), "many.zip");
    let dir = tempfile::tempdir().unwrap();
    let mut opts = ExtractOptions::new(dir.path(), "many.zip");
    opts.limits.max_entries = 10;
    assert!(
        plan(&*a, &opts)
            .risks
            .iter()
            .any(|r| matches!(r, Risk::TooManyEntries { count: 50, limit: 10 }))
    );

    let mut opts = ExtractOptions::new(dir.path(), "many.zip");
    opts.limits.max_total_bytes = 10;
    assert!(
        plan(&*a, &opts)
            .risks
            .iter()
            .any(|r| matches!(r, Risk::TotalTooLarge { .. }))
    );

    // "needs more space than the disk has": ask for an absurd safety margin
    let mut opts = ExtractOptions::new(dir.path(), "many.zip");
    opts.limits.free_space_margin = 1 << 62;
    let p = plan(&*a, &opts);
    if packrat_core::safety::free_space(dir.path()).is_some() {
        assert!(
            p.risks.iter().any(|r| matches!(r, Risk::NotEnoughSpace { .. })),
            "{:?}",
            p.risks
        );
    }
}

/// 200 MiB of zeros as a single .gz; `trailer_size` is what the gzip trailer claims (a lying or wrapped value hides
/// the real size from the pre-check).
fn gzip_bomb(trailer_size: u32) -> Vec<u8> {
    let mut packed = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 11];
    let zeros = vec![0u8; 1 << 20];
    let mut crc = crc32fast::Hasher::new();
    for _ in 0..200 {
        packed.extend(deflate::compress_block(&zeros, 6, false));
        crc.update(&zeros);
    }
    packed.extend([1, 0, 0, 0xff, 0xff]);
    packed.extend(crc.finalize().to_le_bytes());
    packed.extend(trailer_size.to_le_bytes());
    packed
}

fn bomb_options(dir: &Path, archive_len: u64) -> ExtractOptions {
    let mut opts = ExtractOptions::new(dir, "zeros.bin.gz");
    opts.archive_len = archive_len;
    opts.limits.max_total_ratio = 100.0;
    opts.limits.total_ratio_min_size = 8 << 20;
    opts.limits.ratio_min_size = u64::MAX; // the per-entry check is not what is tested here
    opts
}

#[test]
fn a_gzip_bomb_with_an_honest_size_is_flagged_before_unpacking() {
    let packed = gzip_bomb(200 << 20);
    let len = packed.len() as u64;
    assert!(len < 400_000, "the bomb is small: {len} bytes");
    let a = open(packed, "zeros.bin.gz");
    let dir = tempfile::tempdir().unwrap();
    let p = plan(&*a, &bomb_options(dir.path(), len));
    assert!(
        p.risks.iter().any(|r| matches!(r, Risk::HighTotalRatio { .. })),
        "{:?}",
        p.risks
    );
    assert!(execute(&*a, &p, &bomb_options(dir.path(), len), &Progress::new()).is_err());
    assert!(tree(dir.path()).is_empty());
}

#[test]
fn a_gzip_bomb_that_hides_its_size_is_stopped_while_unpacking() {
    let packed = gzip_bomb(0);
    let len = packed.len() as u64;
    let a = open(packed, "zeros.bin.gz");
    let dir = tempfile::tempdir().unwrap();
    let opts = bomb_options(dir.path(), len);
    assert!(plan(&*a, &opts).risks.is_empty(), "nothing is known in advance");
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    assert!(report.stopped.is_some(), "{report:?}");
    assert_eq!(tree(dir.path()).values().sum::<u64>(), 0, "the partial file is removed");
}

#[test]
fn ordinary_archives_raise_no_warnings() {
    let a = open(
        raw_zip(&[
            RawEntry::dir("docs/"),
            RawEntry::file("docs/a.txt", "hello world ".repeat(1000)).deflated(),
            RawEntry::file("b.bin", vec![9u8; 5000]),
        ]),
        "fine.zip",
    );
    let dir = tempfile::tempdir().unwrap();
    let opts = ExtractOptions::new(dir.path(), "fine.zip");
    assert!(plan(&*a, &opts).risks.is_empty());
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    assert_eq!(report.files, 2);
}
