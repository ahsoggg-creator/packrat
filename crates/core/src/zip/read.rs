use super::extra;
use super::{METHOD_DEFLATE, METHOD_STORE, METHOD_ZSTD, SIG_CENTRAL, SIG_EOCD, SIG_EOCD64, SIG_LOC64, SIG_LOCAL};
use crate::archive::{Archive, EntryReader, Kind};
use crate::codec::zstd::ZstdReader;
use crate::encoding::decode_cp437;
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result, corrupt};
use crate::source::{FileSource, SharedSource, SourceReader, read_exact_at};
use crate::timeutil;
use std::io::{self, BufReader, Read};
use std::path::Path;
use std::sync::Arc;

const MAX_CENTRAL_DIRECTORY: u64 = 1 << 30;

#[derive(Clone, Debug)]
pub(crate) struct Meta {
    pub local_offset: u64,
    pub csize: u64,
    pub method: u16,
    pub flags: u16,
}

pub struct ZipArchive {
    src: SharedSource,
    entries: Vec<Entry>,
    meta: Vec<Meta>,
    comment: Vec<u8>,
    /// Absolute offset of the central directory: entry data must end before it.
    data_end: u64,
}

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).filter(|&e| e <= self.b.len());
        let Some(end) = end else {
            return corrupt("central directory is truncated");
        };
        let s = &self.b[self.p..end];
        self.p = end;
        Ok(s)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

struct EndRecord {
    entries: u64,
    cd_size: u64,
    cd_offset: u64,
    /// Offset of the structure that ends the central directory (EOCD or Zip64 EOCD).
    end_of_cd: u64,
    comment: Vec<u8>,
}

fn find_end_record(src: &dyn crate::source::Source) -> Result<EndRecord> {
    let len = src.len();
    if len < 22 {
        return corrupt("the file is too small to be a ZIP archive");
    }
    let tail_len = len.min(22 + 65_535) as usize;
    let tail_start = len - tail_len as u64;
    let mut tail = vec![0u8; tail_len];
    read_exact_at(src, tail_start, &mut tail)?;

    let sig = SIG_EOCD.to_le_bytes();
    let mut exact = None;
    let mut nearest = None;
    for i in (0..=tail_len - 22).rev() {
        if tail[i..i + 4] == sig {
            if i + 22 + le16(&tail, i + 20) as usize == tail_len {
                exact = Some(i);
                break;
            }
            nearest.get_or_insert(i);
        }
    }
    let Some(i) = exact.or(nearest) else {
        return corrupt("end of central directory record not found");
    };
    let eocd_abs = tail_start + i as u64;
    let comment_len = le16(&tail, i + 20) as usize;
    let comment = tail[i + 22..(i + 22 + comment_len).min(tail_len)].to_vec();

    let mut entries = le16(&tail, i + 10) as u64;
    let mut cd_size = le32(&tail, i + 12) as u64;
    let mut cd_offset = le32(&tail, i + 16) as u64;
    let mut end_of_cd = eocd_abs;

    if (entries == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF) && eocd_abs >= 20 {
        let mut loc = [0u8; 20];
        read_exact_at(src, eocd_abs - 20, &mut loc)?;
        if le32(&loc, 0) == SIG_LOC64 {
            let stated = le64(&loc, 8);
            let mut rec = [0u8; 56];
            let mut at = None;
            for cand in [Some(stated), eocd_abs.checked_sub(20 + 56)].into_iter().flatten() {
                if read_exact_at(src, cand, &mut rec).is_ok() && le32(&rec, 0) == SIG_EOCD64 {
                    at = Some(cand);
                    break;
                }
            }
            let Some(at) = at else {
                return corrupt("Zip64 end of central directory record not found");
            };
            entries = le64(&rec, 32);
            cd_size = le64(&rec, 40);
            cd_offset = le64(&rec, 48);
            end_of_cd = at;
        }
    }
    Ok(EndRecord {
        entries,
        cd_size,
        cd_offset,
        end_of_cd,
        comment,
    })
}

fn decode_name(raw: &[u8], flags: u16, extra_data: &[u8]) -> String {
    if let Some(n) = extra::unicode_path(extra_data, raw) {
        return n;
    }
    if flags & 0x0800 != 0 {
        return String::from_utf8_lossy(raw).into_owned();
    }
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_owned(),
        Err(_) => decode_cp437(raw),
    }
}

fn method_label(method: u16) -> String {
    match method {
        METHOD_STORE => "Store".into(),
        METHOD_DEFLATE => "Deflate".into(),
        METHOD_ZSTD => "Zstd".into(),
        12 => "BZip2".into(),
        14 => "LZMA".into(),
        99 => "AES".into(),
        n => format!("Method {n}"),
    }
}

impl ZipArchive {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_source(Arc::new(FileSource::open(path)?))
    }

    pub fn from_source(src: SharedSource) -> Result<Self> {
        let end = find_end_record(&*src)?;
        if end.cd_size > MAX_CENTRAL_DIRECTORY || end.cd_size > src.len() {
            return corrupt("central directory size is not plausible");
        }
        let Some(cd_start) = end.end_of_cd.checked_sub(end.cd_size) else {
            return corrupt("central directory does not fit in the file");
        };
        // Archives with data in front (self-extractors) store offsets relative to the archive start.
        let base = cd_start
            .checked_sub(end.cd_offset)
            .ok_or_else(|| Error::Corrupt("central directory offset is beyond the file".into()))?;
        if end.entries > end.cd_size / 46 {
            return corrupt("entry count does not match the central directory size");
        }
        let mut cd = vec![0u8; end.cd_size as usize];
        read_exact_at(&*src, cd_start, &mut cd)?;

        let mut cur = Cur { b: &cd, p: 0 };
        let mut entries = Vec::with_capacity(end.entries as usize);
        let mut meta = Vec::with_capacity(end.entries as usize);
        for index in 0..end.entries as usize {
            let (e, m) = parse_central_entry(&mut cur, index, base)?;
            entries.push(e);
            meta.push(m);
        }
        Ok(Self {
            src,
            entries,
            meta,
            comment: end.comment,
            data_end: cd_start,
        })
    }

    pub fn comment(&self) -> &[u8] {
        &self.comment
    }

    pub fn source(&self) -> &SharedSource {
        &self.src
    }

    /// Start of the compressed data of an entry, from its local header.
    pub(crate) fn data_start(&self, index: usize) -> Result<u64> {
        let m = &self.meta[index];
        let mut h = [0u8; 30];
        read_exact_at(&*self.src, m.local_offset, &mut h)
            .map_err(|_| Error::Corrupt("local header is beyond the file".into()))?;
        if le32(&h, 0) != SIG_LOCAL {
            return corrupt(format!("bad local header signature for '{}'", self.entries[index].path));
        }
        let start = m.local_offset + 30 + le16(&h, 26) as u64 + le16(&h, 28) as u64;
        match start.checked_add(m.csize) {
            Some(end) if end <= self.data_end => Ok(start),
            _ => corrupt(format!(
                "data of '{}' extends beyond the archive",
                self.entries[index].path
            )),
        }
    }
}

fn parse_central_entry(cur: &mut Cur<'_>, index: usize, base: u64) -> Result<(Entry, Meta)> {
    if cur.u32()? != SIG_CENTRAL {
        return corrupt("bad central directory entry signature");
    }
    let made_by = cur.u16()?;
    let _needed = cur.u16()?;
    let flags = cur.u16()?;
    let method = cur.u16()?;
    let dtime = cur.u16()?;
    let ddate = cur.u16()?;
    let crc = cur.u32()?;
    let csize32 = cur.u32()?;
    let usize32 = cur.u32()?;
    let nlen = cur.u16()? as usize;
    let elen = cur.u16()? as usize;
    let clen = cur.u16()? as usize;
    let disk = cur.u16()?;
    let _internal = cur.u16()?;
    let external = cur.u32()?;
    let loff32 = cur.u32()?;
    let raw = cur.take(nlen)?;
    let extra_data = cur.take(elen)?;
    cur.take(clen)?;

    let (mut size, mut csize, mut loff) = (usize32 as u64, csize32 as u64, loff32 as u64);
    if usize32 == u32::MAX || csize32 == u32::MAX || loff32 == u32::MAX || disk == u16::MAX {
        let z =
            extra::find(extra_data, extra::ID_ZIP64).ok_or_else(|| Error::Corrupt("Zip64 field is missing".into()))?;
        let mut zc = Cur { b: z, p: 0 };
        if usize32 == u32::MAX {
            size = zc.u64()?;
        }
        if csize32 == u32::MAX {
            csize = zc.u64()?;
        }
        if loff32 == u32::MAX {
            loff = zc.u64()?;
        }
    }

    let unix_mode = ((made_by >> 8) == 3).then_some(external >> 16).filter(|m| *m != 0);
    let is_dir_name = raw.ends_with(b"/") || raw.ends_with(b"\\");
    let kind = if is_dir_name {
        EntryKind::Dir
    } else if let Some(mode) = unix_mode {
        match mode & 0o170_000 {
            0o040_000 => EntryKind::Dir,
            0o120_000 => EntryKind::Symlink,
            0 | 0o100_000 => EntryKind::File,
            _ => EntryKind::Other,
        }
    } else if external & 0x10 != 0 && size == 0 {
        EntryKind::Dir
    } else {
        EntryKind::File
    };

    let mtime = extra::ext_mtime(extra_data)
        .or_else(|| extra::ntfs_mtime(extra_data))
        .or_else(|| timeutil::dos_to_naive(ddate, dtime).map(timeutil::local_to_utc));

    let mut e = Entry::new(index, decode_name(raw, flags, extra_data), kind);
    e.raw_name = raw.to_vec();
    e.size = size;
    e.packed = Some(csize);
    e.mtime = mtime;
    e.crc32 = Some(crc);
    e.encrypted = flags & 1 != 0;
    e.method = method_label(method);
    e.mode = unix_mode;
    let local_offset = loff
        .checked_add(base)
        .ok_or_else(|| Error::Corrupt("entry offset overflows".into()))?;
    Ok((
        e,
        Meta {
            local_offset,
            csize,
            method,
            flags,
        },
    ))
}

/// Ends a stream with a size and CRC check, and refuses to produce more than the declared size.
struct Verify<R: Read> {
    inner: io::Take<R>,
    name: String,
    expected: u64,
    crc: Option<u32>,
    hasher: crc32fast::Hasher,
    seen: u64,
}

impl<R: Read> Read for Verify<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n == 0 {
            if self.seen != self.expected {
                return Err(Error::Integrity(format!(
                    "'{}' is shorter than declared ({} of {} bytes)",
                    self.name, self.seen, self.expected
                ))
                .into());
            }
            if let Some(crc) = self.crc
                && self.hasher.clone().finalize() != crc
            {
                return Err(Error::Integrity(format!("CRC mismatch in '{}'", self.name)).into());
            }
            return Ok(0);
        }
        self.seen += n as u64;
        if self.seen > self.expected {
            return Err(Error::Integrity(format!("'{}' is larger than declared", self.name)).into());
        }
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

impl Archive for ZipArchive {
    fn kind(&self) -> Kind {
        Kind::Zip
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn open_entry(&self, index: usize) -> Result<EntryReader<'_>> {
        let e = self
            .entries
            .get(index)
            .ok_or_else(|| Error::Other(format!("no entry #{index}")))?;
        let m = &self.meta[index];
        if e.encrypted || m.flags & 1 != 0 || m.method == 99 {
            return Err(Error::Unsupported(format!("'{}' is encrypted", e.path)));
        }
        let start = self.data_start(index)?;
        let raw = SourceReader::window(self.src.clone(), start, m.csize);
        let decoded: EntryReader<'_> = match m.method {
            METHOD_STORE => Box::new(raw),
            METHOD_DEFLATE => Box::new(flate2::read::DeflateDecoder::new(BufReader::with_capacity(
                64 << 10,
                raw,
            ))),
            METHOD_ZSTD => Box::new(ZstdReader::new(BufReader::with_capacity(64 << 10, raw))),
            other => {
                return Err(Error::Unsupported(format!(
                    "compression method {} in '{}'",
                    method_label(other),
                    e.path
                )));
            }
        };
        Ok(Box::new(Verify {
            inner: decoded.take(e.size.saturating_add(1)),
            name: e.path.clone(),
            expected: e.size,
            crc: e.crc32,
            hasher: crc32fast::Hasher::new(),
            seen: 0,
        }))
    }
}
