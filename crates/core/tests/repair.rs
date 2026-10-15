use packrat_core::create::{CreateOptions, Format, create_archive};
use packrat_core::extract::{ExtractOptions, extract};
use packrat_core::input::collect_inputs;
use packrat_core::open::{open, open_with};
use packrat_core::source::MemSource;
use packrat_core::verify::verify;
use packrat_core::zip::{ZipMethod, ZipOptions, write_zip};
use packrat_core::{OpenOptions, Progress};
use std::fs;
use std::path::Path;
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

fn sample(root: &Path, files: usize) -> std::path::PathBuf {
    let src = root.join("docs");
    fs::create_dir_all(&src).unwrap();
    for i in 0..files {
        // half compressible, half noise, so entries differ in how they deflate
        let body = if i % 2 == 0 {
            format!("file {i} ").repeat(8000).into_bytes()
        } else {
            noise(60_000, i as u64 + 1)
        };
        fs::write(src.join(format!("f{i:02}.dat")), body).unwrap();
    }
    src
}

fn expected(src: &Path, i: usize) -> Vec<u8> {
    fs::read(src.join(format!("f{i:02}.dat"))).unwrap()
}

#[test]
fn verify_passes_clean_archives_and_finds_damage_in_every_format() {
    for format in [
        Format::Zip(ZipMethod::Deflate),
        Format::Zip(ZipMethod::Store),
        Format::SevenZ,
        Format::Tar(packrat_core::codec::stream::Compression::Gz),
        Format::Tar(packrat_core::codec::stream::Compression::Zst),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let src = sample(dir.path(), 6);
        let path = dir.path().join(format!("a.{}", format.extension()));
        create_archive(&path, &[src], &CreateOptions::new(format), &Progress::new()).unwrap();

        let a = open(&path).unwrap();
        let clean = verify(&*a, None, 3, &Progress::new());
        assert!(clean.is_clean(), "{format:?}: {clean:?}");
        assert_eq!(clean.ok, 6);

        let mut bytes = fs::read(&path).unwrap();
        let at = bytes.len() / 3;
        for b in &mut bytes[at..at + 40] {
            *b ^= 0xA5;
        }
        let bad = dir.path().join(format!("bad.{}", format.extension()));
        fs::write(&bad, bytes).unwrap();
        // if the damage hit the headers, refusing to open is also a detection
        if let Ok(a) = open(&bad) {
            let r = verify(&*a, None, 3, &Progress::new());
            assert!(!r.is_clean(), "{format:?}: damage went unnoticed");
        }
    }
}

#[test]
fn a_truncated_zip_is_recovered_up_to_the_cut() {
    let dir = tempfile::tempdir().unwrap();
    let src = sample(dir.path(), 10);
    let path = dir.path().join("whole.zip");
    create_archive(
        &path,
        std::slice::from_ref(&src),
        &CreateOptions::new(Format::Zip(ZipMethod::Deflate)),
        &Progress::new(),
    )
    .unwrap();
    let bytes = fs::read(&path).unwrap();
    let cut = dir.path().join("cut.zip");
    fs::write(&cut, &bytes[..bytes.len() * 6 / 10]).unwrap();

    assert!(open(&cut).is_err(), "a cut ZIP has no central directory");
    let a = open_with(&cut, &OpenOptions::default().with_recover()).unwrap();
    assert!(a.is_recovered());
    let files = a.entries().iter().filter(|e| e.is_file()).count();
    assert!((3..10).contains(&files), "recovered {files} of 10 files");

    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    let mut opts = ExtractOptions::new(&out, "cut.zip");
    opts.smart = false;
    opts.keep_partial = true;
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    let mut whole = 0;
    for i in 0..10 {
        let p = out.join("docs").join(format!("f{i:02}.dat"));
        if p.exists() {
            assert_eq!(fs::read(&p).unwrap(), expected(&src, i), "f{i:02} must be intact");
            whole += 1;
        }
    }
    assert!(whole >= 3, "{whole} intact files");
    // the entry that was cut in half is kept as a partial file, never presented as a whole one
    assert!(report.failed.len() <= 1, "{report:?}");
    if let Some(f) = report.failed.first() {
        assert!(f.message.contains("partial"), "{}", f.message);
    }
}

#[test]
fn a_zip_with_a_destroyed_central_directory_is_recovered_completely() {
    let dir = tempfile::tempdir().unwrap();
    let src = sample(dir.path(), 8);
    let path = dir.path().join("a.zip");
    create_archive(
        &path,
        std::slice::from_ref(&src),
        &CreateOptions::new(Format::Zip(ZipMethod::Deflate)),
        &Progress::new(),
    )
    .unwrap();
    let mut bytes = fs::read(&path).unwrap();
    let n = bytes.len();
    for b in &mut bytes[n - 22..] {
        *b = 0; // the end-of-central-directory record is gone
    }
    let broken = dir.path().join("broken.zip");
    fs::write(&broken, bytes).unwrap();
    assert!(open(&broken).is_err());

    let a = open_with(&broken, &OpenOptions::default().with_recover()).unwrap();
    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    let mut opts = ExtractOptions::new(&out, "broken.zip");
    opts.smart = false;
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    assert!(report.failed.is_empty(), "{report:?}");
    for i in 0..8 {
        assert_eq!(
            fs::read(out.join("docs").join(format!("f{i:02}.dat"))).unwrap(),
            expected(&src, i)
        );
    }
}

#[test]
fn streamed_entries_with_data_descriptors_are_recovered() {
    // 600 KB cut into 64 KB blocks: written with a data descriptor, sizes unknown in the local header
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big");
    fs::create_dir_all(&src).unwrap();
    let big = noise(600_000, 5).iter().map(|b| b % 40).collect::<Vec<u8>>();
    fs::write(src.join("big.bin"), &big).unwrap();
    fs::write(src.join("tail.txt"), "after the big one").unwrap();
    let (items, _) = collect_inputs(std::slice::from_ref(&src)).unwrap();
    let mut buf = Vec::new();
    write_zip(
        &mut buf,
        &items,
        &ZipOptions {
            block_size: 64 << 10,
            threads: 2,
            ..Default::default()
        },
        &Progress::new(),
    )
    .unwrap();
    let cd = buf.windows(4).position(|w| w == [b'P', b'K', 1, 2]).unwrap();

    let a = packrat_core::open::open_source_with(
        Arc::new(MemSource(buf[..cd].to_vec())),
        "x.zip",
        &OpenOptions::default().with_recover(),
    )
    .unwrap();
    assert!(a.is_recovered());
    let dest = tempfile::tempdir().unwrap();
    let mut opts = ExtractOptions::new(dest.path(), "x.zip");
    opts.smart = false;
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(fs::read(dest.path().join("big/big.bin")).unwrap(), big);
    assert_eq!(
        fs::read_to_string(dest.path().join("big/tail.txt")).unwrap(),
        "after the big one"
    );
}

#[test]
fn one_damaged_entry_does_not_stop_the_rest_and_can_leave_a_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = sample(dir.path(), 6);
    let path = dir.path().join("a.zip");
    create_archive(
        &path,
        std::slice::from_ref(&src),
        &CreateOptions::new(Format::Zip(ZipMethod::Deflate)),
        &Progress::new(),
    )
    .unwrap();
    let a = open(&path).unwrap();
    // damage the middle of the data of f01 (a noise file, stored almost as is)
    let e = a.entries().iter().find(|e| e.path == "docs/f01.dat").unwrap().clone();
    let mut bytes = fs::read(&path).unwrap();
    let local = bytes
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == [b'P', b'K', 3, 4])
        .map(|(i, _)| i)
        .nth(e.index)
        .unwrap();
    let mid = local + 30 + e.raw_name.len() + (e.packed.unwrap() / 2) as usize;
    for b in &mut bytes[mid..mid + 16] {
        *b ^= 0xFF;
    }
    let bad = dir.path().join("bad.zip");
    fs::write(&bad, bytes).unwrap();

    let a = open(&bad).unwrap();
    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();
    let mut opts = ExtractOptions::new(&out, "bad.zip");
    opts.smart = false;
    opts.keep_partial = true;
    let report = extract(&*a, &opts, &Progress::new()).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(
        !out.join("docs/f01.dat").exists(),
        "a damaged file is never left under its real name"
    );
    assert!(
        fs::read_dir(out.join("docs"))
            .unwrap()
            .any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".partial"))
    );
    for i in [0, 2, 3, 4, 5] {
        assert_eq!(
            fs::read(out.join("docs").join(format!("f{i:02}.dat"))).unwrap(),
            expected(&src, i)
        );
    }
}
