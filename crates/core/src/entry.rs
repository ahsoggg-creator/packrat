#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    /// Devices, hard links, FIFOs: never extracted.
    Other,
}

/// One item of an archive as stored. `path` is exactly what the archive says (decoded to text, but not
/// sanitized); use `safepath` before touching the disk.
#[derive(Clone, Debug)]
pub struct Entry {
    pub index: usize,
    pub path: String,
    /// Name bytes as stored (ZIP only; empty for formats whose names are already text).
    pub raw_name: Vec<u8>,
    pub kind: EntryKind,
    /// Uncompressed size.
    pub size: u64,
    /// Compressed size when the format knows it per entry.
    pub packed: Option<u64>,
    /// Unix seconds (UTC).
    pub mtime: Option<i64>,
    pub crc32: Option<u32>,
    pub encrypted: bool,
    pub method: String,
    pub mode: Option<u32>,
    pub link_target: Option<String>,
}

impl Entry {
    pub fn new(index: usize, path: String, kind: EntryKind) -> Self {
        Self {
            index,
            path,
            raw_name: Vec::new(),
            kind,
            size: 0,
            packed: None,
            mtime: None,
            crc32: None,
            encrypted: false,
            method: String::new(),
            mode: None,
            link_target: None,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir
    }

    pub fn is_file(&self) -> bool {
        self.kind == EntryKind::File
    }

    /// Compressed / uncompressed in percent, when both are known and the entry is not empty.
    pub fn ratio_percent(&self) -> Option<f64> {
        let packed = self.packed?;
        (self.size > 0).then(|| packed as f64 * 100.0 / self.size as f64)
    }
}
