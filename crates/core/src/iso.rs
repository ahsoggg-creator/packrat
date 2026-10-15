//! ISO 9660 disc images, with Joliet (UTF-16 names) or Rock Ridge (long names) when present.
//! Files stored in several extents (over 4 GiB) show up as separate entries.

use crate::archive::Kind;
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result, corrupt};
use crate::ranged::{MAX_ENTRIES, RangedArchive};
use crate::source::{SharedSource, read_exact_at};
use crate::timeutil::unix_from_parts;
use std::collections::HashSet;

const SECTOR: u64 = 2048;
const FIRST_DESCRIPTOR: u64 = 16 * SECTOR;
const MAX_DIR_BYTES: u64 = 64 << 20;
const MAX_DEPTH: usize = 64;

pub fn is_iso(src: &SharedSource) -> bool {
    let mut b = [0u8; 5];
    src.len() > FIRST_DESCRIPTOR + 6 && read_exact_at(&**src, FIRST_DESCRIPTOR + 1, &mut b).is_ok() && &b == b"CD001"
}

struct Volume {
    root: (u64, u64),
    block: u64,
    joliet: bool,
}

fn le32(b: &[u8]) -> u64 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64
}

fn find_volume(src: &SharedSource) -> Result<Volume> {
    let mut primary: Option<Volume> = None;
    for i in 0..32u64 {
        let mut d = [0u8; SECTOR as usize];
        if read_exact_at(&**src, FIRST_DESCRIPTOR + i * SECTOR, &mut d).is_err() || &d[1..6] != b"CD001" {
            break;
        }
        let block = u16::from_le_bytes([d[128], d[129]]) as u64;
        let vol = Volume {
            root: (le32(&d[158..162]), le32(&d[166..170])),
            block: if block >= 512 { block } else { SECTOR },
            joliet: false,
        };
        match d[0] {
            1 if primary.is_none() => primary = Some(vol),
            2 if matches!(&d[88..91], b"%/@" | b"%/C" | b"%/E") => {
                return Ok(Volume { joliet: true, ..vol });
            }
            255 => break,
            _ => {}
        }
    }
    primary.ok_or_else(|| Error::Corrupt("iso: no primary volume descriptor".into()))
}

fn record_name(rec: &[u8], joliet: bool) -> String {
    let n = rec[32] as usize;
    let raw = &rec[33..33 + n];
    let name = if joliet {
        let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        // Rock Ridge alternate name, if the system use area has one
        let mut at = 33 + n + usize::from(n.is_multiple_of(2));
        let mut nm: Option<String> = None;
        while at + 4 <= rec.len() {
            let len = rec[at + 2] as usize;
            if len < 4 || at + len > rec.len() {
                break;
            }
            if &rec[at..at + 2] == b"NM" && len > 5 {
                let flags = rec[at + 4];
                if flags & 0b110 == 0 {
                    nm.get_or_insert_with(String::new)
                        .push_str(&String::from_utf8_lossy(&rec[at + 5..at + len]));
                }
            }
            at += len;
        }
        nm.unwrap_or_else(|| String::from_utf8_lossy(raw).into_owned())
    };
    let name = match name.rsplit_once(';') {
        Some((base, v)) if !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()) => base.to_owned(),
        _ => name,
    };
    name.strip_suffix('.').map(str::to_owned).unwrap_or(name)
}

fn record_time(rec: &[u8]) -> Option<i64> {
    let t = &rec[18..25];
    if t[1] == 0 || t[1] > 12 || t[2] == 0 || t[2] > 31 {
        return None;
    }
    let offset = t[6] as i8 as i64 * 15 * 60;
    Some(
        unix_from_parts(
            1900 + t[0] as i64,
            t[1] as i64,
            t[2] as i64,
            t[3] as i64,
            t[4] as i64,
            t[5] as i64,
        ) - offset,
    )
}

pub fn open(src: SharedSource) -> Result<RangedArchive> {
    let vol = find_volume(&src)?;
    let total = src.len();
    let mut items: Vec<(Entry, (u64, u64))> = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut stack: Vec<(u64, u64, String, usize)> = vec![(vol.root.0, vol.root.1, String::new(), 0)];
    while let Some((lba, len, prefix, depth)) = stack.pop() {
        if !seen.insert(lba) {
            continue;
        }
        let at = lba.saturating_mul(vol.block);
        if len > MAX_DIR_BYTES || at.saturating_add(len) > total {
            return corrupt("iso: directory lies outside the image");
        }
        let mut dir = vec![0u8; len as usize];
        read_exact_at(&*src, at, &mut dir)?;
        let mut pos = 0usize;
        while pos < dir.len() {
            let rl = dir[pos] as usize;
            if rl == 0 {
                // records never cross a sector: the rest of this one is padding
                pos = (pos / vol.block as usize + 1) * vol.block as usize;
                continue;
            }
            if rl < 34 || pos + rl > dir.len() || 33 + dir[pos + 32] as usize > rl {
                return corrupt("iso: bad directory record");
            }
            let rec = &dir[pos..pos + rl];
            pos += rl;
            if rec[32] == 1 && (rec[33] == 0 || rec[33] == 1) {
                continue; // "." and ".."
            }
            let name = record_name(rec, vol.joliet);
            if name.is_empty() {
                continue;
            }
            let extent = le32(&rec[2..6]);
            let size = le32(&rec[10..14]);
            let is_dir = rec[25] & 2 != 0;
            if items.len() >= MAX_ENTRIES {
                return Err(Error::Unsupported("too many entries".into()));
            }
            let path = format!("{prefix}{name}");
            let mut e = Entry::new(
                items.len(),
                path.clone(),
                if is_dir { EntryKind::Dir } else { EntryKind::File },
            );
            e.mtime = record_time(rec);
            e.method = "stored".into();
            if is_dir {
                if depth + 1 > MAX_DEPTH {
                    return corrupt("iso: directories are nested too deeply");
                }
                stack.push((extent, size, format!("{path}/"), depth + 1));
                items.push((e, (0, 0)));
            } else {
                let start = extent.saturating_mul(vol.block);
                if start.saturating_add(size) > total {
                    return corrupt("iso: a file lies outside the image");
                }
                e.size = size;
                e.packed = Some(size);
                items.push((e, (start, size)));
            }
        }
    }
    items.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    for (i, (e, _)) in items.iter_mut().enumerate() {
        e.index = i;
    }
    Ok(RangedArchive::new(Kind::Iso, src, items))
}
