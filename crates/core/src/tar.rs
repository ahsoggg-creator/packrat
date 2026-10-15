//! TAR, plain or inside gzip / bzip2 / xz / zstd / lz4. Listing needs one pass over the whole (decompressed)
//! stream, extraction another.

use crate::archive::{Archive, EntryReader, Kind};
use crate::codec::stream::{self, Compression, FinishWrite};
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result};
use crate::input::InputItem;
use crate::progress::Progress;
use crate::source::{SharedSource, SourceReader};
use std::io::{self, Read};
use std::path::Path;

/// Largest entry `open_entry` will buffer for a streaming format.
const MAX_BUFFERED_ENTRY: u64 = 512 << 20;

pub struct TarArchive {
    src: SharedSource,
    compression: Compression,
    entries: Vec<Entry>,
}

fn tar_kind(c: Compression) -> Kind {
    match c {
        Compression::None => Kind::Tar,
        Compression::Gz => Kind::TarGz,
        Compression::Bz2 => Kind::TarBz2,
        Compression::Xz => Kind::TarXz,
        Compression::Zst => Kind::TarZst,
        Compression::Lz4 => Kind::TarLz4,
    }
}

/// True when `head` (at least 512 bytes of the decoded stream) starts with a valid TAR header.
pub fn looks_like_tar(head: &[u8]) -> bool {
    if head.len() < 512 {
        return false;
    }
    if &head[257..262] == b"ustar" {
        return true;
    }
    // old V7 headers: check the checksum field
    let stored = std::str::from_utf8(&head[148..156])
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_matches(|c: char| c == '\0' || c == ' '), 8).ok());
    let Some(stored) = stored else { return false };
    let sum: u32 = head[..512]
        .iter()
        .enumerate()
        .map(|(i, &b)| if (148..156).contains(&i) { 32 } else { b as u32 })
        .sum();
    sum == stored && head[0] != 0
}

impl TarArchive {
    pub fn from_source(src: SharedSource, compression: Compression) -> Result<Self> {
        let mut a = Self {
            src,
            compression,
            entries: Vec::new(),
        };
        a.entries = a.scan()?;
        Ok(a)
    }

    fn stream(&self) -> Box<dyn Read + Send + '_> {
        stream::reader(self.compression, SourceReader::new(self.src.clone()))
    }

    fn scan(&self) -> Result<Vec<Entry>> {
        let mut archive = tar::Archive::new(self.stream());
        let mut out = Vec::new();
        for item in archive.entries().map_err(Error::from)? {
            let e = item.map_err(|e| Error::Corrupt(format!("tar: {e}")))?;
            out.push(describe(out.len(), &e));
        }
        Ok(out)
    }
}

fn describe<R: Read>(index: usize, e: &tar::Entry<'_, R>) -> Entry {
    use tar::EntryType as T;
    let path = String::from_utf8_lossy(&e.path_bytes()).into_owned();
    let t = e.header().entry_type();
    let kind = match t {
        T::Regular | T::Continuous | T::GNUSparse => EntryKind::File,
        T::Directory => EntryKind::Dir,
        T::Symlink => EntryKind::Symlink,
        _ => EntryKind::Other,
    };
    let mut entry = Entry::new(index, path, kind);
    entry.size = e.size();
    entry.mtime = e.header().mtime().ok().map(|t| t as i64);
    entry.mode = e.header().mode().ok();
    entry.method = "TAR".into();
    entry.link_target = e.link_name_bytes().map(|b| String::from_utf8_lossy(&b).into_owned());
    entry
}

impl Archive for TarArchive {
    fn kind(&self) -> Kind {
        tar_kind(self.compression)
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn skip_cost(&self, index: usize) -> u64 {
        self.entries.iter().take(index).map(|e| e.size).sum()
    }

    fn open_entry(&self, index: usize) -> Result<EntryReader<'_>> {
        let e = self
            .entries
            .get(index)
            .ok_or_else(|| Error::Other(format!("no entry #{index}")))?;
        if e.size > MAX_BUFFERED_ENTRY {
            return Err(Error::Unsupported(format!(
                "'{}' is too large to open from a streaming archive",
                e.path
            )));
        }
        let mut data = Vec::with_capacity(e.size as usize);
        let mut found = false;
        self.walk(&mut |x| x.index == index, &mut |_, r| {
            r.read_to_end(&mut data)?;
            found = true;
            Ok(())
        })?;
        if !found {
            return Err(Error::Corrupt("entry disappeared from the archive".into()));
        }
        Ok(Box::new(io::Cursor::new(data)))
    }

    fn walk(
        &self,
        want: &mut dyn FnMut(&Entry) -> bool,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let mut archive = tar::Archive::new(self.stream());
        for (i, item) in archive.entries().map_err(Error::from)?.enumerate() {
            let mut e = item.map_err(|e| Error::Corrupt(format!("tar: {e}")))?;
            let Some(entry) = self.entries.get(i) else { break };
            if want(entry) {
                visit(entry, &mut e)?;
            }
        }
        // read the stream to its end: the checksum of a compressed stream sits after the TAR end marker
        let mut rest = archive.into_inner();
        io::copy(&mut rest, &mut io::sink()).map_err(Error::from)?;
        Ok(())
    }
}

/// Writes `items` as a TAR stream into `out`.
pub fn write_tar(out: Box<dyn FinishWrite>, items: &[InputItem], progress: &Progress) -> Result<()> {
    progress.set_totals(
        items.iter().filter(|i| !i.is_dir).count() as u64,
        items.iter().map(|i| i.size).sum(),
    );
    let mut builder = tar::Builder::new(out);
    builder.mode(tar::HeaderMode::Deterministic);
    for item in items {
        progress.check()?;
        progress.set_current(&item.name);
        let mut h = tar::Header::new_gnu();
        h.set_mtime(item.mtime.unwrap_or(0).max(0) as u64);
        if item.is_dir {
            h.set_entry_type(tar::EntryType::Directory);
            h.set_mode(0o755);
            h.set_size(0);
            builder.append_data(&mut h, item.name.trim_end_matches('/'), io::empty())?;
        } else {
            let file = std::fs::File::open(&item.disk_path)
                .map_err(|e| Error::Other(format!("cannot read '{}': {e}", item.disk_path.display())))?;
            // the size in the header must match what is written, even if the file changes meanwhile
            let size = file.metadata()?.len();
            h.set_entry_type(tar::EntryType::Regular);
            h.set_mode(0o644);
            h.set_size(size);
            builder.append_data(
                &mut h,
                &item.name,
                ProgressReader {
                    inner: file.take(size),
                    progress,
                },
            )?;
            progress.add_file();
        }
    }
    let out = builder.into_inner()?;
    out.finish()
}

struct ProgressReader<'a, R: Read> {
    inner: R,
    progress: &'a Progress,
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.progress.is_cancelled() {
            return Err(Error::Cancelled.into());
        }
        let n = self.inner.read(buf)?;
        self.progress.add_bytes(n as u64);
        Ok(n)
    }
}

pub fn create_file_writer(
    path: &Path,
    compression: Compression,
    level: u32,
    threads: usize,
) -> Result<Box<dyn FinishWrite>> {
    let file = std::fs::File::create(path)?;
    stream::writer(
        compression,
        io::BufWriter::with_capacity(256 << 10, file),
        level,
        threads,
    )
}
