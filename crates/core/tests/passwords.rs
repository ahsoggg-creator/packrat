use packrat_core::codec::stream::Compression;
use packrat_core::create::{CreateOptions, Format, create_archive};
use packrat_core::input::collect_inputs;
use packrat_core::open::{open, open_with};
use packrat_core::zip::{ZipMethod, ZipOptions, write_zip};
use packrat_core::{Error, OpenOptions, Progress};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const SECRET: &str = "s3cret-пароль";

fn sample_dir(root: &Path) -> PathBuf {
    let src = root.join("data");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a.txt"), "alpha ".repeat(2000)).unwrap();
    fs::write(src.join("empty.bin"), b"").unwrap();
    let mut x = 9u64;
    let noise: Vec<u8> = (0..300_000)
        .map(|_| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (x >> 33) as u8
        })
        .collect();
    fs::write(src.join("sub").join("noise.bin"), noise).unwrap();
    src
}

fn read_entry(a: &dyn packrat_core::Archive, name: &str) -> Result<Vec<u8>, Error> {
    let e = a
        .entries()
        .iter()
        .find(|e| e.path == name)
        .unwrap_or_else(|| panic!("{name} missing"));
    let mut v = Vec::new();
    a.open_entry(e.index)?.read_to_end(&mut v).map_err(Error::from)?;
    Ok(v)
}

fn seven_zip() -> Option<PathBuf> {
    let p = PathBuf::from(r"C:\Program Files\7-Zip\7z.exe");
    p.exists().then_some(p)
}

fn run(exe: &Path, args: &[&str], cwd: &Path) -> (bool, String) {
    let out = Command::new(exe).args(args).current_dir(cwd).output().unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn encrypted_zip_roundtrip_single_and_multi_block() {
    let dir = tempfile::tempdir().unwrap();
    let src = sample_dir(dir.path());
    let (items, _) = collect_inputs(std::slice::from_ref(&src)).unwrap();
    for method in [ZipMethod::Store, ZipMethod::Deflate, ZipMethod::Zstd] {
        for block_size in [4 << 20, 64 << 10] {
            let opts = ZipOptions {
                method,
                threads: 3,
                block_size,
                password: Some(packrat_core::Password::new(SECRET)),
                ..Default::default()
            };
            let mut buf = Vec::new();
            write_zip(&mut buf, &items, &opts, &Progress::new()).unwrap();
            let path = dir.path().join("enc.zip");
            fs::write(&path, &buf).unwrap();

            let good = open_with(&path, &OpenOptions::default().with_password(SECRET)).unwrap();
            assert!(
                good.entries()
                    .iter()
                    .any(|e| e.encrypted && e.method.starts_with("AES-256")),
                "{method:?}"
            );
            assert_eq!(
                read_entry(&*good, "data/a.txt").unwrap(),
                "alpha ".repeat(2000).into_bytes()
            );
            assert_eq!(read_entry(&*good, "data/empty.bin").unwrap(), b"");
            assert_eq!(
                read_entry(&*good, "data/sub/noise.bin").unwrap(),
                fs::read(src.join("sub/noise.bin")).unwrap()
            );

            let none = open(&path).unwrap();
            assert!(matches!(read_entry(&*none, "data/a.txt"), Err(Error::PasswordRequired)));
            let bad = open_with(&path, &OpenOptions::default().with_password("nope")).unwrap();
            assert!(matches!(read_entry(&*bad, "data/a.txt"), Err(Error::WrongPassword)));
            // file contents are really not in the clear
            assert!(!buf.windows(12).any(|w| w == b"alpha alpha "));
        }
    }
}

#[test]
fn tar_cannot_be_encrypted() {
    let dir = tempfile::tempdir().unwrap();
    let src = sample_dir(dir.path());
    let mut opts = CreateOptions::new(Format::Tar(Compression::Zst));
    opts.password = Some(packrat_core::Password::new("x"));
    let out = dir.path().join("x.tar.zst");
    let err = create_archive(&out, &[src], &opts, &Progress::new()).unwrap_err();
    assert!(matches!(err, Error::Unsupported(_)), "{err}");
    assert!(!out.exists(), "no half-written file is left behind");
}

#[test]
fn encrypted_7z_roundtrip_with_and_without_name_encryption() {
    let dir = tempfile::tempdir().unwrap();
    let src = sample_dir(dir.path());
    for encrypt_names in [true, false] {
        let out = dir.path().join(format!("enc-{encrypt_names}.7z"));
        let mut opts = CreateOptions::new(Format::SevenZ);
        opts.password = Some(packrat_core::Password::new(SECRET));
        opts.encrypt_names = encrypt_names;
        create_archive(&out, std::slice::from_ref(&src), &opts, &Progress::new()).unwrap();

        let without = open(&out);
        if encrypt_names {
            assert!(
                matches!(without.err(), Some(Error::PasswordRequired)),
                "header is encrypted"
            );
        } else {
            let a = without.unwrap();
            assert!(matches!(read_entry(&*a, "data/a.txt"), Err(Error::PasswordRequired)));
        }
        let a = open_with(&out, &OpenOptions::default().with_password(SECRET)).unwrap();
        assert!(a.entries().iter().any(|e| e.encrypted));
        assert_eq!(
            read_entry(&*a, "data/a.txt").unwrap(),
            "alpha ".repeat(2000).into_bytes()
        );
        assert_eq!(
            read_entry(&*a, "data/sub/noise.bin").unwrap(),
            fs::read(src.join("sub/noise.bin")).unwrap()
        );

        let wrong = open_with(&out, &OpenOptions::default().with_password("wrong"));
        match wrong {
            Err(e) => assert!(
                matches!(e, Error::WrongPassword | Error::PasswordRequired | Error::Corrupt(_)),
                "{e}"
            ),
            Ok(a) => assert!(read_entry(&*a, "data/a.txt").is_err()),
        }
    }
}

#[test]
fn seven_zip_and_packrat_understand_each_others_encrypted_archives() {
    let Some(z) = seven_zip() else {
        eprintln!("7-Zip is not installed: skipped");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let src = sample_dir(dir.path());
    let want_a = "alpha ".repeat(2000).into_bytes();

    // Packrat -> 7-Zip
    let (items, _) = collect_inputs(std::slice::from_ref(&src)).unwrap();
    let mut buf = Vec::new();
    write_zip(
        &mut buf,
        &items,
        &ZipOptions {
            block_size: 64 << 10,
            password: Some(packrat_core::Password::new("plain-ascii")),
            ..Default::default()
        },
        &Progress::new(),
    )
    .unwrap();
    fs::write(dir.path().join("p.zip"), &buf).unwrap();
    let (ok, log) = run(&z, &["t", "p.zip", "-pplain-ascii"], dir.path());
    assert!(
        ok && log.contains("Everything is Ok"),
        "7-Zip rejected a Packrat AES zip: {log}"
    );
    let (ok, _) = run(&z, &["t", "p.zip", "-pwrong"], dir.path());
    assert!(!ok, "7-Zip accepted a wrong password");

    let mut o = CreateOptions::new(Format::SevenZ);
    o.password = Some(packrat_core::Password::new("plain-ascii"));
    create_archive(
        &dir.path().join("p.7z"),
        std::slice::from_ref(&src),
        &o,
        &Progress::new(),
    )
    .unwrap();
    let (ok, log) = run(&z, &["t", "p.7z", "-pplain-ascii"], dir.path());
    assert!(
        ok && log.contains("Everything is Ok"),
        "7-Zip rejected a Packrat encrypted 7z: {log}"
    );

    // 7-Zip -> Packrat: AES-256 zip, ZipCrypto zip, encrypted 7z with encrypted names
    let pw = OpenOptions::default().with_password("plain-ascii");
    for (name, args) in [
        (
            "z-aes.zip",
            vec!["a", "-tzip", "-pplain-ascii", "-mem=AES256", "z-aes.zip", "data"],
        ),
        (
            "z-old.zip",
            vec!["a", "-tzip", "-pplain-ascii", "-mem=ZipCrypto", "z-old.zip", "data"],
        ),
        (
            "z-enc.7z",
            vec!["a", "-t7z", "-pplain-ascii", "-mhe=on", "z-enc.7z", "data"],
        ),
    ] {
        let (ok, log) = run(&z, &args, dir.path());
        assert!(ok, "7-Zip could not create {name}: {log}");
        let path = dir.path().join(name);
        if name.ends_with(".7z") {
            assert!(
                matches!(open(&path).err(), Some(Error::PasswordRequired)),
                "{name}: names are encrypted"
            );
        } else {
            let a = open(&path).unwrap();
            assert!(
                matches!(read_entry(&*a, "data/a.txt"), Err(Error::PasswordRequired)),
                "{name}"
            );
        }
        let a = open_with(&path, &pw).unwrap();
        assert_eq!(read_entry(&*a, "data/a.txt").unwrap(), want_a, "{name}");
        assert_eq!(
            read_entry(&*a, "data/sub/noise.bin").unwrap(),
            fs::read(src.join("sub/noise.bin")).unwrap(),
            "{name}"
        );
        if name == "z-old.zip" {
            assert!(
                a.entries().iter().any(|e| e.method.starts_with("ZipCrypto")),
                "weak encryption is labelled"
            );
        }
        let bad = open_with(&path, &OpenOptions::default().with_password("wrong"));
        match bad {
            Err(e) => assert!(
                matches!(e, Error::WrongPassword | Error::PasswordRequired),
                "{name}: {e}"
            ),
            Ok(a) => assert!(
                read_entry(&*a, "data/a.txt").is_err(),
                "{name}: wrong password must not read"
            ),
        }
    }
}
