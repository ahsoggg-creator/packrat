use crate::entry::Entry;
use crate::error::Result;
use std::io::Read;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zip,
    SevenZ,
    Tar,
    TarGz,
    TarZst,
    TarXz,
    TarBz2,
    TarLz4,
    Gz,
    Bz2,
    Xz,
    Zst,
    Lz4,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Zip => "ZIP",
            Kind::SevenZ => "7z",
            Kind::Tar => "TAR",
            Kind::TarGz => "TAR.GZ",
            Kind::TarZst => "TAR.ZST",
            Kind::TarXz => "TAR.XZ",
            Kind::TarBz2 => "TAR.BZ2",
            Kind::TarLz4 => "TAR.LZ4",
            Kind::Gz => "GZ",
            Kind::Bz2 => "BZ2",
            Kind::Xz => "XZ",
            Kind::Zst => "ZST",
            Kind::Lz4 => "LZ4",
        }
    }

    /// True when entries can be opened independently (central directory), so extraction can use many
    /// threads and a preview does not have to read what comes before the entry.
    pub fn random_access(self) -> bool {
        matches!(self, Kind::Zip)
    }
}

pub type EntryReader<'a> = Box<dyn Read + Send + 'a>;

pub trait Archive: Send + Sync {
    fn kind(&self) -> Kind;
    fn entries(&self) -> &[Entry];

    /// ZIP names that had no encoding information (see `OpenOptions::encoding`).
    fn legacy_names(&self) -> Option<&crate::zip::read::LegacyNames> {
        None
    }

    /// Entries whose stored data overlaps another entry (ZIP only).
    fn overlapping_entries(&self) -> usize {
        0
    }

    /// Bytes that must be decoded and thrown away to reach entry `index` (0 when it can be opened directly).
    fn skip_cost(&self, _index: usize) -> u64 {
        0
    }

    fn random_access(&self) -> bool {
        self.kind().random_access()
    }

    /// Opens one entry for reading. For streaming formats this decodes everything before the entry, so
    /// callers that want many entries should use `walk`.
    fn open_entry(&self, index: usize) -> Result<EntryReader<'_>>;

    /// Visits the wanted entries in archive order with a reader for each. Streaming formats do this in a
    /// single pass.
    fn walk(
        &self,
        want: &mut dyn FnMut(&Entry) -> bool,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        for e in self.entries() {
            if want(e) {
                let mut r = self.open_entry(e.index)?;
                visit(e, &mut r)?;
            }
        }
        Ok(())
    }
}
