//! Microsoft Cabinet files (`.cab`): MSZIP, LZX and Quantum, single cabinets only.

use crate::archive::{Archive, EntryReader, Kind, open_via_walk};
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result};
use crate::ranged::MAX_ENTRIES;
use crate::source::{SharedSource, SourceReader};
use crate::timeutil::local_to_utc;
use std::io::Read;
use std::sync::Mutex;

pub const MAGIC: &[u8] = b"MSCF";

pub struct CabArchive {
    cabinet: Mutex<cab::Cabinet<SourceReader>>,
    entries: Vec<Entry>,
    /// The names as stored, which is what the reader looks files up by.
    stored_names: Vec<String>,
}

fn method(c: cab::CompressionType) -> String {
    match c {
        cab::CompressionType::None => "stored".into(),
        cab::CompressionType::MsZip => "MSZIP".into(),
        cab::CompressionType::Quantum(..) => "Quantum".into(),
        cab::CompressionType::Lzx(_) => "LZX".into(),
    }
}

fn io_err(e: std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof => Error::Corrupt(format!("cab: {e}")),
        _ => Error::Io(e),
    }
}

impl CabArchive {
    pub fn from_source(src: SharedSource) -> Result<Self> {
        let cabinet = cab::Cabinet::new(SourceReader::new(src)).map_err(io_err)?;
        let mut entries = Vec::new();
        let mut stored_names = Vec::new();
        for folder in cabinet.folder_entries() {
            let m = method(folder.compression_type());
            for f in folder.file_entries() {
                if entries.len() >= MAX_ENTRIES {
                    return Err(Error::Unsupported("too many entries".into()));
                }
                let mut e = Entry::new(entries.len(), f.name().replace('\\', "/"), EntryKind::File);
                e.size = f.uncompressed_size() as u64;
                e.method = m.clone();
                e.mtime = f.datetime().map(|d| local_to_utc(d.assume_utc().unix_timestamp()));
                stored_names.push(f.name().to_owned());
                entries.push(e);
            }
        }
        Ok(Self {
            cabinet: Mutex::new(cabinet),
            entries,
            stored_names,
        })
    }
}

impl Archive for CabArchive {
    fn kind(&self) -> Kind {
        Kind::Cab
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn skip_cost(&self, index: usize) -> u64 {
        // a folder is decoded from its start up to the file
        self.entries.iter().take(index).map(|e| e.size).sum()
    }

    fn open_entry(&self, index: usize) -> Result<EntryReader<'_>> {
        open_via_walk(self, index)
    }

    fn walk(
        &self,
        want: &mut dyn FnMut(&Entry) -> bool,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let mut cabinet = self
            .cabinet
            .lock()
            .map_err(|_| Error::Other("cabinet reader poisoned".into()))?;
        for (e, name) in self.entries.iter().zip(&self.stored_names) {
            if !want(e) {
                continue;
            }
            let mut r = cabinet.read_file(name).map_err(io_err)?;
            visit(e, &mut r)?;
        }
        Ok(())
    }
}
