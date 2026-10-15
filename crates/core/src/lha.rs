//! LHA / LZH archives (`.lzh`, `.lha`): the `-lh0-` to `-lh7-` and `-lzs-` / `-lz5-` methods.

use crate::archive::{Archive, EntryReader, Kind, open_via_walk};
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result};
use crate::ranged::MAX_ENTRIES;
use crate::source::{SharedSource, SourceReader};
use crate::timeutil::local_to_utc;
use delharc::LhaDecodeReader;
use delharc::header::TimestampResult;
use std::io::Read;

pub struct LhaArchive {
    src: SharedSource,
    entries: Vec<Entry>,
}

/// LHA headers carry the method as `-lh5-`: that is the one place a file can be told from other formats.
pub fn is_lha(head: &[u8]) -> bool {
    head.len() >= 7 && head[2] == b'-' && head[6] == b'-' && matches!(&head[3..5], b"lh" | b"lz" | b"pm")
}

fn fail(e: impl std::fmt::Display) -> Error {
    Error::Corrupt(format!("lha: {e}"))
}

fn describe(index: usize, h: &delharc::LhaHeader) -> Entry {
    let kind = if h.is_directory() {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    let mut e = Entry::new(index, h.parse_pathname_to_str(), kind);
    e.size = h.original_size;
    e.packed = Some(h.compressed_size);
    e.method = String::from_utf8_lossy(&h.compression).trim_matches('-').to_owned();
    e.mtime = match h.parse_last_modified() {
        TimestampResult::Utc(t) => Some(t.timestamp()),
        TimestampResult::Naive(t) => Some(local_to_utc(t.and_utc().timestamp())),
        TimestampResult::None => None,
    };
    e
}

impl LhaArchive {
    pub fn from_source(src: SharedSource) -> Result<Self> {
        let mut reader = LhaDecodeReader::new(SourceReader::new(src.clone())).map_err(fail)?;
        let mut entries = Vec::new();
        loop {
            if entries.len() >= MAX_ENTRIES {
                return Err(Error::Unsupported("too many entries".into()));
            }
            entries.push(describe(entries.len(), reader.header()));
            if !reader.seek_next_file().map_err(fail)? {
                break;
            }
        }
        Ok(Self { src, entries })
    }
}

impl Archive for LhaArchive {
    fn kind(&self) -> Kind {
        Kind::Lzh
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn skip_cost(&self, _index: usize) -> u64 {
        0
    }

    fn open_entry(&self, index: usize) -> Result<EntryReader<'_>> {
        open_via_walk(self, index)
    }

    fn walk(
        &self,
        want: &mut dyn FnMut(&Entry) -> bool,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let mut reader = LhaDecodeReader::new(SourceReader::new(self.src.clone())).map_err(fail)?;
        for entry in &self.entries {
            if want(entry) {
                if entry.kind == EntryKind::Dir {
                    visit(entry, &mut std::io::empty())?;
                } else if !reader.is_decoder_supported() {
                    return Err(Error::Unsupported(format!(
                        "LHA method '{}' of '{}' is not supported",
                        entry.method, entry.path
                    )));
                } else {
                    visit(entry, &mut reader)?;
                    if reader.is_empty() {
                        reader
                            .crc_check()
                            .map_err(|_| Error::Integrity(format!("{}: LHA checksum mismatch", entry.path)))?;
                    }
                }
            }
            if !reader.seek_next_file().map_err(fail)? {
                break;
            }
        }
        Ok(())
    }
}
