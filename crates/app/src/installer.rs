//! Per-user installer / uninstaller. Nothing here needs administrator rights: files go to
//! `%LOCALAPPDATA%\Programs\Packrat`, the shortcut to the user's Start menu and the "Apps & features" entry
//! to `HKCU`. All locations live in `Env`, so tests can run the whole thing in a sandbox.

use crate::winint;
use std::fs;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "Packrat";
pub const EXE_NAME: &str = "Packrat.exe";

#[derive(Clone, Debug)]
pub struct Env {
    pub install_dir: PathBuf,
    pub start_menu_dir: PathBuf,
    /// The "Send to" folder of the user.
    pub sendto_dir: PathBuf,
    /// Registry path below HKCU that holds `Microsoft\Windows\CurrentVersion\Uninstall` (normally `Software`).
    pub software_key: String,
}

impl Env {
    /// The user's real locations. `PACKRAT_SANDBOX=<folder>` redirects everything (files and registry) into that
    /// folder and a throw-away registry branch, so the installer can be run end to end without touching the system.
    pub fn real() -> Env {
        if let Some(sandbox) = std::env::var_os("PACKRAT_SANDBOX") {
            let root = PathBuf::from(sandbox);
            return Env {
                install_dir: root.join("Programs").join(APP_NAME),
                start_menu_dir: root.join("StartMenu"),
                sendto_dir: root.join("SendTo"),
                software_key: r"Software\PackratSandbox".into(),
            };
        }
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let roaming = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Env {
            install_dir: local.join("Programs").join(APP_NAME),
            start_menu_dir: roaming.join(r"Microsoft\Windows\Start Menu\Programs"),
            sendto_dir: roaming.join(r"Microsoft\Windows\SendTo"),
            software_key: "Software".into(),
        }
    }

    pub fn installed_exe(&self) -> PathBuf {
        self.install_dir.join(EXE_NAME)
    }

    pub fn shortcut(&self) -> PathBuf {
        self.start_menu_dir.join(format!("{APP_NAME}.lnk"))
    }

    pub fn uninstall_key_path(&self) -> String {
        format!(
            r"{}\Microsoft\Windows\CurrentVersion\Uninstall\{APP_NAME}",
            self.software_key
        )
    }
}

/// True when this executable was started as the installer (named `*Setup*.exe` or given `--install`).
pub fn started_as_installer(exe_name: &str, args: &[String]) -> bool {
    exe_name.to_ascii_lowercase().contains("setup") || args.iter().any(|a| a == "--install")
}

pub fn is_installed_copy(env: &Env, exe: &Path) -> bool {
    exe.starts_with(&env.install_dir)
}

#[cfg(windows)]
fn write_registry(env: &Env, exe: &Path) -> Result<(), String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(env.uninstall_key_path())
        .map_err(|e| e.to_string())?;
    let size_kb = fs::metadata(exe).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    let put = |name: &str, v: &str| key.set_value(name, &v.to_owned()).map_err(|e| e.to_string());
    put("DisplayName", APP_NAME)?;
    put("DisplayVersion", env!("CARGO_PKG_VERSION"))?;
    put("Publisher", "ahsoggg-creator")?;
    put("InstallLocation", &env.install_dir.to_string_lossy())?;
    put("DisplayIcon", &exe.to_string_lossy())?;
    put("UninstallString", &format!("\"{}\" --uninstall", exe.display()))?;
    put("URLInfoAbout", "https://github.com/ahsoggg-creator/packrat")?;
    key.set_value("EstimatedSize", &size_kb).map_err(|e| e.to_string())?;
    key.set_value("NoModify", &1u32).map_err(|e| e.to_string())?;
    key.set_value("NoRepair", &1u32).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(windows))]
fn write_registry(_: &Env, _: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn delete_registry(env: &Env) {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(env.uninstall_key_path());
}

#[cfg(not(windows))]
fn delete_registry(_: &Env) {}

/// Copies `source_exe` into the install folder and registers it. Returns the installed exe path.
pub fn install(env: &Env, source_exe: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(&env.install_dir).map_err(|e| format!("cannot create {}: {e}", env.install_dir.display()))?;
    let target = env.installed_exe();
    let same = fs::canonicalize(source_exe).ok() == fs::canonicalize(&target).ok();
    if !same {
        // a running copy cannot be overwritten but can be renamed out of the way
        if target.exists() {
            let old = target.with_extension("exe.old");
            let _ = fs::remove_file(&old);
            fs::rename(&target, &old).map_err(|e| format!("cannot replace the installed copy: {e}"))?;
        }
        fs::copy(source_exe, &target).map_err(|e| format!("cannot copy the program: {e}"))?;
    }
    winint::create_shortcut(&env.shortcut(), &target, "", "Packrat — archive manager")?;
    write_registry(env, &target)?;
    crate::shell::register_if_enabled(env, &target);
    Ok(target)
}

/// Removes everything `install` created. When the running executable lives in the install folder, a detached
/// helper deletes it after this process exits.
pub fn uninstall(env: &Env, running_exe: Option<&Path>) -> Result<(), String> {
    crate::shell::unregister(env);
    let _ = fs::remove_file(env.shortcut());
    delete_registry(env);
    let running_inside = running_exe.is_some_and(|e| is_installed_copy(env, e));
    if running_inside {
        schedule_dir_removal(&env.install_dir);
    } else if env.install_dir.exists() {
        fs::remove_dir_all(&env.install_dir)
            .map_err(|e| format!("cannot remove {}: {e}", env.install_dir.display()))?;
    }
    Ok(())
}

#[cfg(windows)]
fn schedule_dir_removal(dir: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Wait for this process to exit, then delete the folder (retrying while the exe is still locked). The whole
    // script is wrapped in one pair of quotes, which `cmd /C` strips, so the quoted path inside survives.
    let d = dir.display();
    let script = format!(
        "\"ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{d}\" & if exist \"{d}\" (ping 127.0.0.1 -n 4 >nul & rmdir /s /q \"{d}\")\""
    );
    let _ = std::process::Command::new("cmd.exe")
        .arg("/C")
        .raw_arg(script)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

#[cfg(not(windows))]
fn schedule_dir_removal(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn registry_has(env: &Env) -> bool {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(env.uninstall_key_path())
            .is_ok()
    }

    #[test]
    fn installer_detection() {
        assert!(started_as_installer("Packrat-Setup.exe", &[]));
        assert!(started_as_installer("packrat.exe", &["--install".into()]));
        assert!(!started_as_installer("Packrat.exe", &[]));
    }

    #[cfg(windows)]
    #[test]
    fn install_and_uninstall_in_a_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env {
            install_dir: dir.path().join("Programs").join("Packrat"),
            start_menu_dir: dir.path().join("StartMenu"),
            sendto_dir: dir.path().join("SendTo"),
            software_key: format!(r"Software\PackratTest-{}", std::process::id()),
        };
        let fake_exe = dir.path().join("Packrat-Setup.exe");
        fs::write(&fake_exe, b"MZ fake").unwrap();

        let installed = install(&env, &fake_exe).unwrap();
        assert_eq!(installed, env.installed_exe());
        assert_eq!(fs::read(&installed).unwrap(), b"MZ fake");
        let target = winint::read_shortcut_target(&env.shortcut()).expect("shortcut readable");
        // the shell may report the 8.3 short form of the temp folder: compare real paths
        assert_eq!(fs::canonicalize(target).unwrap(), fs::canonicalize(&installed).unwrap());
        assert!(registry_has(&env));
        assert!(
            crate::shell::sendto_links(&env).iter().any(|l| l.exists()),
            "Send to entry"
        );

        // installing again over an existing copy works (upgrade)
        fs::write(&fake_exe, b"MZ newer").unwrap();
        install(&env, &fake_exe).unwrap();
        assert_eq!(fs::read(&installed).unwrap(), b"MZ newer");

        uninstall(&env, None).unwrap();
        assert!(!env.install_dir.exists() && !env.shortcut().exists() && !registry_has(&env));
        assert!(crate::shell::sendto_links(&env).iter().all(|l| !l.exists()));

        // clean the sandbox registry root
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&env.software_key);
    }
}
