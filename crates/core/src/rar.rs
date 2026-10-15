//! RAR 1.5 to 5.x archives (read only), through the unrar library. It works on files, so a RAR that sits inside
//! another archive has to be extracted first. Multi-volume sets open from any part.

use crate::archive::{Archive, EntryReader, Kind, open_via_walk};
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result};
use crate::options::{OpenOptions, Password};
use crate::ranged::MAX_ENTRIES;
use crate::timeutil::{dos_to_naive, local_to_utc};
use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use unrar::error::{Code, UnrarError};

/// Entries up to this size are decoded into memory; bigger ones go through a temporary file.
const IN_MEMORY: u64 = 64 << 20;

pub fn is_rar(head: &[u8]) -> bool {
    head.starts_with(b"Rar!\x1a\x07")
}

pub struct RarArchive {
    path: PathBuf,
    password: Option<Password>,
    entries: Vec<Entry>,
    solid: bool,
}

fn map_err(e: UnrarError, password_given: bool, encrypted: bool) -> Error {
    match e.code {
        Code::MissingPassword => Error::PasswordRequired,
        Code::BadPassword => Error::WrongPassword,
        // RAR 3 cannot tell a wrong password from damage: it only shows as a checksum error
        Code::BadData if password_given && encrypted => Error::WrongPassword,
        Code::BadData | Code::BadArchive => Error::Corrupt(format!("rar: {e}")),
        Code::UnknownFormat => Error::Unsupported(format!("rar: {e}")),
        Code::NoMemory => Error::Other("not enough memory".into()),
        _ => Error::Other(format!("rar: {e}")),
    }
}

fn normalized(name: &Path) -> String {
    name.to_string_lossy().replace('\\', "/")
}

fn unrar_archive<'a>(path: &'a PathBuf, password: &'a Option<String>) -> unrar::Archive<'a> {
    match password {
        Some(p) => unrar::Archive::with_password(path, p),
        None => unrar::Archive::new(path),
    }
}

impl RarArchive {
    pub fn open(path: &Path, opts: &OpenOptions) -> Result<Self> {
        let path = path.to_path_buf();
        let password = opts.password.clone();
        let pw = password.as_ref().map(|p| p.expose().to_owned());
        let given = password.is_some();
        let first = unrar_archive(&path, &pw).as_first_part();
        let listing = first
            .open_for_listing_split()
            .map_err(|e| map_err(e, given, true))?;
        let solid = listing.is_solid();
        let mut entries: Vec<Entry> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for header in listing {
            let h = header.map_err(|e| map_err(e, given, true))?;
            let name = normalized(&h.filename);
            // a file continued in the next volume is listed once per volume
            if !seen.insert(name.clone()) {
                continue;
            }
            if entries.len() >= MAX_ENTRIES {
                return Err(Error::Unsupported("too many entries".into()));
            }
            let kind = if h.is_directory() { EntryKind::Dir } else { EntryKind::File };
            let mut e = Entry::new(entries.len(), name, kind);
            e.size = h.unpacked_size;
            e.crc32 = (kind == EntryKind::File && h.file_crc != 0).then_some(h.file_crc);
            e.encrypted = h.is_encrypted();
            e.mtime = dos_to_naive((h.file_time >> 16) as u16, h.file_time as u16).map(local_to_utc);
            e.method = match h.method {
                0x30 => "stored".to_owned(),
                m @ 0x31..=0x35 => format!("m{}", m - 0x30),
                m => format!("method {m}"),
            };
            entries.push(e);
        }
        Ok(Self {
            path,
            password,
            entries,
            solid,
        })
    }
}

/// A temporary file that is gone once it has been read.
struct TempFile {
    file: std::fs::File,
    path: PathBuf,
}

impl Read for TempFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn temp_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "packrat-rar-{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

impl Archive for RarArchive {
    fn kind(&self) -> Kind {
        Kind::Rar
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn skip_cost(&self, index: usize) -> u64 {
        if self.solid {
            self.entries.iter().take(index).map(|e| e.size).sum()
        } else {
            0
        }
    }

    fn open_entry(&self, index: usize) -> Result<EntryReader<'_>> {
        open_via_walk(self, index)
    }

    fn walk(
        &self,
        want: &mut dyn FnMut(&Entry) -> bool,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let pw = self.password.as_ref().map(|p| p.expose().to_owned());
        let given = pw.is_some();
        let mut cursor = unrar_archive(&self.path, &pw)
            .as_first_part()
            .open_for_processing()
            .map_err(|e| map_err(e, given, true))?;
        let mut next = 0usize;
        while let Some(before) = cursor.read_header().map_err(|e| map_err(e, given, true))? {
            let name = normalized(&before.entry().filename);
            let Some(index) = (next..self.entries.len()).find(|&i| self.entries[i].path == name) else {
                cursor = before.skip().map_err(|e| map_err(e, given, true))?;
                continue;
            };
            next = index + 1;
            let entry = &self.entries[index];
            if !want(entry) {
                cursor = before.skip().map_err(|e| map_err(e, given, true))?;
                continue;
            }
            if entry.kind == EntryKind::Dir {
                visit(entry, &mut std::io::empty())?;
                cursor = before.skip().map_err(|e| map_err(e, given, true))?;
            } else if entry.size <= IN_MEMORY {
                let (data, after) = before.read().map_err(|e| map_err(e, given, entry.encrypted))?;
                cursor = after;
                visit(entry, &mut Cursor::new(data))?;
            } else {
                let tmp = temp_path();
                let after = before
                    .extract_to(&tmp)
                    .map_err(|e| {
                        let _ = std::fs::remove_file(&tmp);
                        map_err(e, given, entry.encrypted)
                    })?;
                cursor = after;
                let mut file = TempFile {
                    file: std::fs::File::open(&tmp)?,
                    path: tmp,
                };
                visit(entry, &mut file)?;
            }
        }
        Ok(())
    }
}
