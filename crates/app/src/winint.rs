//! Small Windows integrations: shortcuts, console output for GUI-subsystem builds, Explorer.

use std::path::Path;

#[cfg(windows)]
mod imp {
    use std::io::Write;
    use std::path::Path;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{Interface, PCWSTR};

    fn wide(p: &str) -> Vec<u16> {
        p.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn create_shortcut(lnk: &Path, target: &Path, args: &str, description: &str) -> Result<(), String> {
        let err = |e: windows::core::Error| e.to_string();
        // SAFETY: plain COM calls on a link object created and used on this thread.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(err)?;
            let target_w = wide(&target.to_string_lossy());
            link.SetPath(PCWSTR(target_w.as_ptr())).map_err(err)?;
            let args_w = wide(args);
            link.SetArguments(PCWSTR(args_w.as_ptr())).map_err(err)?;
            let desc_w = wide(description);
            link.SetDescription(PCWSTR(desc_w.as_ptr())).map_err(err)?;
            if let Some(dir) = target.parent() {
                let dir_w = wide(&dir.to_string_lossy());
                link.SetWorkingDirectory(PCWSTR(dir_w.as_ptr())).map_err(err)?;
            }
            link.SetIconLocation(PCWSTR(target_w.as_ptr()), 0).map_err(err)?;
            if let Some(parent) = lnk.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let file: IPersistFile = link.cast().map_err(err)?;
            let lnk_w = wide(&lnk.to_string_lossy());
            file.Save(PCWSTR(lnk_w.as_ptr()), true).map_err(err)
        }
    }

    /// Moves a file to the Recycle Bin (no prompts).
    pub fn recycle(path: &Path) -> Result<(), String> {
        use windows::Win32::UI::Shell::{
            FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING,
            SHFILEOPSTRUCTW, SHFileOperationW,
        };
        if !path.is_absolute() {
            return Err("not an absolute path".into());
        }
        // the list of names ends with an extra NUL
        let from: Vec<u16> = path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain([0, 0])
            .collect();
        let mut op = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: PCWSTR(from.as_ptr()),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT | FOF_WANTNUKEWARNING).0 as u16,
            ..Default::default()
        };
        // SAFETY: `from` is NUL-terminated twice and outlives the call.
        let code = unsafe { SHFileOperationW(&mut op) };
        if code != 0 || op.fAnyOperationsAborted.as_bool() {
            return Err(format!("code {code:#x}"));
        }
        Ok(())
    }

    /// Ends Explorer; Windows starts the shell again by itself.
    pub fn restart_explorer() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("taskkill")
            .args(["/f", "/im", "explorer.exe"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }

    #[cfg(test)]
    pub fn read_shortcut_target(lnk: &Path) -> Option<String> {
        use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
        use windows::Win32::System::Com::STGM_READ;
        // SAFETY: as above; the buffer outlives the call.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let file: IPersistFile = link.cast().ok()?;
            let lnk_w = wide(&lnk.to_string_lossy());
            file.Load(PCWSTR(lnk_w.as_ptr()), STGM_READ).ok()?;
            let mut buf = [0u16; 520];
            let mut fd = WIN32_FIND_DATAW::default();
            link.GetPath(&mut buf, &mut fd, 0).ok()?;
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..len]))
        }
    }

    /// Prints to stdout when it is redirected; otherwise to the parent console of a GUI-subsystem process
    /// (a no-op when there is neither).
    pub fn console_print(text: &str) {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        let mut out = std::io::stdout();
        if out.write_all(text.as_bytes()).and_then(|()| out.flush()).is_ok() {
            return;
        }
        // SAFETY: AttachConsole has no memory-safety preconditions.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            let _ = f.write_all(text.as_bytes());
        }
    }

    pub fn message_box(title: &str, text: &str, yes_no: bool) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONINFORMATION, MB_OK, MB_YESNO, MessageBoxW};
        let (t, x) = (wide(title), wide(text));
        let style = if yes_no {
            MB_YESNO | MB_ICONINFORMATION
        } else {
            MB_OK | MB_ICONINFORMATION
        };
        // SAFETY: both buffers are NUL-terminated and live for the call.
        let r = unsafe { MessageBoxW(None, PCWSTR(x.as_ptr()), PCWSTR(t.as_ptr()), style) };
        r == IDYES || !yes_no
    }

    pub fn show_in_explorer(path: &Path) {
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }

    /// Runs `exe` with the "runas" verb: Windows shows the UAC prompt.
    pub fn run_elevated(exe: &Path, args: &str) -> Result<(), String> {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let (verb, file, params) = (wide("runas"), wide(&exe.to_string_lossy()), wide(args));
        // SAFETY: all strings are NUL-terminated and live for the call.
        let h = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR(params.as_ptr()),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // values above 32 mean success
        if h.0 as usize > 32 {
            Ok(())
        } else {
            Err(format!(
                "the elevated window could not be started (code {})",
                h.0 as usize
            ))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    pub fn create_shortcut(_: &Path, _: &Path, _: &str, _: &str) -> Result<(), String> {
        Err("shortcuts are only available on Windows".into())
    }

    pub fn recycle(_: &Path) -> Result<(), String> {
        Err("the Recycle Bin is only available on Windows".into())
    }

    pub fn restart_explorer() {}

    #[cfg(test)]
    pub fn read_shortcut_target(_: &Path) -> Option<String> {
        None
    }

    pub fn console_print(text: &str) {
        print!("{text}");
    }

    pub fn message_box(title: &str, text: &str, _yes_no: bool) -> bool {
        println!("{title}: {text}");
        true
    }

    pub fn show_in_explorer(_: &Path) {}

    pub fn run_elevated(_: &Path, _: &str) -> Result<(), String> {
        Err("elevation needs Windows".into())
    }
}

pub fn create_shortcut(lnk: &Path, target: &Path, args: &str, description: &str) -> Result<(), String> {
    imp::create_shortcut(lnk, target, args, description)
}

#[cfg(test)]
pub fn read_shortcut_target(lnk: &Path) -> Option<String> {
    imp::read_shortcut_target(lnk)
}

/// Moves a file to the Recycle Bin, so that a mistake can be undone.
pub fn recycle(path: &Path) -> Result<(), String> {
    imp::recycle(path)
}

pub fn restart_explorer() {
    imp::restart_explorer()
}

pub fn console_print(text: &str) {
    imp::console_print(text)
}

pub fn show_in_explorer(path: &Path) {
    imp::show_in_explorer(path)
}

/// Starts a program with administrator rights (one UAC prompt).
pub fn run_elevated(exe: &Path, args: &str) -> Result<(), String> {
    imp::run_elevated(exe, args)
}

/// Native message box; returns false only when a Yes/No box was answered "No".
pub fn message_box(title: &str, text: &str, yes_no: bool) -> bool {
    imp::message_box(title, text, yes_no)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn recycle_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("packrat-recycle-test.txt");
        std::fs::write(&f, b"x").unwrap();
        recycle(&f).unwrap();
        assert!(!f.exists());
        assert!(recycle(&dir.path().join("missing.txt")).is_err() || !dir.path().join("missing.txt").exists());
    }
}
