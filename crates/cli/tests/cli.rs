use std::fs;
use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_packrat-cli"))
}

#[test]
fn create_list_extract_smart_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("project");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a.txt"), "alpha").unwrap();
    fs::write(src.join("sub/b.txt"), "beta").unwrap();
    let archive = dir.path().join("project.tar.zst");

    let out = cli()
        .args(["create"])
        .arg(&archive)
        .arg(&src)
        .args(["--threads", "2"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let out = cli().arg("list").arg(&archive).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && text.contains("project/sub/b.txt"), "{text}");

    let dest = dir.path().join("out");
    fs::create_dir(&dest).unwrap();
    let out = cli()
        .arg("extract")
        .arg(&archive)
        .arg("-o")
        .arg(&dest)
        .arg("--smart")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // one root folder: no project/project
    assert_eq!(fs::read_to_string(dest.join("project/sub/b.txt")).unwrap(), "beta");
    assert!(!dest.join("project/project").exists());

    // second run must not overwrite anything
    fs::write(dest.join("project/a.txt"), "edited").unwrap();
    let out = cli()
        .arg("extract")
        .arg(&archive)
        .arg("-o")
        .arg(&dest)
        .arg("--smart")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(fs::read_to_string(dest.join("project/a.txt")).unwrap(), "edited");
}

#[test]
fn bad_usage_exits_with_2() {
    assert_eq!(cli().arg("frobnicate").output().unwrap().status.code(), Some(2));
    assert_eq!(cli().output().unwrap().status.code(), Some(2));
    assert!(cli().arg("--version").output().unwrap().status.success());
}
