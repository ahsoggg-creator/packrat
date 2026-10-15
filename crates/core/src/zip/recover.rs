//! Rebuilding the entry list of a damaged ZIP from its local file headers.
//!
//! A ZIP keeps a second copy of every header in the central directory at the end of the file. When the end of the
//! file is lost (an interrupted download, a truncated copy) or the directory is damaged, the local headers in front
//! of each entry still say what the file is. This walks the file, finds them, works out where each entry's data ends
//! (from the header, from the deflate stream itself, or from the next header) and returns what it found.

use super::METHOD_DEFLATE;
use super::extra;
use super::read::{AesInfo, Meta, Parsed, decode_name, method_label, parse_aes_extra};
use crate::entry::{Entry, EntryKind};
use crate::error::Result;
use crate::source::{SharedSource, Source, read_exact_at};
use crate::timeutil;
use flate2::{Decompress, FlushDecompress, Status};

const SIG: [u8; 4] = [b'P', b'K', 3, 4];
const SIG_DESCRIPTOR_BYTES: [u8; 4] = [b'P', b'K', 7, 8];
const MAX_ENTRIES: usize = 5_000_000;

/// Offset of the next local-header signature at or after `from`.
fn find_signature(src: &dyn Source, from: u64, needle: &[u8; 4]) -> Option<u64> {
    let len = src.len();
    let mut pos = from;
    let mut buf = vec![0u8; 1 << 20];
    while pos + 4 <= len {
        let want = buf.len().min((len - pos) as usize);
        if read_exact_at(src, pos, &mut buf[..want]).is_err() {
            return None;
        }
        if let Some(i) = buf[..want].windows(4).position(|w| w == needle) {
            return Some(pos + i as u64);
        }
        if want < 4 {
            return None;
        }
        pos += want as u64 - 3; // keep an overlap so a signature on the boundary is not missed
    }
    None
}

/// Where a deflate stream that starts at `start` ends, and how many bytes it expands to.
fn deflate_extent(src: &dyn Source, start: u64) -> Option<(u64, u64)> {
    let mut d = Decompress::new(false);
    let mut inbuf = vec![0u8; 64 << 10];
    let mut out = vec![0u8; 256 << 10];
    let mut pos = start;
    loop {
        let n = src.read_at(pos, &mut inbuf).ok()?;
        if n == 0 {
            return None; // ran off the end of the file: the entry is truncated
        }
        let mut fed = 0;
        while fed < n {
            let before_in = d.total_in();
            let status = d.decompress(&inbuf[fed..n], &mut out, FlushDecompress::None).ok()?;
            fed += (d.total_in() - before_in) as usize;
            if status == Status::StreamEnd {
                return Some((d.total_in(), d.total_out()));
            }
            if d.total_in() == before_in && status == Status::BufError {
                // output buffer was full without consuming input: loop again with the same input
                continue;
            }
        }
        pos += n as u64;
    }
}

struct Local {
    flags: u16,
    method: u16,
    dos_time: u16,
    dos_date: u16,
    crc: u32,
    csize: u64,
    usize: u64,
    name: Vec<u8>,
    extra: Vec<u8>,
    data_start: u64,
}

fn read_local(src: &dyn Source, pos: u64) -> Option<Local> {
    let mut h = [0u8; 30];
    read_exact_at(src, pos, &mut h).ok()?;
    if h[..4] != SIG {
        return None;
    }
    let u16_at = |i: usize| u16::from_le_bytes([h[i], h[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
    let (flags, method) = (u16_at(6), u16_at(8));
    let (nlen, elen) = (u16_at(26) as usize, u16_at(28) as usize);
    if nlen == 0 || !matches!(method, 0 | 8 | 93 | 99 | 12 | 14 | 9) || u16_at(4) > 100 {
        return None;
    }
    let mut name = vec![0u8; nlen];
    let mut extra_data = vec![0u8; elen];
    read_exact_at(src, pos + 30, &mut name).ok()?;
    read_exact_at(src, pos + 30 + nlen as u64, &mut extra_data).ok()?;
    let (mut csize, mut usize_) = (u32_at(18) as u64, u32_at(22) as u64);
    if (csize == u32::MAX as u64 || usize_ == u32::MAX as u64)
        && let Some(z) = extra::find(&extra_data, extra::ID_ZIP64)
    {
        let mut at = 0;
        if usize_ == u32::MAX as u64 && z.len() >= at + 8 {
            usize_ = u64::from_le_bytes(z[at..at + 8].try_into().ok()?);
            at += 8;
        }
        if csize == u32::MAX as u64 && z.len() >= at + 8 {
            csize = u64::from_le_bytes(z[at..at + 8].try_into().ok()?);
        }
    }
    Some(Local {
        flags,
        method,
        dos_time: u16_at(10),
        dos_date: u16_at(12),
        crc: u32_at(14),
        csize,
        usize: usize_,
        name,
        extra: extra_data,
        data_start: pos + 30 + nlen as u64 + elen as u64,
    })
}

/// Walks the file and returns every entry whose local header can be read.
pub(super) fn scan(src: &SharedSource) -> Result<Parsed> {
    let len = src.len();
    let mut entries: Vec<Entry> = Vec::new();
    let mut meta: Vec<Meta> = Vec::new();
    let mut legacy_idx: Vec<usize> = Vec::new();
    let mut pos = 0u64;

    while entries.len() < MAX_ENTRIES {
        let Some(at) = find_signature(&**src, pos, &SIG) else {
            break;
        };
        let Some(l) = read_local(&**src, at) else {
            pos = at + 4;
            continue;
        };

        let has_descriptor = l.flags & 8 != 0;
        let (mut csize, mut size, mut crc) = (l.csize, l.usize, l.crc);
        let mut damaged_extent = false;
        if has_descriptor || (csize == 0 && l.method == METHOD_DEFLATE && size > 0) {
            match l.method {
                METHOD_DEFLATE if l.flags & 1 == 0 => match deflate_extent(&**src, l.data_start) {
                    Some((consumed, produced)) => {
                        csize = consumed;
                        size = produced;
                    }
                    None => damaged_extent = true,
                },
                _ => damaged_extent = true,
            }
            if damaged_extent {
                // unknown length: the data runs up to the next local header (or the end of the file)
                let next = find_signature(&**src, l.data_start, &SIG)
                    .or_else(|| find_signature(&**src, l.data_start, &SIG_DESCRIPTOR_BYTES))
                    .unwrap_or(len);
                csize = next.saturating_sub(l.data_start);
            }
        }
        let mut end = l.data_start.saturating_add(csize);
        let truncated = end > len;
        if truncated {
            csize = len.saturating_sub(l.data_start);
            end = len;
        }
        // an optional data descriptor follows: signature, CRC, sizes
        if has_descriptor && !truncated {
            let mut d = [0u8; 16];
            if read_exact_at(&**src, end, &mut d).is_ok() && d[..4] == SIG_DESCRIPTOR_BYTES {
                crc = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
                end += 16; // 4 signature + 4 CRC + 4 + 4 (ZIP32 sizes)
                if !damaged_extent && u32::from_le_bytes([d[8], d[9], d[10], d[11]]) as u64 != csize {
                    end += 8; // ZIP64 descriptor has 8-byte sizes
                }
            }
        }

        let index = entries.len();
        let is_dir = l.name.ends_with(b"/") || l.name.ends_with(b"\\");
        let decoded = decode_name(&l.name, l.flags, &l.extra);
        let is_legacy = decoded.is_none();
        let mut e = Entry::new(
            index,
            decoded.unwrap_or_default(),
            if is_dir { EntryKind::Dir } else { EntryKind::File },
        );
        e.raw_name = l.name.clone();
        e.size = size;
        e.packed = Some(csize);
        e.crc32 = (crc != 0 || size == 0).then_some(crc);
        e.encrypted = l.flags & 1 != 0;
        e.method = method_label(l.method);
        e.mtime = extra::ext_mtime(&l.extra)
            .or_else(|| extra::ntfs_mtime(&l.extra))
            .or_else(|| timeutil::dos_to_naive(l.dos_date, l.dos_time).map(timeutil::local_to_utc));
        let aes: Option<AesInfo> = (l.method == 99).then(|| parse_aes_extra(&l.extra)).flatten();
        if aes.is_some() {
            e.crc32 = None;
        }
        if is_legacy {
            legacy_idx.push(index);
        }
        entries.push(e);
        meta.push(Meta {
            local_offset: at,
            csize,
            method: l.method,
            flags: l.flags,
            dos_time: l.dos_time,
            aes,
        });
        pos = end.max(at + 4);
        if truncated {
            break;
        }
    }
    Ok(Parsed {
        entries,
        meta,
        legacy_idx,
        comment: Vec::new(),
        data_end: len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemSource;
    use crate::testkit::{RawEntry, raw_zip};
    use std::sync::Arc;

    fn src(bytes: Vec<u8>) -> SharedSource {
        Arc::new(MemSource(bytes))
    }

    #[test]
    fn local_headers_are_found_without_a_central_directory() {
        let zip = raw_zip(&[
            RawEntry::dir("d/"),
            RawEntry::file("d/a.txt", "alpha ".repeat(100)).deflated(),
            RawEntry::file("b.bin", vec![7u8; 5000]),
        ]);
        // cut the file where the central directory begins
        let cd = zip.windows(4).position(|w| w == [b'P', b'K', 1, 2]).unwrap();
        let parsed = scan(&src(zip[..cd].to_vec())).unwrap();
        let names: Vec<_> = parsed.entries.iter().map(|e| e.raw_name.clone()).collect();
        assert_eq!(names, [b"d/".to_vec(), b"d/a.txt".to_vec(), b"b.bin".to_vec()]);
        assert_eq!(parsed.entries[1].size, 600);
        assert_eq!(parsed.entries[2].size, 5000);
    }
}
