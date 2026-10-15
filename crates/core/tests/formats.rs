//! AR, CPIO, CAB, LZH, RAR and ISO, from archives assembled byte by byte (the layouts were also checked against
//! 7-Zip, bsdtar and makecab while writing the readers).

use packrat_core::extract::{ExtractOptions, extract};
use packrat_core::open::{open, open_source};
use packrat_core::source::MemSource;
use packrat_core::{Archive, Error, Kind, Progress};
use std::fs;
use std::io::Read;
use std::sync::Arc;

const HELLO: &[u8] = b"hello\n";

fn big() -> Vec<u8> {
    b"second file\n".repeat(400)
}

fn open_bytes(bytes: Vec<u8>, name: &str) -> Box<dyn Archive> {
    open_source(Arc::new(MemSource(bytes)), name).unwrap()
}

fn read(a: &dyn Archive, path: &str) -> Vec<u8> {
    let e = a.entries().iter().find(|e| e.path == path).unwrap_or_else(|| panic!("no {path}"));
    let mut v = Vec::new();
    a.open_entry(e.index).unwrap().read_to_end(&mut v).unwrap();
    v
}

fn check_both(a: &dyn Archive) {
    assert_eq!(read(a, "a.txt"), HELLO);
    assert_eq!(read(a, "sub/b.txt"), big());
    let mut seen = Vec::new();
    a.walk(&mut |_| true, &mut |e, r| {
        let mut v = Vec::new();
        r.read_to_end(&mut v).unwrap();
        seen.push((e.path.clone(), v.len()));
        Ok(())
    })
    .unwrap();
    assert!(seen.contains(&("a.txt".to_string(), 6)));
    assert!(seen.contains(&("sub/b.txt".to_string(), 4800)));
}

fn ar_member(name: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", 1_700_000_000, 0, 0, "100644", data.len()).into_bytes();
    v.extend_from_slice(data);
    if data.len() % 2 == 1 {
        v.push(b'\n');
    }
    v
}

fn cpio_newc(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut entry = |name: &str, data: &[u8], mode: u32| {
        let mut n = name.as_bytes().to_vec();
        n.push(0);
        let h = format!(
            "070701{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}",
            1, mode, 0, 0, 1, 1_700_000_000u32, data.len(), 0, 0, 0, 0, n.len(), 0
        );
        out.extend_from_slice(h.as_bytes());
        out.extend_from_slice(&n);
        while out.len() % 4 != 0 {
            out.push(0);
        }
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    };
    for (n, d) in files {
        entry(n, d, 0o100644);
    }
    entry("TRAILER!!!", b"", 0);
    out
}

fn cab_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let data: Vec<u8> = files.iter().flat_map(|(_, d)| d.iter().copied()).collect();
    let names: usize = files.iter().map(|(n, _)| 16 + n.len() + 1).sum();
    let data_at = 36 + 8 + names;
    let total = data_at + 8 + data.len();
    let mut v = b"MSCF".to_vec();
    for x in [0u32, total as u32, 0, 44, 0] {
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.extend_from_slice(&[3, 1]);
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&(files.len() as u16).to_le_bytes());
    v.extend_from_slice(&[0; 6]);
    // folder
    v.extend_from_slice(&(data_at as u32).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    let mut offset = 0u32;
    for (n, d) in files {
        v.extend_from_slice(&(d.len() as u32).to_le_bytes());
        v.extend_from_slice(&offset.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(&((2026 - 1980) << 9 | 10 << 5 | 15u16).to_le_bytes());
        v.extend_from_slice(&(12u16 << 11).to_le_bytes());
        v.extend_from_slice(&0x20u16.to_le_bytes());
        v.extend_from_slice(n.as_bytes());
        v.push(0);
        offset += d.len() as u32;
    }
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&(data.len() as u16).to_le_bytes());
    v.extend_from_slice(&(data.len() as u16).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

fn dos(h: u32) -> (u16, u16) {
    (((2026 - 1980) << 9 | 10 << 5 | 15) as u16, (h << 11) as u16)
}

fn crc16_arc(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

fn lzh_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data) in files {
        let name = name.replace('/', "\\");
        let (date, time) = dos(12);
        let mut h = b"-lh0-".to_vec();
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&time.to_le_bytes());
        h.extend_from_slice(&date.to_le_bytes());
        h.extend_from_slice(&[0x20, 0, name.len() as u8]);
        h.extend_from_slice(name.as_bytes());
        h.extend_from_slice(&crc16_arc(data).to_le_bytes());
        out.push(h.len() as u8);
        out.push(h.iter().fold(0u8, |s, &b| s.wrapping_add(b)));
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
    }
    out.push(0);
    out
}

fn rar4_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let crc16 = |b: &[u8]| (crc32fast::hash(b) & 0xffff) as u16;
    let mut out = b"Rar!\x1a\x07\x00".to_vec();
    let mut main = vec![0x73, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0];
    out.extend_from_slice(&crc16(&main).to_le_bytes());
    out.append(&mut main);
    for (name, data) in files {
        let (date, time) = dos(12);
        let mut h = vec![0x74, 0x00, 0x80];
        h.extend_from_slice(&((32 + name.len()) as u16).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.push(2);
        h.extend_from_slice(&crc32fast::hash(data).to_le_bytes());
        h.extend_from_slice(&time.to_le_bytes());
        h.extend_from_slice(&date.to_le_bytes());
        h.extend_from_slice(&[20, 0x30]);
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&0x20u32.to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&crc16(&h).to_le_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
    }
    out.extend_from_slice(&[0xc4, 0x3d, 0x7b, 0x00, 0x40, 0x07, 0x00]);
    out
}

fn iso_record(name: &[u8], extent: u32, size: u32, dir: bool) -> Vec<u8> {
    let mut len = 33 + name.len();
    if len % 2 == 1 {
        len += 1;
    }
    let mut r = vec![0u8; len];
    r[0] = len as u8;
    r[2..6].copy_from_slice(&extent.to_le_bytes());
    r[6..10].copy_from_slice(&extent.to_be_bytes());
    r[10..14].copy_from_slice(&size.to_le_bytes());
    r[14..18].copy_from_slice(&size.to_be_bytes());
    r[18..25].copy_from_slice(&[126, 10, 15, 12, 0, 0, 0]);
    r[25] = if dir { 2 } else { 0 };
    r[28..30].copy_from_slice(&1u16.to_le_bytes());
    r[30..32].copy_from_slice(&1u16.to_be_bytes());
    r[32] = name.len() as u8;
    r[33..33 + name.len()].copy_from_slice(name);
    r
}

fn iso_image() -> Vec<u8> {
    let mut img = vec![0u8; 22 * 2048];
    let dir = |entries: Vec<Vec<u8>>, at: usize, img: &mut Vec<u8>| {
        let mut p = at * 2048;
        for e in entries {
            img[p..p + e.len()].copy_from_slice(&e);
            p += e.len();
        }
    };
    let mut pvd = vec![0u8; 2048];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pvd[128..130].copy_from_slice(&2048u16.to_le_bytes());
    pvd[130..132].copy_from_slice(&2048u16.to_be_bytes());
    pvd[156..156 + 34].copy_from_slice(&iso_record(&[0], 18, 2048, true)[..34]);
    img[16 * 2048..17 * 2048].copy_from_slice(&pvd);
    img[17 * 2048] = 255;
    img[17 * 2048 + 1..17 * 2048 + 6].copy_from_slice(b"CD001");
    img[17 * 2048 + 6] = 1;
    dir(
        vec![
            iso_record(&[0], 18, 2048, true),
            iso_record(&[1], 18, 2048, true),
            iso_record(b"A.TXT;1", 20, 6, false),
            iso_record(b"SUB", 19, 2048, true),
        ],
        18,
        &mut img,
    );
    dir(
        vec![
            iso_record(&[0], 19, 2048, true),
            iso_record(&[1], 18, 2048, true),
            iso_record(b"B.TXT;1", 21, 4800, false),
        ],
        19,
        &mut img,
    );
    img[20 * 2048..20 * 2048 + 6].copy_from_slice(HELLO);
    img.resize(21 * 2048 + 4800, 0);
    img[21 * 2048..21 * 2048 + 4800].copy_from_slice(&big());
    img
}

#[test]
fn ar_members_and_long_names() {
    let mut bytes = b"!<arch>\n".to_vec();
    bytes.extend(ar_member("//", b"a_rather_long_member_name.txt/\n"));
    bytes.extend(ar_member("a.txt/", HELLO));
    bytes.extend(ar_member("/0", b"long"));
    let a = open_bytes(bytes, "x.ar");
    assert_eq!(a.kind(), Kind::Ar);
    assert_eq!(read(&*a, "a.txt"), HELLO);
    assert_eq!(read(&*a, "a_rather_long_member_name.txt"), b"long");
}

#[test]
fn cpio_newc_files() {
    let a = open_bytes(cpio_newc(&[("a.txt", HELLO), ("sub/b.txt", &big())]), "x.cpio");
    assert_eq!(a.kind(), Kind::Cpio);
    check_both(&*a);
}

#[test]
fn cab_stored_files() {
    let a = open_bytes(cab_stored(&[("a.txt", HELLO), ("sub\\b.txt", &big())]), "x.cab");
    assert_eq!(a.kind(), Kind::Cab);
    check_both(&*a);
}

#[test]
fn lzh_stored_files() {
    let a = open_bytes(lzh_stored(&[("a.txt", HELLO), ("sub/b.txt", &big())]), "x.lzh");
    assert_eq!(a.kind(), Kind::Lzh);
    check_both(&*a);
}

#[test]
fn iso_with_a_subdirectory() {
    let a = open_bytes(iso_image(), "x.iso");
    assert_eq!(a.kind(), Kind::Iso);
    let paths: Vec<&str> = a.entries().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["A.TXT", "SUB", "SUB/B.TXT"]);
    assert_eq!(read(&*a, "A.TXT"), HELLO);
    assert_eq!(read(&*a, "SUB/B.TXT"), big());
}

#[test]
fn rar_from_a_file_and_not_from_memory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.rar");
    fs::write(&path, rar4_stored(&[("a.txt", HELLO), ("sub/b.txt", &big())])).unwrap();
    let a = open(&path).unwrap();
    assert_eq!(a.kind(), Kind::Rar);
    check_both(&*a);
    let out = dir.path().join("out");
    let report = extract(&*a, &ExtractOptions::new(&out, "x.rar"), &Progress::new()).unwrap();
    assert!(report.failed.is_empty() && report.blocked.is_empty());
    assert_eq!(fs::read(out.join("x").join("sub").join("b.txt")).unwrap(), big());

    let err = open_source(Arc::new(MemSource(fs::read(&path).unwrap())), "x.rar").map(|_| ()).unwrap_err();
    assert!(matches!(err, Error::Unsupported(_)));
}

#[test]
fn damaged_archives_are_errors_not_panics() {
    let mut cab = cab_stored(&[("a.txt", HELLO)]);
    cab.truncate(40);
    assert!(open_source(Arc::new(MemSource(cab)), "x.cab").is_err());
    let mut ar = b"!<arch>\n".to_vec();
    ar.extend(ar_member("a.txt/", HELLO));
    ar.truncate(ar.len() - 3);
    assert!(open_source(Arc::new(MemSource(ar)), "x.ar").is_err());
    let mut cpio = cpio_newc(&[("a.txt", HELLO)]);
    cpio.truncate(118);
    assert!(open_source(Arc::new(MemSource(cpio)), "x.cpio").is_err());
    let mut iso = iso_image();
    iso.truncate(19 * 2048);
    assert!(open_source(Arc::new(MemSource(iso)), "x.iso").is_err());
    assert!(open_source(Arc::new(MemSource(lzh_stored(&[("a", HELLO)])[..10].to_vec())), "x.lzh").is_err());
}

#[test]
fn a_cpio_cannot_write_outside_the_destination() {
    let dir = tempfile::tempdir().unwrap();
    let a = open_bytes(cpio_newc(&[("../escaped.txt", HELLO), ("ok.txt", HELLO)]), "evil.cpio");
    let dest = dir.path().join("dest");
    let _ = extract(&*a, &ExtractOptions::new(&dest, "evil.cpio"), &Progress::new());
    assert!(!dir.path().join("escaped.txt").exists());
}

fn gzip(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

#[test]
fn compressed_cpio_and_rpm_payload() {
    let cpio = cpio_newc(&[("./a.txt", HELLO), ("./sub/b.txt", &big())]);
    let a = open_bytes(gzip(&cpio), "initrd.cpio.gz");
    assert_eq!(a.kind(), Kind::Cpio);
    check_both(&*a);

    let mut rpm = vec![0xed, 0xab, 0xee, 0xdb, 3, 0];
    rpm.resize(96, 0);
    for _ in 0..2 {
        // an empty signature header, then an empty main header
        rpm.extend_from_slice(&[0x8e, 0xad, 0xe8, 0x01, 0, 0, 0, 0]);
        rpm.extend_from_slice(&[0; 8]);
    }
    rpm.extend(gzip(&cpio));
    let a = open_bytes(rpm, "pkg.rpm");
    assert_eq!(a.kind(), Kind::Rpm);
    check_both(&*a);
}

#[test]
fn a_rar_entry_too_big_for_memory_goes_through_a_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.rar");
    let data = vec![7u8; 65 << 20];
    fs::write(&path, rar4_stored(&[("big.bin", &data)])).unwrap();
    let a = open(&path).unwrap();
    let mut seen = 0u64;
    a.walk(&mut |_| true, &mut |_, r| {
        seen = std::io::copy(r, &mut std::io::sink())?;
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, data.len() as u64);
}
