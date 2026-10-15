//! CPIO inside a compressed stream: `.cpio.gz` / `.cpio.xz` / `.cpio.zst` (initramfs images) and the payload of
//! `.rpm` packages. Like TAR, such an archive can only be read front to back.

use crate::archive::{Archive, EntryReader, Kind, open_via_walk};
use crate::codec::stream::{self, Compression};
use crate::cpio::{Header, entry_kind, is_cpio, parse_header};
use crate::encoding::detect;
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result, corrupt};
use crate::options::OpenOptions;
use crate::ranged::MAX_ENTRIES;
use crate::source::{SharedSource, SourceReader, read_exact_at};
use std::io::{self, Read};

pub const RPM_MAGIC: &[u8] = b"\xed\xab\xee\xdb";

pub struct CpioStreamArchive {
    src: SharedSource,
    start: u64,
    compression: Compression,
    kind: Kind,
    entries: Vec<Entry>,
}

/// Receives a member's name bytes, its header and its data.
type Visitor<'a> = dyn FnMut(&[u8], &Header, &mut dyn Read) -> Result<()> + 'a;

/// Calls `f` for every member in order; what `f` leaves unread is skipped.
fn each_member(reader: &mut dyn Read, f: &mut Visitor) -> Result<()> {
    loop {
        let mut magic = [0u8; 6];
        reader
            .read_exact(&mut magic)
            .map_err(|_| Error::Corrupt("cpio: stream ends without a trailer".into()))?;
        let newc = match &magic {
            b"070701" | b"070702" => true,
            b"070707" => false,
            _ => return corrupt("cpio: bad header magic"),
        };
        let header_len = if newc { 110 } else { 76 };
        let mut h = vec![0u8; header_len];
        h[..6].copy_from_slice(&magic);
        reader
            .read_exact(&mut h[6..])
            .map_err(|_| Error::Corrupt("cpio: truncated header".into()))?;
        let hdr = parse_header(&h, newc)?;
        let mut name = vec![0u8; hdr.name_len as usize];
        reader
            .read_exact(&mut name)
            .map_err(|_| Error::Corrupt("cpio: truncated name".into()))?;
        let pad = |n: u64| if newc { (4 - n % 4) % 4 } else { 0 };
        io::copy(
            &mut (&mut *reader).take(pad(header_len as u64 + hdr.name_len)),
            &mut io::sink(),
        )?;
        let name: Vec<u8> = name.split(|&c| c == 0).next().unwrap_or_default().to_vec();
        if name == b"TRAILER!!!" {
            // the checksum of a compressed stream sits after the end marker
            io::copy(reader, &mut io::sink())?;
            return Ok(());
        }
        let mut data = (&mut *reader).take(hdr.size);
        f(&name, &hdr, &mut data)?;
        io::copy(&mut data, &mut io::sink())?;
        io::copy(&mut (&mut *reader).take(pad(hdr.size)), &mut io::sink())?;
    }
}

fn path_of(name: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(name);
    let p = s.trim_start_matches("./");
    (!p.is_empty() && p != ".").then(|| p.to_owned())
}

impl CpioStreamArchive {
    pub fn from_source(
        src: SharedSource,
        start: u64,
        compression: Compression,
        kind: Kind,
        opts: &OpenOptions,
    ) -> Result<Self> {
        let mut a = Self {
            src,
            start,
            compression,
            kind,
            entries: Vec::new(),
        };
        let mut raw_names: Vec<Vec<u8>> = Vec::new();
        let mut entries: Vec<Entry> = Vec::new();
        each_member(&mut *a.stream(), &mut |name, h, data| {
            let Some(path) = path_of(name) else { return Ok(()) };
            if entries.len() >= MAX_ENTRIES {
                return Err(Error::Unsupported("too many entries".into()));
            }
            let kind = entry_kind(h.mode);
            let mut e = Entry::new(entries.len(), path, kind);
            e.size = h.size;
            e.mtime = Some(h.mtime as i64);
            e.mode = Some((h.mode & 0o7777) as u32);
            e.method = a.compression.extension().to_owned();
            if kind == EntryKind::Symlink && h.size <= 4096 {
                let mut t = Vec::new();
                data.read_to_end(&mut t)?;
                e.link_target = Some(String::from_utf8_lossy(&t).into_owned());
            }
            entries.push(e);
            raw_names.push(name.to_vec());
            Ok(())
        })?;
        let legacy: Vec<&[u8]> = raw_names
            .iter()
            .filter(|n| std::str::from_utf8(n).is_err())
            .map(Vec::as_slice)
            .collect();
        if !legacy.is_empty() {
            let enc = opts.encoding.unwrap_or_else(|| detect(&legacy).best);
            for (e, raw) in entries.iter_mut().zip(&raw_names) {
                if std::str::from_utf8(raw).is_err() {
                    e.path = path_of(enc.decode(raw).0.as_bytes()).unwrap_or_default();
                }
            }
        }
        a.entries = entries;
        Ok(a)
    }

    fn stream(&self) -> Box<dyn Read + Send + '_> {
        let len = self.src.len().saturating_sub(self.start);
        stream::reader(
            self.compression,
            SourceReader::window(self.src.clone(), self.start, len),
        )
    }

    /// An `.rpm`: lead, signature header, header, then the compressed cpio payload.
    pub fn open_rpm(src: SharedSource, opts: &OpenOptions) -> Result<Self> {
        const MAX_HEADER: u64 = 64 << 20;
        let mut pos = 96u64;
        for after_signature in [true, false] {
            let mut h = [0u8; 16];
            read_exact_at(&*src, pos, &mut h).map_err(|_| Error::Corrupt("rpm: truncated header".into()))?;
            if h[..4] != [0x8e, 0xad, 0xe8, 0x01] {
                return corrupt("rpm: bad header magic");
            }
            let count = u32::from_be_bytes([h[8], h[9], h[10], h[11]]) as u64;
            let size = u32::from_be_bytes([h[12], h[13], h[14], h[15]]) as u64;
            if count > 1 << 20 || size > MAX_HEADER {
                return corrupt("rpm: implausible header size");
            }
            pos += 16 + count * 16 + size;
            if after_signature {
                pos = (pos + 7) & !7; // the signature header is padded to 8 bytes
            }
        }
        let mut head = [0u8; 8];
        let n = src.len().saturating_sub(pos).min(8) as usize;
        read_exact_at(&*src, pos, &mut head[..n])?;
        let compression = Compression::detect(&head[..n]);
        if compression == Compression::None && !is_cpio(&head[..n]) {
            return Err(Error::Unsupported(
                "this RPM has a payload that is not gzip, bzip2, xz, zstd or plain cpio".into(),
            ));
        }
        Self::from_source(src, pos, compression, Kind::Rpm, opts)
    }
}

impl Archive for CpioStreamArchive {
    fn kind(&self) -> Kind {
        self.kind
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn skip_cost(&self, index: usize) -> u64 {
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
        let mut next = 0usize;
        each_member(&mut *self.stream(), &mut |name, _, data| {
            if path_of(name).is_none() {
                return Ok(());
            }
            let Some(entry) = self.entries.get(next) else {
                return Ok(());
            };
            next += 1;
            if want(entry) {
                visit(entry, data)?;
            }
            Ok(())
        })
    }
}
