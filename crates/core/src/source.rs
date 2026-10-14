//! Random-access byte sources. Everything that reads an archive goes through `Source`, so reads can be
//! counted (tests prove that a preview does not read the whole archive) and shared between threads
//! without seek races.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub trait Source: Send + Sync {
    fn len(&self) -> u64;
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct FileSource {
    file: File,
    len: u64,
}

impl FileSource {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(windows)]
        {
            std::os::windows::fs::FileExt::seek_read(&self.file, buf, offset)
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_at(&self.file, buf, offset)
        }
    }
}

pub struct MemSource(pub Vec<u8>);

impl Source for MemSource {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let Ok(start) = usize::try_from(offset) else {
            return Ok(0);
        };
        if start >= self.0.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.0.len() - start);
        buf[..n].copy_from_slice(&self.0[start..start + n]);
        Ok(n)
    }
}

/// Wraps a source and counts the bytes that were actually read from it.
pub struct CountingSource<S: Source> {
    inner: S,
    bytes: AtomicU64,
    reads: AtomicU64,
}

impl<S: Source> CountingSource<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            bytes: AtomicU64::new(0),
            reads: AtomicU64::new(0),
        }
    }

    pub fn bytes_read(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub fn read_calls(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.bytes.store(0, Ordering::Relaxed);
        self.reads.store(0, Ordering::Relaxed);
    }
}

impl<S: Source> Source for CountingSource<S> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read_at(offset, buf)?;
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(n)
    }
}

impl<S: Source + ?Sized> Source for Arc<S> {
    fn len(&self) -> u64 {
        (**self).len()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read_at(offset, buf)
    }
}

pub type SharedSource = Arc<dyn Source>;

pub fn read_exact_at(src: &dyn Source, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
    while !buf.is_empty() {
        match src.read_at(offset, buf) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                offset += n as u64;
                buf = &mut buf[n..];
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `Read + Seek` view of a source (or of a window of it). Cheap to create: one per entry / per thread.
#[derive(Clone)]
pub struct SourceReader {
    src: SharedSource,
    start: u64,
    end: u64,
    pos: u64,
}

impl SourceReader {
    pub fn new(src: SharedSource) -> Self {
        let end = src.len();
        Self {
            src,
            start: 0,
            end,
            pos: 0,
        }
    }

    /// A reader limited to `[start, start + len)`; seek positions are relative to `start`.
    pub fn window(src: SharedSource, start: u64, len: u64) -> Self {
        let end = start.saturating_add(len).min(src.len());
        Self {
            src,
            start,
            end,
            pos: start,
        }
    }
}

impl Read for SourceReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.end || buf.is_empty() {
            return Ok(0);
        }
        let max = (self.end - self.pos).min(buf.len() as u64) as usize;
        let n = self.src.read_at(self.pos, &mut buf[..max])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for SourceReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let len = (self.end - self.start) as i128;
        let cur = (self.pos - self.start) as i128;
        let target = match from {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::End(d) => len + d as i128,
            SeekFrom::Current(d) => cur + d as i128,
        };
        if target < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = self.start + target as u64;
        Ok(target as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_and_windows() {
        let src = Arc::new(CountingSource::new(MemSource((0..=255u8).collect())));
        let shared: SharedSource = src.clone();
        let mut r = SourceReader::window(shared, 10, 20);
        let mut buf = Vec::new();
        r.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, (10..30u8).collect::<Vec<_>>());
        assert_eq!(src.bytes_read(), 20);
        assert_eq!(r.seek(SeekFrom::End(-5)).unwrap(), 15);
        let mut b = [0u8; 8];
        assert_eq!(r.read(&mut b).unwrap(), 5);
        assert_eq!(b[0], 25);
    }
}
