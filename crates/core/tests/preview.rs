use packrat_core::codec::stream::Compression;
use packrat_core::create::{CreateOptions, Format, create_archive};
use packrat_core::open::{open, open_source};
use packrat_core::preview::{Limits, read_preview};
use packrat_core::source::{CountingSource, MemSource, SharedSource};
use packrat_core::testkit::{RawEntry, raw_zip};
use packrat_core::{Error, Progress};
use std::fs;
use std::sync::Arc;

fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (x >> 33) as u8
        })
        .collect()
}

/// The acceptance test: previewing one file of a large ZIP must not read the whole archive.
#[test]
fn previewing_one_zip_entry_reads_a_tiny_part_of_the_archive() {
    for deflated in [false, true] {
        let entries: Vec<RawEntry> = (0..80)
            .map(|i| {
                let e = RawEntry::file(format!("dir/file{i:02}.bin"), noise(1 << 20, i as u64 + 1));
                if deflated { e.deflated() } else { e }
            })
            .collect();
        let bytes = raw_zip(&entries);
        let total = bytes.len() as u64;
        assert!(total > 80 << 20);

        let counting = Arc::new(CountingSource::new(MemSource(bytes)));
        let shared: SharedSource = counting.clone();
        let archive = open_source(shared, "big.zip").unwrap();
        let after_open = counting.bytes_read();
        assert!(after_open < 512 << 10, "opening read {after_open} bytes of {total}");

        counting.reset();
        let index = archive
            .entries()
            .iter()
            .position(|e| e.path == "dir/file53.bin")
            .unwrap();
        let p = read_preview(
            &*archive,
            index,
            &Limits {
                max_bytes: 64 << 10,
                ..Limits::default()
            },
        )
        .unwrap();
        assert_eq!(p.data.len(), 64 << 10);
        assert!(p.truncated && !p.slow);
        let read = counting.bytes_read();
        assert!(
            read < 512 << 10,
            "preview read {read} bytes of {total} (deflated={deflated})"
        );
        assert!(read * 100 < total, "preview must be far below 1% of the archive");
        assert_eq!(
            &p.data[..],
            &noise(1 << 20, 54)[..64 << 10],
            "the bytes are the right ones (deflated={deflated})"
        );
    }
}

#[test]
fn small_files_come_back_whole_and_untruncated() {
    let zip = raw_zip(&[RawEntry::file("note.txt", "short text").deflated()]);
    let a = open_source(Arc::new(MemSource(zip)), "n.zip").unwrap();
    let p = read_preview(&*a, 0, &Limits::default()).unwrap();
    assert_eq!(p.data, b"short text");
    assert!(!p.truncated);
}

fn tree_with_files(root: &std::path::Path) -> std::path::PathBuf {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    for i in 0..6 {
        fs::write(src.join(format!("f{i}.bin")), noise(200_000, 100 + i)).unwrap();
    }
    src
}

#[test]
fn streaming_archives_preview_a_later_entry_and_are_marked_slow() {
    let dir = tempfile::tempdir().unwrap();
    let src = tree_with_files(dir.path());
    for format in [
        Format::Tar(Compression::Zst),
        Format::Tar(Compression::Gz),
        Format::SevenZ,
    ] {
        let out = dir.path().join(format!("a.{}", format.extension()));
        create_archive(
            &out,
            std::slice::from_ref(&src),
            &CreateOptions::new(format),
            &Progress::new(),
        )
        .unwrap();
        let a = open(&out).unwrap();
        let index = a.entries().iter().position(|e| e.path == "src/f4.bin").unwrap();
        let p = read_preview(
            &*a,
            index,
            &Limits {
                max_bytes: 1000,
                ..Limits::default()
            },
        )
        .unwrap();
        assert_eq!(&p.data[..], &noise(200_000, 104)[..1000], "{format:?}");
        assert!(
            p.slow && p.truncated,
            "{format:?}: slow={} truncated={}",
            p.slow,
            p.truncated
        );

        // a limit on how much may be decoded first
        let err = read_preview(
            &*a,
            index,
            &Limits {
                max_bytes: 1000,
                max_skip: 1000,
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)), "{format:?}: {err}");
        // the first file needs no skipping
        let first = a.entries().iter().position(|e| e.path == "src/f0.bin").unwrap();
        assert!(
            read_preview(
                &*a,
                first,
                &Limits {
                    max_bytes: 10,
                    max_skip: 0
                }
            )
            .is_ok()
        );
    }
}

#[test]
fn folders_cannot_be_previewed() {
    let zip = raw_zip(&[RawEntry::dir("d/")]);
    let a = open_source(Arc::new(MemSource(zip)), "d.zip").unwrap();
    assert!(read_preview(&*a, 0, &Limits::default()).is_err());
}
