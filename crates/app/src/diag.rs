//! Diagnostics: a rotating log, crash reports and the text users can attach to a bug report.
//! Nothing sensitive is ever written: no passwords, no names of files inside archives, no file contents;
//! the user's profile folder is replaced by `%USERPROFILE%` in everything that is recorded.

use crate::settings::{self, Settings};
use packrat_core::timeutil;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_LOG_BYTES: u64 = 512 * 1024;
const LOG_TAIL_LINES: usize = 40;

static LOG_LOCK: Mutex<()> = Mutex::new(());

pub fn logs_dir() -> PathBuf {
    settings::data_dir().join("logs")
}

pub fn crashes_dir() -> PathBuf {
    logs_dir().join("crashes")
}

fn log_path() -> PathBuf {
    logs_dir().join("packrat.log")
}

fn now() -> i64 {
    timeutil::system_time_to_unix(std::time::SystemTime::now())
}

fn stamp(t: i64) -> String {
    let (y, mo, d, h, mi, s) = timeutil::parts_from_unix(t);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Replaces the user's profile path with a placeholder.
pub fn redact(text: &str) -> String {
    let mut out = text.to_owned();
    for var in ["USERPROFILE", "HOME"] {
        if let Some(p) = std::env::var_os(var) {
            let p = p.to_string_lossy().into_owned();
            if p.len() > 3 {
                out = out.replace(&p, "%USERPROFILE%");
                out = out.replace(&p.replace('\\', "/"), "%USERPROFILE%");
            }
        }
    }
    out
}

pub fn log(level: &str, message: &str) {
    let _guard = LOG_LOCK.lock();
    let _ = fs::create_dir_all(logs_dir());
    let path = log_path();
    if fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = fs::rename(&path, path.with_extension("log.1"));
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            f,
            "{} {level:<5} {}",
            stamp(now()),
            redact(message).replace('\n', " | ")
        );
    }
}

pub fn info(message: &str) {
    log("INFO", message);
}

pub fn warn(message: &str) {
    log("WARN", message);
}

pub fn error(message: &str) {
    log("ERROR", message);
}

pub fn log_tail(lines: usize) -> String {
    let text = fs::read_to_string(log_path()).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Latest crash report file, if any.
pub fn last_crash() -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(crashes_dir())
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    files.sort();
    files.pop()
}

/// Installs the panic hook that writes `logs\crashes\crash-<time>.txt`.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".into());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let backtrace = std::backtrace::Backtrace::force_capture();
        let t = now();
        let (y, mo, d, h, mi, s) = timeutil::parts_from_unix(t);
        let report = format!(
            "Packrat {} crash report\nTime (UTC): {}\nLocation: {location}\nMessage: {message}\n\nBacktrace:\n{backtrace}\n\nLog tail:\n{}\n",
            env!("CARGO_PKG_VERSION"),
            stamp(t),
            log_tail(LOG_TAIL_LINES),
        );
        let _ = fs::create_dir_all(crashes_dir());
        let file = crashes_dir().join(format!("crash-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}.txt"));
        let _ = fs::write(file, redact(&report));
        error(&format!("panic at {location}: {message}"));
    }));
}

#[cfg(windows)]
fn windows_version() -> String {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok()
        .map(|k| {
            let name: String = k.get_value("ProductName").unwrap_or_default();
            let build: String = k.get_value("CurrentBuild").unwrap_or_default();
            format!("{name} (build {build})")
        })
        .unwrap_or_else(|| "Windows".into())
}

#[cfg(not(windows))]
fn windows_version() -> String {
    std::env::consts::OS.into()
}

/// Text for the "copy diagnostics" button: versions, settings without personal data, the log tail.
pub fn diagnostic_text(settings: &Settings) -> String {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let crash = last_crash()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "none".into());
    format!(
        "Packrat {}\nOS: {}\nLogical cores: {cores}\nLanguage: {}\nThreads setting: {}\nSmart Extract default: {}\nUpdate checks: {}\nLast crash report: {crash}\n\nLog tail:\n{}\n",
        env!("CARGO_PKG_VERSION"),
        windows_version(),
        settings.language,
        settings.threads,
        settings.smart_default,
        settings.check_updates,
        log_tail(LOG_TAIL_LINES),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_path_is_redacted() {
        if let Some(p) = std::env::var_os("USERPROFILE") {
            let p = p.to_string_lossy().into_owned();
            let r = redact(&format!("cannot read {p}\\Documents\\x.zip"));
            assert!(!r.contains(&p) && r.contains("%USERPROFILE%"), "{r}");
        }
    }

    #[test]
    fn log_rotates_and_never_grows_without_bound() {
        settings::use_test_data_dir();
        let line = "x".repeat(900);
        for _ in 0..800 {
            info(&line);
        }
        let size = fs::metadata(log_path()).unwrap().len();
        assert!(size <= MAX_LOG_BYTES + 1000, "{size}");
        assert!(log_path().with_extension("log.1").exists());
        assert!(log_tail(3).lines().count() == 3);
    }
}
