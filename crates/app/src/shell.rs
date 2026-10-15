//! Explorer integration, all per user (`HKCU`, no administrator rights):
//!
//! * the cascading "Packrat" context menu: on archives (extract smart / extract to… / test / open) and on every
//!   other file and folder (add to archive…, quick .zip / .7z / .tar.zst). It lives in the classic menu; on
//!   Windows 11 that is "Show more options" (Shift+F10).
//! * optional file associations with a recorded backup, so they can be undone exactly.
//!
//! Every location is derived from `Env`, so tests run against a throw-away registry branch.

use crate::installer::Env;
use crate::{i18n, settings};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const ARCHIVE_EXTS: [&str; 11] = [
    "zip", "7z", "tar", "gz", "tgz", "bz2", "xz", "txz", "zst", "tzst", "lz4",
];
pub const PROG_ID: &str = "Packrat.Archive";
const MENU_KEY: &str = "Packrat";

fn classes(env: &Env) -> String {
    format!(r"{}\Classes", env.software_key)
}

/// `(key name, label, command-line arguments)`; keys sort the menu.
type Item = (&'static str, i18n::Key, &'static str);

const ARCHIVE_ITEMS: [Item; 4] = [
    ("10smart", i18n::Key::MenuExtractHere, "--extract-here \"%1\""),
    ("20to", i18n::Key::MenuExtractTo, "--extract-to \"%1\""),
    ("30test", i18n::Key::MenuTest, "--test \"%1\""),
    ("40open", i18n::Key::MenuOpen, "\"%1\""),
];

const ADD_ITEMS: [Item; 4] = [
    ("10add", i18n::Key::MenuAdd, "--add \"%1\""),
    ("20zip", i18n::Key::MenuQuickZip, "--quick zip \"%1\""),
    ("30seven", i18n::Key::MenuQuick7z, "--quick 7z \"%1\""),
    ("40tarzst", i18n::Key::MenuQuickTarZst, "--quick tar.zst \"%1\""),
];

#[cfg(windows)]
mod imp {
    use super::*;
    use winreg::RegKey;
    use winreg::RegValue;
    use winreg::enums::{HKEY_CURRENT_USER, REG_NONE};

    fn hkcu() -> RegKey {
        RegKey::predef(HKEY_CURRENT_USER)
    }

    fn cascade(
        path: &str,
        label: &str,
        exe: &Path,
        applies_to: Option<&str>,
        items: &[Item],
        lang: i18n::Lang,
    ) -> Result<(), String> {
        let e = |x: std::io::Error| x.to_string();
        let (key, _) = hkcu().create_subkey(path).map_err(e)?;
        key.set_value("MUIVerb", &label).map_err(e)?;
        key.set_value("Icon", &format!("\"{}\",0", exe.display())).map_err(e)?;
        key.set_value("SubCommands", &"").map_err(e)?;
        if let Some(a) = applies_to {
            key.set_value("AppliesTo", &a).map_err(e)?;
        }
        for (name, text, args) in items {
            let (item, _) = hkcu().create_subkey(format!(r"{path}\shell\{name}")).map_err(e)?;
            item.set_value("MUIVerb", &text.text(lang)).map_err(e)?;
            let (cmd, _) = item.create_subkey("command").map_err(e)?;
            cmd.set_value("", &format!("\"{}\" {args}", exe.display())).map_err(e)?;
        }
        Ok(())
    }

    pub fn register(env: &Env, exe: &Path, lang: i18n::Lang) -> Result<(), String> {
        let base = classes(env);
        let not_archive = format!(
            "NOT ({})",
            ARCHIVE_EXTS
                .iter()
                .map(|x| format!("System.FileExtension:=\".{x}\""))
                .collect::<Vec<_>>()
                .join(" OR ")
        );
        cascade(
            &format!(r"{base}\*\shell\{MENU_KEY}"),
            MENU_KEY,
            exe,
            Some(&not_archive),
            &ADD_ITEMS,
            lang,
        )?;
        cascade(
            &format!(r"{base}\Directory\shell\{MENU_KEY}"),
            MENU_KEY,
            exe,
            None,
            &ADD_ITEMS,
            lang,
        )?;
        for ext in ARCHIVE_EXTS {
            cascade(
                &format!(r"{base}\SystemFileAssociations\.{ext}\shell\{MENU_KEY}"),
                MENU_KEY,
                exe,
                None,
                &ARCHIVE_ITEMS,
                lang,
            )?;
        }
        notify(env);
        Ok(())
    }

    pub fn unregister(env: &Env) {
        let base = classes(env);
        let mut paths = vec![
            format!(r"{base}\*\shell\{MENU_KEY}"),
            format!(r"{base}\Directory\shell\{MENU_KEY}"),
        ];
        paths.extend(
            ARCHIVE_EXTS
                .iter()
                .map(|x| format!(r"{base}\SystemFileAssociations\.{x}\shell\{MENU_KEY}")),
        );
        for p in paths {
            let _ = hkcu().delete_subkey_all(p);
        }
        notify(env);
    }

    pub fn is_registered(env: &Env) -> bool {
        hkcu()
            .open_subkey(format!(r"{}\*\shell\{MENU_KEY}\shell\10add\command", classes(env)))
            .is_ok()
    }

    #[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
    pub struct Previous {
        /// The `.ext` key did not exist before Packrat touched it.
        pub absent: bool,
        pub default: Option<String>,
        pub had_open_with: bool,
    }

    fn backup_path() -> PathBuf {
        settings::data_dir().join("assoc-backup.json")
    }

    fn load_backup() -> BTreeMap<String, Previous> {
        std::fs::read(backup_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_backup(b: &BTreeMap<String, Previous>) -> Result<(), String> {
        let _ = std::fs::create_dir_all(settings::data_dir());
        let json = serde_json::to_vec_pretty(b).map_err(|e| e.to_string())?;
        std::fs::write(backup_path(), json).map_err(|e| e.to_string())
    }

    pub fn associate(env: &Env, exe: &Path, exts: &[&str]) -> Result<(), String> {
        let e = |x: std::io::Error| x.to_string();
        let base = classes(env);
        // the program identity: what "open" does and which icon the files get
        let (prog, _) = hkcu().create_subkey(format!(r"{base}\{PROG_ID}")).map_err(e)?;
        prog.set_value("", &"Packrat archive").map_err(e)?;
        let (icon, _) = prog.create_subkey("DefaultIcon").map_err(e)?;
        icon.set_value("", &format!("\"{}\",0", exe.display())).map_err(e)?;
        let (cmd, _) = prog.create_subkey(r"shell\open\command").map_err(e)?;
        cmd.set_value("", &format!("\"{}\" \"%1\"", exe.display())).map_err(e)?;
        // so that it also shows up in "Open with" and can be picked in Settings > Default apps
        let (app, _) = hkcu()
            .create_subkey(format!(r"{base}\Applications\Packrat.exe"))
            .map_err(e)?;
        let (appcmd, _) = app.create_subkey(r"shell\open\command").map_err(e)?;
        appcmd
            .set_value("", &format!("\"{}\" \"%1\"", exe.display()))
            .map_err(e)?;
        let (supported, _) = app.create_subkey("SupportedTypes").map_err(e)?;
        for x in exts {
            supported.set_value(format!(".{x}"), &"").map_err(e)?;
        }

        let mut backup = load_backup();
        for x in exts {
            let path = format!(r"{base}\.{x}");
            let existing = hkcu().open_subkey(&path).ok();
            backup.entry((*x).to_owned()).or_insert_with(|| Previous {
                absent: existing.is_none(),
                default: existing.as_ref().and_then(|k| k.get_value::<String, _>("").ok()),
                had_open_with: existing
                    .as_ref()
                    .and_then(|k| k.open_subkey("OpenWithProgids").ok())
                    .is_some_and(|k| k.get_raw_value(PROG_ID).is_ok()),
            });
            save_backup(&backup)?;
            let (key, _) = hkcu().create_subkey(&path).map_err(e)?;
            key.set_value("", &PROG_ID).map_err(e)?;
            let (ow, _) = key.create_subkey("OpenWithProgids").map_err(e)?;
            ow.set_raw_value(
                PROG_ID,
                &RegValue {
                    bytes: Vec::new().into(),
                    vtype: REG_NONE,
                },
            )
            .map_err(e)?;
        }
        notify(env);
        Ok(())
    }

    pub fn restore_associations(env: &Env) -> Result<(), String> {
        let base = classes(env);
        for (x, prev) in load_backup() {
            let path = format!(r"{base}\.{x}");
            let Ok(key) = hkcu().open_subkey_with_flags(&path, winreg::enums::KEY_ALL_ACCESS) else {
                continue;
            };
            let ours = key.get_value::<String, _>("").ok().as_deref() == Some(PROG_ID);
            if let Ok(ow) = key.open_subkey_with_flags("OpenWithProgids", winreg::enums::KEY_ALL_ACCESS)
                && !prev.had_open_with
            {
                let _ = ow.delete_value(PROG_ID);
            }
            drop(key);
            if prev.absent {
                if ours {
                    let _ = hkcu().delete_subkey_all(&path);
                }
            } else if ours {
                let key = hkcu().create_subkey(&path).map_err(|e| e.to_string())?.0;
                match &prev.default {
                    Some(d) => key.set_value("", d).map_err(|e| e.to_string())?,
                    None => {
                        let _ = key.delete_value("");
                    }
                }
            }
        }
        let _ = hkcu().delete_subkey_all(format!(r"{base}\{PROG_ID}"));
        let _ = hkcu().delete_subkey_all(format!(r"{base}\Applications\Packrat.exe"));
        let _ = std::fs::remove_file(backup_path());
        notify(env);
        Ok(())
    }

    pub fn associated_exts(env: &Env) -> Vec<String> {
        let base = classes(env);
        ARCHIVE_EXTS
            .iter()
            .filter(|x| {
                hkcu()
                    .open_subkey(format!(r"{base}\.{x}"))
                    .ok()
                    .and_then(|k| k.get_value::<String, _>("").ok())
                    .as_deref()
                    == Some(PROG_ID)
            })
            .map(|x| (*x).to_owned())
            .collect()
    }

    /// Tells Explorer that associations changed (skipped for the sandbox registry branch).
    fn notify(env: &Env) {
        if env.software_key != "Software" {
            return;
        }
        use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
        // SAFETY: no pointers are passed.
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn register(_: &Env, _: &Path, _: i18n::Lang) -> Result<(), String> {
        Err("the Explorer menu needs Windows".into())
    }

    pub fn unregister(_: &Env) {}

    pub fn is_registered(_: &Env) -> bool {
        false
    }

    pub fn associate(_: &Env, _: &Path, _: &[&str]) -> Result<(), String> {
        Err("file associations need Windows".into())
    }

    pub fn restore_associations(_: &Env) -> Result<(), String> {
        Ok(())
    }

    pub fn associated_exts(_: &Env) -> Vec<String> {
        Vec::new()
    }
}

/// Installs the context menu (labels in `lang`).
pub fn register(env: &Env, exe: &Path, lang: i18n::Lang) -> Result<(), String> {
    imp::register(env, exe, lang)
}

pub fn unregister(env: &Env) {
    imp::unregister(env);
}

pub fn is_registered(env: &Env) -> bool {
    imp::is_registered(env)
}

/// Makes Packrat the handler of `exts` for this user, remembering what was there before.
pub fn associate(env: &Env, exe: &Path, exts: &[&str]) -> Result<(), String> {
    imp::associate(env, exe, exts)
}

/// Puts the associations back exactly as they were before `associate`.
pub fn restore_associations(env: &Env) -> Result<(), String> {
    imp::restore_associations(env)
}

pub fn associated_exts(env: &Env) -> Vec<String> {
    imp::associated_exts(env)
}

/// Called by the installer: the menu is part of a normal install.
pub fn register_if_enabled(env: &Env, exe: &Path) {
    let lang = i18n::lang();
    if let Err(e) = register(env, exe, lang) {
        crate::diag::warn(&format!("cannot add the Explorer menu: {e}"));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    fn sandbox() -> Env {
        settings::use_test_data_dir();
        Env {
            install_dir: PathBuf::from(r"C:\nowhere"),
            start_menu_dir: PathBuf::from(r"C:\nowhere"),
            software_key: format!(
                r"Software\PackratShellTest-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            )
            .replace(['(', ')'], ""),
        }
    }

    fn cleanup(env: &Env) {
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&env.software_key);
    }

    fn value(path: &str, name: &str) -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(path)
            .ok()?
            .get_value::<String, _>(name)
            .ok()
    }

    #[test]
    fn the_menu_is_written_read_back_and_removed() {
        let env = sandbox();
        let exe = PathBuf::from(r"C:\Program Files\Packrat\Packrat.exe");
        register(&env, &exe, i18n::Lang::En).unwrap();
        assert!(is_registered(&env));
        let base = classes(&env);

        let cmd = value(
            &format!(r"{base}\SystemFileAssociations\.zip\shell\Packrat\shell\10smart\command"),
            "",
        )
        .unwrap();
        assert_eq!(cmd, r#""C:\Program Files\Packrat\Packrat.exe" --extract-here "%1""#);
        assert_eq!(
            value(
                &format!(r"{base}\SystemFileAssociations\.7z\shell\Packrat\shell\30test"),
                "MUIVerb"
            )
            .unwrap(),
            "Test archive"
        );
        assert_eq!(
            value(&format!(r"{base}\*\shell\Packrat\shell\20zip\command"), "").unwrap(),
            format!("\"{}\" --quick zip \"%1\"", exe.display())
        );
        assert_eq!(
            value(&format!(r"{base}\Directory\shell\Packrat\shell\10add"), "MUIVerb").unwrap(),
            "Add to archive…"
        );
        assert_eq!(value(&format!(r"{base}\*\shell\Packrat"), "SubCommands").unwrap(), "");
        let applies = value(&format!(r"{base}\*\shell\Packrat"), "AppliesTo").unwrap();
        assert!(
            applies.starts_with("NOT (") && applies.contains(".zip") && applies.contains(".tzst"),
            "{applies}"
        );
        assert!(value(&format!(r"{base}\Directory\shell\Packrat"), "AppliesTo").is_none());

        // labels follow the language chosen at registration
        register(&env, &exe, i18n::Lang::Ru).unwrap();
        assert_eq!(
            value(
                &format!(r"{base}\SystemFileAssociations\.zip\shell\Packrat\shell\30test"),
                "MUIVerb"
            )
            .unwrap(),
            "Проверить архив"
        );

        unregister(&env);
        assert!(!is_registered(&env));
        assert!(
            RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey(format!(r"{base}\SystemFileAssociations\.zip\shell\Packrat"))
                .is_err()
        );
        cleanup(&env);
    }

    #[test]
    fn associations_are_applied_and_undone_exactly() {
        let env = sandbox();
        let base = classes(&env);
        let hk = RegKey::predef(HKEY_CURRENT_USER);
        // .zip already belongs to another program; .7z has no user-level key at all
        let (zip, _) = hk.create_subkey(format!(r"{base}\.zip")).unwrap();
        zip.set_value("", &"7-Zip.zip").unwrap();
        let exe = PathBuf::from(r"C:\Packrat\Packrat.exe");

        associate(&env, &exe, &["zip", "7z"]).unwrap();
        assert_eq!(value(&format!(r"{base}\.zip"), "").unwrap(), PROG_ID);
        assert_eq!(value(&format!(r"{base}\.7z"), "").unwrap(), PROG_ID);
        assert_eq!(associated_exts(&env), ["zip", "7z"]);
        assert!(
            value(&format!(r"{base}\{PROG_ID}\shell\open\command"), "")
                .unwrap()
                .contains(r#"Packrat.exe" "%1""#)
        );

        // applying twice must not overwrite the remembered originals
        associate(&env, &exe, &["zip", "7z"]).unwrap();

        restore_associations(&env).unwrap();
        assert_eq!(
            value(&format!(r"{base}\.zip"), "").unwrap(),
            "7-Zip.zip",
            "the previous handler is back"
        );
        assert!(
            hk.open_subkey(format!(r"{base}\.7z")).is_err(),
            "a key that did not exist is gone again"
        );
        assert!(hk.open_subkey(format!(r"{base}\{PROG_ID}")).is_err());
        assert!(associated_exts(&env).is_empty());
        cleanup(&env);
    }
}
