use packrat_core::archive::Archive;
use packrat_core::input::collect_inputs;
use packrat_core::progress::Progress;
use packrat_core::source::MemSource;
use packrat_core::zip::{ZipArchive, ZipMethod, ZipOptions, write_zip};
use std::fs;
use std::io::Read;
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

fn text(n: usize) -> Vec<u8> {
    "Packrat stores things for later. ".bytes().cycle().take(n).collect()
}

fn make_tree(root: &Path) {
    fs::create_dir_all(root.join("docs/deep/er")).unwrap();
    fs::create_dir_all(root.join("empty")).unwrap();
    fs::write(root.join("small.txt"), b"hello").unwrap();
    fs::write(root.join("zero.bin"), b"").unwrap();
    fs::write(root.join("docs/readme.md"), text(10_000)).unwrap();
    fs::write(root.join("docs/deep/er/random.bin"), noise(150_000, 7)).unwrap();
    fs::write(root.join("big-text.txt"), text(700_000)).unwrap();
    fs::write(root.join("big-noise.bin"), noise(500_000, 42)).unwrap();
    fs::write(root.join("имя файла.txt"), "юникод".as_bytes()).unwrap();
}

fn read_all(a: &ZipArchive, i: usize) -> Vec<u8> {
    let mut v = Vec::new();
    a.open_entry(i).unwrap().read_to_end(&mut v).unwrap();
    v
}

#[test]
fn roundtrip_all_methods_and_thread_counts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    make_tree(&root);
    let (items, skipped) = collect_inputs(std::slice::from_ref(&root)).unwrap();
    assert!(skipped.is_empty());

    for method in [ZipMethod::Store, ZipMethod::Deflate, ZipMethod::Zstd] {
        for threads in [1, 4] {
            let opts = ZipOptions {
                method,
                level: 6,
                threads,
                block_size: 64 << 10,
            };
            let mut buf = Vec::new();
            let stats = write_zip(&mut buf, &items, &opts, &Progress::new()).unwrap();
            assert_eq!(stats.entries, items.len());

            let zip = ZipArchive::from_source(Arc::new(MemSource(buf))).unwrap();
            assert_eq!(zip.entries().len(), items.len());
            for item in &items {
                let e = zip
                    .entries()
                    .iter()
                    .find(|e| e.path == item.name)
                    .unwrap_or_else(|| panic!("{} missing", item.name));
                if item.is_dir {
                    assert!(e.is_dir(), "{}", item.name);
                } else {
                    let want = fs::read(&item.disk_path).unwrap();
                    let got = read_all(&zip, e.index);
                    assert_eq!(got, want, "{} via {method:?}/{threads}", item.name);
                    assert_eq!(e.size, want.len() as u64);
                }
            }
        }
    }
}

#[test]
fn corrupted_data_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, text(5000)).unwrap();
    let (items, _) = collect_inputs(&[f]).unwrap();
    let mut buf = Vec::new();
    write_zip(
        &mut buf,
        &items,
        &ZipOptions {
            method: ZipMethod::Store,
            ..Default::default()
        },
        &Progress::new(),
    )
    .unwrap();
    buf[60] ^= 0xFF; // inside the stored data
    let zip = ZipArchive::from_source(Arc::new(MemSource(buf))).unwrap();
    let mut v = Vec::new();
    let err = zip.open_entry(0).unwrap().read_to_end(&mut v).unwrap_err();
    assert!(err.to_string().contains("CRC"), "{err}");
}

#[test]
fn modified_times_survive() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, b"x").unwrap();
    let (items, _) = collect_inputs(&[f]).unwrap();
    let mut buf = Vec::new();
    write_zip(&mut buf, &items, &ZipOptions::default(), &Progress::new()).unwrap();
    let zip = ZipArchive::from_source(Arc::new(MemSource(buf))).unwrap();
    assert_eq!(zip.entries()[0].mtime, items[0].mtime);
}
