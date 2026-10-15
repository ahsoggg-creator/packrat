//! Dragging files out of the archive window into Explorer (or any drop target).
//!
//! The selection is extracted to a temporary folder first; then `DoDragDrop` runs the drag with a data object
//! that offers those files as CF_HDROP, the format Explorer, browsers and chat apps all understand.
//! Temporary folders are removed on the next start.

use std::path::PathBuf;

pub const TEMP_PREFIX: &str = "drag-";

pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join("Packrat")
}

/// Removes drag folders left behind by earlier runs.
pub fn cleanup_stale() {
    let Ok(rd) = std::fs::read_dir(temp_root()) else { return };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

pub fn drag_dir() -> PathBuf {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    temp_root().join(format!("{TEMP_PREFIX}{}-{millis}", std::process::id()))
}

#[cfg(windows)]
mod imp {
    use std::mem::ManuallyDrop;
    use std::path::PathBuf;
    use windows::Win32::Foundation::{
        DATA_S_SAMEFORMATETC, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC,
        E_NOTIMPL, GlobalFree, HGLOBAL, OLE_E_ADVISENOTSUPPORTED, S_OK,
    };
    use windows::Win32::System::Com::{
        DVASPECT_CONTENT, FORMATETC, IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA,
        STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::{
        CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize,
    };
    use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
    use windows::Win32::UI::Shell::{DROPFILES, SHCreateStdEnumFmtEtc};
    use windows::core::{BOOL, HRESULT, Ref, implement};

    #[implement(IDropSource)]
    struct DropSource;

    impl IDropSource_Impl for DropSource_Impl {
        fn QueryContinueDrag(&self, escape_pressed: BOOL, key_state: MODIFIERKEYS_FLAGS) -> HRESULT {
            if escape_pressed.as_bool() {
                DRAGDROP_S_CANCEL
            } else if key_state.0 & MK_LBUTTON.0 == 0 {
                DRAGDROP_S_DROP
            } else {
                S_OK
            }
        }

        fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }

    /// A data object that offers exactly one format: the list of files (CF_HDROP).
    #[implement(IDataObject)]
    struct HDropData {
        paths: Vec<PathBuf>,
    }

    fn hdrop_format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn is_hdrop(f: *const FORMATETC) -> bool {
        // SAFETY: COM passes a valid FORMATETC pointer (or null, which is rejected).
        let Some(f) = (unsafe { f.as_ref() }) else { return false };
        f.cfFormat == CF_HDROP.0 && f.tymed & TYMED_HGLOBAL.0 as u32 != 0 && f.dwAspect == DVASPECT_CONTENT.0
    }

    /// `DROPFILES` header followed by NUL-separated UTF-16 paths and a closing double NUL.
    fn hdrop_bytes(paths: &[PathBuf]) -> Vec<u8> {
        let header = DROPFILES {
            pFiles: std::mem::size_of::<DROPFILES>() as u32,
            pt: Default::default(),
            fNC: BOOL(0),
            fWide: BOOL(1),
        };
        let mut out = Vec::new();
        // SAFETY: DROPFILES is a plain repr(C) struct.
        out.extend_from_slice(unsafe {
            std::slice::from_raw_parts(
                (&header as *const DROPFILES).cast::<u8>(),
                std::mem::size_of::<DROPFILES>(),
            )
        });
        for p in paths {
            for unit in p.to_string_lossy().encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
            out.extend_from_slice(&[0, 0]);
        }
        out.extend_from_slice(&[0, 0]);
        out
    }

    impl IDataObject_Impl for HDropData_Impl {
        fn GetData(&self, format: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
            if !is_hdrop(format) {
                return Err(DV_E_FORMATETC.into());
            }
            let bytes = hdrop_bytes(&self.paths);
            // SAFETY: the block is allocated, locked, filled within its size and unlocked; ownership passes to the caller.
            unsafe {
                let h: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len())?;
                let dst = GlobalLock(h);
                if dst.is_null() {
                    let _ = GlobalFree(Some(h));
                    return Err(windows::core::Error::from_thread());
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst.cast::<u8>(), bytes.len());
                let _ = GlobalUnlock(h);
                Ok(STGMEDIUM {
                    tymed: TYMED_HGLOBAL.0 as u32,
                    u: STGMEDIUM_0 { hGlobal: h },
                    pUnkForRelease: ManuallyDrop::new(None),
                })
            }
        }

        fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
            if is_hdrop(format) { S_OK } else { DV_E_FORMATETC }
        }

        fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, out: *mut FORMATETC) -> HRESULT {
            // SAFETY: `out` is a valid pointer supplied by the caller.
            if let Some(o) = unsafe { out.as_mut() } {
                o.ptd = std::ptr::null_mut();
            }
            DATA_S_SAMEFORMATETC
        }

        fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
            if direction != 1 {
                return Err(E_NOTIMPL.into()); // DATADIR_GET only
            }
            // SAFETY: plain call with a one-element slice.
            unsafe { SHCreateStdEnumFmtEtc(&[hdrop_format()]) }
        }

        fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<IAdviseSink>) -> windows::core::Result<u32> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }

        fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }

        fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }
    }

    pub fn data_object(paths: &[PathBuf]) -> IDataObject {
        HDropData { paths: paths.to_vec() }.into()
    }

    pub fn start(paths: &[PathBuf]) -> Result<(), String> {
        let obj = data_object(paths);
        let source: IDropSource = DropSource.into();
        let mut effect = DROPEFFECT(0);
        // SAFETY: OLE is initialised on this thread (a repeated call just returns S_FALSE); the objects outlive the call.
        let hr = unsafe {
            let _ = OleInitialize(None);
            DoDragDrop(&obj, &source, DROPEFFECT_COPY, &mut effect)
        };
        if hr.is_ok() || hr == DRAGDROP_S_DROP || hr == DRAGDROP_S_CANCEL {
            Ok(())
        } else {
            Err(format!("DoDragDrop failed: {hr:?}"))
        }
    }

    /// File paths stored in the CF_HDROP format of a data object, read the way a drop target reads them.
    #[cfg(test)]
    pub fn hdrop_paths(obj: &IDataObject) -> Vec<String> {
        // SAFETY: the HGLOBAL returned for CF_HDROP holds a DROPFILES header followed by the file list.
        unsafe {
            let fmt = hdrop_format();
            obj.QueryGetData(&fmt).ok().expect("format is accepted");
            let medium: STGMEDIUM = obj.GetData(&fmt).expect("CF_HDROP is offered");
            let hglobal = medium.u.hGlobal;
            let base = GlobalLock(hglobal) as *const u8;
            let header = &*(base as *const DROPFILES);
            let mut p = base.add(header.pFiles as usize) as *const u16;
            let mut out = Vec::new();
            loop {
                let mut len = 0;
                while *p.add(len) != 0 {
                    len += 1;
                }
                if len == 0 {
                    break;
                }
                out.push(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                p = p.add(len + 1);
            }
            let _ = GlobalUnlock(hglobal);
            let _ = GlobalFree(Some(hglobal));
            let en = obj.EnumFormatEtc(1).expect("enumerator");
            let mut f = [FORMATETC::default()];
            let mut n = 0u32;
            assert!(en.Next(&mut f, Some(&mut n)).is_ok() && n == 1 && f[0].cfFormat == CF_HDROP.0);
            out
        }
    }
}

/// Runs the drag with the given existing files. Blocks until the mouse button is released.
#[cfg(windows)]
pub fn start(paths: &[PathBuf]) -> Result<(), String> {
    imp::start(paths)
}

#[cfg(not(windows))]
pub fn start(_: &[PathBuf]) -> Result<(), String> {
    Err("drag and drop out of the window needs Windows".into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn data_object_offers_the_files_as_hdrop() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("файл а.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, "1").unwrap();
        std::fs::write(&b, "2").unwrap();
        let obj = imp::data_object(&[a.clone(), b.clone()]);
        let mut got = imp::hdrop_paths(&obj);
        got.sort();
        let mut want = vec![a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn stale_folders_are_cleaned() {
        let d = drag_dir();
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("x"), "1").unwrap();
        cleanup_stale();
        assert!(!d.exists());
    }
}
