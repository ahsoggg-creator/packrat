//! Unix `ar` archives (`.a`, `.lib`, `.deb`), with GNU and BSD long names.

use crate::archive::Kind;
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result, corrupt};
use crate::ranged::{MAX_ENTRIES, RangedArchive};
use crate::source::{SharedSource, read_exact_at};

pub const MAGIC: &[u8] = b"!<arch>\n";
const HEADER: u64 = 60;

fn field(raw: &[u8]) -> &str {
    std::str::from_utf8(raw)
        .unwrap_or("")
        .trim_matches(|c: char| c == ' ' || c == '\0')
}

fn number(raw: &[u8], radix: u32) -> Option<u64> {
    u64::from_str_radix(field(raw), radix).ok()
}

pub fn open(src: SharedSource) -> Result<RangedArchive> {
    let total = src.len();
    let mut pos = MAGIC.len() as u64;
    let mut long_names: Vec<u8> = Vec::new();
    let mut items: Vec<(Entry, (u64, u64))> = Vec::new();
    while pos + HEADER <= total {
        let mut h = [0u8; HEADER as usize];
        read_exact_at(&*src, pos, &mut h)?;
        if &h[58..60] != b"`\n" {
            return corrupt("ar: bad header terminator");
        }
        let Some(size) = number(&h[48..58], 10) else {
            return corrupt("ar: bad member size");
        };
        let data = pos + HEADER;
        if size > total - data.min(total) {
            return corrupt("ar: member runs past the end of the file");
        }
        let next = data + size + (size & 1);
        let raw_name = field(&h[0..16]).to_owned();
        match raw_name.as_str() {
            // symbol tables
            "/" | "__.SYMDEF" | "__.SYMDEF SORTED" | "/SYM64/" => {
                pos = next;
                continue;
            }
            "//" => {
                if size > 16 << 20 {
                    return corrupt("ar: long-name table is too large");
                }
                long_names = vec![0; size as usize];
                read_exact_at(&*src, data, &mut long_names)?;
                pos = next;
                continue;
            }
            _ => {}
        }
        let (mut start, mut len) = (data, size);
        let name = if let Some(n) = raw_name.strip_prefix("#1/").and_then(|n| n.parse::<u64>().ok()) {
            // BSD: the name is the first bytes of the data
            if n > len || n > 4096 {
                return corrupt("ar: bad BSD name length");
            }
            let mut b = vec![0u8; n as usize];
            read_exact_at(&*src, start, &mut b)?;
            start += n;
            len -= n;
            String::from_utf8_lossy(&b).trim_end_matches('\0').to_owned()
        } else if let Some(off) = raw_name.strip_prefix('/').and_then(|n| n.parse::<usize>().ok()) {
            let tail = long_names.get(off..).unwrap_or_default();
            let end = tail.iter().position(|&c| c == b'\n').unwrap_or(tail.len());
            String::from_utf8_lossy(&tail[..end]).trim_end_matches('/').to_owned()
        } else {
            raw_name.trim_end_matches('/').to_owned()
        };
        if items.len() >= MAX_ENTRIES {
            return Err(Error::Unsupported("too many entries".into()));
        }
        let mut e = Entry::new(items.len(), name, EntryKind::File);
        e.size = len;
        e.packed = Some(len);
        e.mtime = number(&h[16..28], 10).map(|t| t as i64);
        e.mode = number(&h[40..48], 8).map(|m| (m & 0o7777) as u32);
        e.method = "stored".into();
        items.push((e, (start, len)));
        pos = next;
    }
    Ok(RangedArchive::new(Kind::Ar, src, items))
}
