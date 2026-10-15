//! Self-update from GitHub Releases. The only network traffic Packrat ever produces: one request a day to
//! `api.github.com` for the latest release, and, when the user asks for it, the download of `Packrat.exe` and
//! `SHA256SUMS.txt`. The file is replaced only if its SHA-256 matches the published sum.

use crate::settings::Settings;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const REPO: &str = "ahsoggg-creator/packrat";
const DOWNLOAD_PREFIX: &str = "https://github.com/ahsoggg-creator/packrat/";
const MAX_DOWNLOAD: u64 = 200 << 20;
const CHECK_INTERVAL_SECS: i64 = 24 * 3600;

pub type Version = (u32, u32, u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub exe_url: String,
    pub sums_url: String,
}

pub fn parse_version(s: &str) -> Option<Version> {
    let mut it = s.trim().trim_start_matches(['v', 'V']).split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next().unwrap_or("0").split(['-', '+']).next()?.parse().ok()?;
    Some((major, minor, patch))
}

pub fn current_version() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0))
}

pub fn version_text(v: Version) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

pub fn parse_release(json: &str) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad release JSON: {e}"))?;
    let tag = v["tag_name"].as_str().ok_or("release has no tag")?.to_owned();
    let version = parse_version(&tag).ok_or_else(|| format!("cannot read version from tag '{tag}'"))?;
    let asset = |name: &str| {
        v["assets"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["name"] == name))
            .and_then(|x| x["browser_download_url"].as_str())
            .map(str::to_owned)
    };
    let exe_url = asset("Packrat.exe").ok_or("release has no Packrat.exe")?;
    let sums_url = asset("SHA256SUMS.txt").ok_or("release has no SHA256SUMS.txt")?;
    for u in [&exe_url, &sums_url] {
        if !u.starts_with(DOWNLOAD_PREFIX) {
            return Err(format!("refusing to download from an unexpected address: {u}"));
        }
    }
    Ok(Release {
        tag,
        version,
        exe_url,
        sums_url,
    })
}

/// `hash  name` lines as written by `sha256sum`.
pub fn parse_sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (hash, name) = l.trim().split_once(char::is_whitespace)?;
            let name = name.trim().trim_start_matches('*');
            (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| (name.to_owned(), hash.to_ascii_lowercase()))
        })
        .collect()
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Short requests (the release information, the checksum file).
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .user_agent(format!("Packrat/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// The download of the program itself: a slow connection must not abort a transfer that is making progress, so only
/// connecting and the response headers have short limits and the whole transfer gets half an hour.
fn download_agent() -> ureq::Agent {
    use std::time::Duration;
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_global(Some(Duration::from_secs(30 * 60)))
        .user_agent(format!("Packrat/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Asks GitHub for the latest release. `Ok(None)` when this build is up to date.
pub fn check_latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let body = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let rel = parse_release(&body)?;
    Ok((rel.version > current_version()).then_some(rel))
}

pub fn due(settings: &Settings, now: i64) -> bool {
    settings.check_updates && now - settings.last_update_check >= CHECK_INTERVAL_SECS
}

/// Downloads the new exe next to `current`, verifies it against the published checksum, and returns its path.
pub fn download_verified(rel: &Release, current: &Path, progress: &dyn Fn(u64, u64)) -> Result<PathBuf, String> {
    let sums_text = agent()
        .get(&rel.sums_url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let expected = parse_sums(&sums_text)
        .remove("Packrat.exe")
        .ok_or("SHA256SUMS.txt has no entry for Packrat.exe")?;

    let target = current.with_extension("exe.new");
    let result = (|| -> Result<(), String> {
        let resp = download_agent().get(&rel.exe_url).call().map_err(|e| e.to_string())?;
        let total = resp.body().content_length().unwrap_or(0);
        let mut reader = resp.into_body().into_reader().take(MAX_DOWNLOAD);
        let mut out = fs::File::create(&target).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 256 << 10];
        let mut done = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            progress(done, total);
        }
        out.flush().map_err(|e| e.to_string())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&target);
        return Err(e);
    }
    let actual = sha256_file(&target).map_err(|e| e.to_string())?;
    if actual != expected {
        let _ = fs::remove_file(&target);
        return Err(format!("checksum mismatch (expected {expected}, got {actual})"));
    }
    let mut head = [0u8; 2];
    if fs::File::open(&target)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_err()
        || &head != b"MZ"
    {
        let _ = fs::remove_file(&target);
        return Err("the downloaded file is not a Windows executable".into());
    }
    Ok(target)
}

/// Swaps `new_exe` into place of `current`. A running exe cannot be overwritten but can be renamed, so the old
/// one moves to `.old` (removed on the next start).
pub fn apply(new_exe: &Path, current: &Path) -> Result<(), String> {
    let old = current.with_extension("exe.old");
    let _ = fs::remove_file(&old);
    fs::rename(current, &old).map_err(|e| format!("cannot move the current file aside: {e}"))?;
    if let Err(e) = fs::rename(new_exe, current) {
        let _ = fs::rename(&old, current);
        return Err(format!("cannot put the new file in place: {e}"));
    }
    Ok(())
}

pub fn cleanup_old(current: &Path) {
    let _ = fs::remove_file(current.with_extension("exe.old"));
    let _ = fs::remove_file(current.with_extension("exe.new"));
}

/// `Packrat.exe --update`: check, download, verify, replace; prints the outcome to the parent console.
pub fn run_headless() -> i32 {
    use crate::{diag, winint};
    let Ok(current) = std::env::current_exe() else {
        winint::console_print("cannot locate the running executable\n");
        return 2;
    };
    match check_latest() {
        Err(e) => {
            diag::warn(&format!("update check failed: {e}"));
            winint::console_print(&format!("update check failed: {e}\n"));
            2
        }
        Ok(None) => {
            winint::console_print(&format!("Packrat {} is up to date\n", version_text(current_version())));
            0
        }
        Ok(Some(rel)) => {
            winint::console_print(&format!("downloading {}...\n", rel.tag));
            let applied = download_verified(&rel, &current, &|_, _| {}).and_then(|new| apply(&new, &current));
            match applied {
                Ok(()) => {
                    diag::info(&format!("updated to {}", rel.tag));
                    winint::console_print(&format!("updated to {}\n", rel.tag));
                    0
                }
                Err(e) => {
                    diag::error(&format!("update failed: {e}"));
                    winint::console_print(&format!("update failed: {e}\n"));
                    1
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("1.10"), Some((1, 10, 0)));
        assert_eq!(parse_version("v0.3.1-beta"), Some((0, 3, 1)));
        assert_eq!(parse_version("nope"), None);
        assert!(parse_version("v0.10.0") > parse_version("v0.9.9"));
    }

    const RELEASE: &str = r#"{"tag_name":"v0.2.0","assets":[
        {"name":"Packrat.exe","browser_download_url":"https://github.com/ahsoggg-creator/packrat/releases/download/v0.2.0/Packrat.exe"},
        {"name":"SHA256SUMS.txt","browser_download_url":"https://github.com/ahsoggg-creator/packrat/releases/download/v0.2.0/SHA256SUMS.txt"}]}"#;

    #[test]
    fn release_json_is_parsed_and_urls_are_pinned_to_the_repo() {
        let r = parse_release(RELEASE).unwrap();
        assert_eq!(r.version, (0, 2, 0));
        assert!(r.exe_url.ends_with("/Packrat.exe"));
        let evil = RELEASE.replace("github.com/ahsoggg-creator/packrat", "evil.example.com/x");
        assert!(parse_release(&evil).is_err());
        assert!(parse_release(r#"{"tag_name":"v1.0.0","assets":[]}"#).is_err());
    }

    #[test]
    fn sums_file_is_parsed() {
        let h = "a".repeat(64);
        let m = parse_sums(&format!(
            "{h}  Packrat.exe\n{}  *packrat-cli.exe\nnot a line\n",
            "B".repeat(64)
        ));
        assert_eq!(m["Packrat.exe"], h);
        assert_eq!(m["packrat-cli.exe"], "b".repeat(64));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn checksum_and_replace() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.bin");
        fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_file(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let current = dir.path().join("Packrat.exe");
        let new = dir.path().join("Packrat.exe.new");
        fs::write(&current, b"old").unwrap();
        fs::write(&new, b"new").unwrap();
        apply(&new, &current).unwrap();
        assert_eq!(fs::read(&current).unwrap(), b"new");
        assert_eq!(fs::read(current.with_extension("exe.old")).unwrap(), b"old");
        cleanup_old(&current);
        assert!(!current.with_extension("exe.old").exists());
    }

    /// Talks to GitHub for real: `cargo test -p packrat-app -- --ignored real_release`.
    #[test]
    #[ignore = "needs the network"]
    fn real_release_downloads_and_verifies() {
        let rel = parse_release(
            &ureq::get(format!("https://api.github.com/repos/{REPO}/releases/tags/v0.2.0"))
                .header("User-Agent", "packrat-test")
                .call()
                .unwrap()
                .body_mut()
                .read_to_string()
                .unwrap(),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("Packrat.exe");
        std::fs::write(&current, b"old").unwrap();
        let started = std::time::Instant::now();
        let new = download_verified(&rel, &current, &|_, _| {}).unwrap();
        eprintln!("downloaded and verified in {:.1} s", started.elapsed().as_secs_f64());
        apply(&new, &current).unwrap();
        assert!(std::fs::metadata(&current).unwrap().len() > 1_000_000);
    }

    #[test]
    fn check_is_due_once_a_day_and_only_when_enabled() {
        let mut s = Settings::default();
        assert!(due(&s, 1_000_000));
        s.last_update_check = 1_000_000;
        assert!(!due(&s, 1_000_000 + 3600));
        assert!(due(&s, 1_000_000 + 25 * 3600));
        s.check_updates = false;
        assert!(!due(&s, 9_999_999));
    }
}
