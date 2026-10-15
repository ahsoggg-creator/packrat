//! CPIO archives: the SVR4 "newc" (`070701`, `070702`) and the old portable ASCII ("odc", `070707`) layouts.

use crate::archive::Kind;
use crate::encoding::{NameEncoding, detect};
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result, corrupt};
use crate::options::OpenOptions;
use crate::ranged::{MAX_ENTRIES, RangedArchive};
use crate::source::{SharedSource, read_exact_at};

pub fn is_cpio(head: &[u8]) -> bool {
    head.starts_with(b"070701") || head.starts_with(b"070702") || head.starts_with(b"070707")
}

fn parse(raw: &[u8], radix: u32) -> Result<u64> {
    std::str::from_utf8(raw)
        .ok()
        .and_then(|s| u64::from_str_radix(s, radix).ok())
        .ok_or_else(|| Error::Corrupt("cpio: bad number in a header".into()))
}

pub(crate) struct Header {
    pub mode: u64,
    pub mtime: u64,
    pub name_len: u64,
    pub size: u64,
}

/// `h` is a whole header: 110 bytes for "newc", 76 for the old octal layout.
pub(crate) fn parse_header(h: &[u8], newc: bool) -> Result<Header> {
    let (mode, mtime, name_len, size) = if newc {
        (
            parse(&h[14..22], 16)?,
            parse(&h[46..54], 16)?,
            parse(&h[94..102], 16)?,
            parse(&h[54..62], 16)?,
        )
    } else {
        (
            parse(&h[18..24], 8)?,
            parse(&h[48..59], 8)?,
            parse(&h[59..65], 8)?,
            parse(&h[65..76], 8)?,
        )
    };
    if name_len == 0 || name_len > 4096 {
        return corrupt("cpio: bad name length");
    }
    Ok(Header {
        mode,
        mtime,
        name_len,
        size,
    })
}

pub(crate) fn entry_kind(mode: u64) -> EntryKind {
    match mode & 0o170000 {
        0o040000 => EntryKind::Dir,
        0o100000 => EntryKind::File,
        0o120000 => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}

pub fn open(src: SharedSource, opts: &OpenOptions) -> Result<RangedArchive> {
    let total = src.len();
    let mut magic = [0u8; 6];
    read_exact_at(&*src, 0, &mut magic)?;
    let newc = &magic != b"070707";
    let header_len: u64 = if newc { 110 } else { 76 };
    let align = |n: u64| if newc { (n + 3) & !3 } else { n };
    let mut pos = 0u64;
    let mut items: Vec<(Entry, (u64, u64))> = Vec::new();
    let mut raw_names: Vec<Vec<u8>> = Vec::new();
    while pos + header_len <= total {
        let mut h = vec![0u8; header_len as usize];
        read_exact_at(&*src, pos, &mut h)?;
        if h[..6] != magic {
            return corrupt("cpio: bad header magic");
        }
        let Header {
            mode,
            mtime,
            name_len,
            size,
        } = parse_header(&h, newc)?;
        let name_at = pos + header_len;
        let mut raw = vec![0u8; name_len as usize];
        read_exact_at(&*src, name_at, &mut raw)?;
        let name_bytes = raw.split(|&c| c == 0).next().unwrap_or_default().to_vec();
        let name = String::from_utf8_lossy(&name_bytes).into_owned();
        let data_at = if newc {
            align(name_at + name_len)
        } else {
            name_at + name_len
        };
        if name == "TRAILER!!!" {
            break;
        }
        if size > total.saturating_sub(data_at) {
            return corrupt("cpio: entry runs past the end of the file");
        }
        pos = align(data_at + size);
        let kind = entry_kind(mode);
        let path = name.trim_start_matches("./").to_owned();
        if path.is_empty() || path == "." {
            continue;
        }
        if items.len() >= MAX_ENTRIES {
            return Err(Error::Unsupported("too many entries".into()));
        }
        let mut e = Entry::new(items.len(), path, kind);
        e.size = size;
        e.packed = Some(size);
        e.mtime = Some(mtime as i64);
        e.mode = Some((mode & 0o7777) as u32);
        e.method = "stored".into();
        if kind == EntryKind::Symlink && size <= 4096 {
            let mut t = vec![0u8; size as usize];
            read_exact_at(&*src, data_at, &mut t)?;
            e.link_target = Some(String::from_utf8_lossy(&t).into_owned());
        }
        items.push((e, (data_at, size)));
        raw_names.push(name_bytes);
    }
    // names written by a tool in a legacy code page (bsdtar on a Russian Windows, for one)
    let legacy: Vec<&[u8]> = raw_names
        .iter()
        .filter(|n| std::str::from_utf8(n).is_err())
        .map(Vec::as_slice)
        .collect();
    if !legacy.is_empty() {
        let enc: NameEncoding = opts.encoding.unwrap_or_else(|| detect(&legacy).best);
        for ((e, _), raw) in items.iter_mut().zip(&raw_names) {
            if std::str::from_utf8(raw).is_err() {
                e.path = enc.decode(raw).0.trim_start_matches("./").to_owned();
            }
        }
    }
    Ok(RangedArchive::new(Kind::Cpio, src, items))
}
