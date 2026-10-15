//! Format detection and the single entry point for reading any supported archive.

use crate::archive::{Archive, EntryReader, Kind};
use crate::codec::stream::{self, Compression};
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result};
use crate::options::OpenOptions;
use crate::sevenz::SevenZArchive;
use crate::source::{FileSource, SharedSource, SourceReader, read_exact_at};
use crate::tar::{TarArchive, looks_like_tar};
use crate::zip::ZipArchive;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

pub fn open(path: &Path) -> Result<Box<dyn Archive>> {
    open_with(path, &OpenOptions::default())
}

pub fn open_with(path: &Path, opts: &OpenOptions) -> Result<Box<dyn Archive>> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    open_source_with(Arc::new(FileSource::open(path)?), &name, opts)
}

pub fn open_source(src: SharedSource, name: &str) -> Result<Box<dyn Archive>> {
    open_source_with(src, name, &OpenOptions::default())
}

/// `name` is the archive's file name; it only breaks ties (a `.zip` that has data in front of it) and names
/// the single entry of a plain `.gz`.
pub fn open_source_with(src: SharedSource, name: &str, opts: &OpenOptions) -> Result<Box<dyn Archive>> {
    let mut head = [0u8; 512];
    let n = (src.len().min(512)) as usize;
    read_exact_at(&*src, 0, &mut head[..n])?;
    let head = &head[..n];
    let lower = name.to_ascii_lowercase();

    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") || head.starts_with(b"PK\x07\x08") {
        return Ok(Box::new(ZipArchive::from_source_with(src, opts)?));
    }
    if head.starts_with(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]) {
        return Ok(Box::new(SevenZArchive::from_source(src)?));
    }
    if head.starts_with(b"Rar!\x1a\x07") {
        return Err(Error::Unsupported(
            "RAR archives are not supported (the reference decoder's license forbids bundling it)".into(),
        ));
    }

    let c = Compression::detect(head);
    if c != Compression::None {
        let mut decoded = Vec::new();
        stream::reader(c, SourceReader::new(src.clone()))
            .take(512)
            .read_to_end(&mut decoded)?;
        if looks_like_tar(&decoded) {
            return Ok(Box::new(TarArchive::from_source(src, c)?));
        }
        return Ok(Box::new(SingleStreamArchive::new(src, c, name)));
    }
    if looks_like_tar(head) {
        return Ok(Box::new(TarArchive::from_source(src, Compression::None)?));
    }
    if lower.ends_with(".zip") {
        // data in front of the archive (self-extractors) is handled by the ZIP reader itself
        return Ok(Box::new(ZipArchive::from_source_with(src, opts)?));
    }
    Err(Error::Unsupported("unknown archive format".into()))
}

/// A bare `.gz` / `.bz2` / `.xz` / `.zst` / `.lz4` file: an archive with exactly one entry.
pub struct SingleStreamArchive {
    src: SharedSource,
    compression: Compression,
    entries: Vec<Entry>,
}

fn single_kind(c: Compression) -> Kind {
    match c {
        Compression::Gz => Kind::Gz,
        Compression::Bz2 => Kind::Bz2,
        Compression::Xz => Kind::Xz,
        Compression::Zst => Kind::Zst,
        Compression::Lz4 => Kind::Lz4,
        Compression::None => Kind::Tar,
    }
}

impl SingleStreamArchive {
    pub fn new(src: SharedSource, compression: Compression, archive_name: &str) -> Self {
        let ext = format!(".{}", compression.extension());
        let inner = match archive_name.strip_suffix(&ext) {
            Some(s) if !s.is_empty() => s.to_owned(),
            _ if archive_name.is_empty() => "data".to_owned(),
            _ => format!("{archive_name}.out"),
        };
        let mut e = Entry::new(0, inner, EntryKind::File);
        e.packed = Some(src.len());
        e.method = compression.extension().to_ascii_uppercase();
        if compression == Compression::Gz && src.len() >= 18 {
            // the gzip trailer holds the size modulo 2^32: right for everything under 4 GiB
            let mut t = [0u8; 4];
            if read_exact_at(&*src, src.len() - 4, &mut t).is_ok() {
                e.size = u32::from_le_bytes(t) as u64;
            }
        }
        Self {
            src,
            compression,
            entries: vec![e],
        }
    }
}

impl Archive for SingleStreamArchive {
    fn kind(&self) -> Kind {
        single_kind(self.compression)
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn open_entry(&self, index: usize) -> Result<EntryReader<'_>> {
        if index != 0 {
            return Err(Error::Other(format!("no entry #{index}")));
        }
        Ok(stream::reader(self.compression, SourceReader::new(self.src.clone())))
    }
}
