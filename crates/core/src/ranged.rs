//! Archives whose entries are plain byte ranges of the file (AR, CPIO, ISO): every entry opens directly.

use crate::archive::{Archive, EntryReader, Kind};
use crate::entry::Entry;
use crate::error::{Error, Result};
use crate::source::{SharedSource, SourceReader};

/// Upper bound on entries any parser here will collect.
pub(crate) const MAX_ENTRIES: usize = 1_000_000;

pub struct RangedArchive {
    kind: Kind,
    src: SharedSource,
    entries: Vec<Entry>,
    /// `(offset, length)` of each entry's data, indexed like `entries`.
    ranges: Vec<(u64, u64)>,
}

impl RangedArchive {
    pub(crate) fn new(kind: Kind, src: SharedSource, items: Vec<(Entry, (u64, u64))>) -> Self {
        let (entries, ranges) = items.into_iter().unzip();
        Self {
            kind,
            src,
            entries,
            ranges,
        }
    }
}

impl Archive for RangedArchive {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn open_entry(&self, index: usize) -> Result<EntryReader<'_>> {
        let &(start, len) = self
            .ranges
            .get(index)
            .ok_or_else(|| Error::Other(format!("no entry #{index}")))?;
        Ok(Box::new(SourceReader::window(self.src.clone(), start, len)))
    }
}
