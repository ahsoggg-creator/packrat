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
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    pub fn create_shortcut(_: &Path, _: &Path, _: &str, _: &str) -> Result<(), String> {
        Err("shortcuts are only available on Windows".into())
    }

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
}

pub fn create_shortcut(lnk: &Path, target: &Path, args: &str, description: &str) -> Result<(), String> {
    imp::create_shortcut(lnk, target, args, description)
}

#[cfg(test)]
pub fn read_shortcut_target(lnk: &Path) -> Option<String> {
    imp::read_shortcut_target(lnk)
}

pub fn console_print(text: &str) {
    imp::console_print(text)
}

pub fn show_in_explorer(path: &Path) {
    imp::show_in_explorer(path)
}

/// Native message box; returns false only when a Yes/No box was answered "No".
pub fn message_box(title: &str, text: &str, yes_no: bool) -> bool {
    imp::message_box(title, text, yes_no)
}
