//! ZIP extra-field helpers.

pub const ID_ZIP64: u16 = 0x0001;
pub const ID_AES: u16 = 0x9901;
pub const ID_NTFS: u16 = 0x000a;
pub const ID_EXT_TIME: u16 = 0x5455;
pub const ID_UNICODE_PATH: u16 = 0x7075;

/// Iterates `(id, data)` pairs; stops quietly at a truncated field.
pub fn fields(extra: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut rest = extra;
    std::iter::from_fn(move || {
        if rest.len() < 4 {
            return None;
        }
        let id = u16::from_le_bytes([rest[0], rest[1]]);
        let len = u16::from_le_bytes([rest[2], rest[3]]) as usize;
        if rest.len() < 4 + len {
            return None;
        }
        let data = &rest[4..4 + len];
        rest = &rest[4 + len..];
        Some((id, data))
    })
}

pub fn find(extra: &[u8], id: u16) -> Option<&[u8]> {
    fields(extra).find(|(i, _)| *i == id).map(|(_, d)| d)
}

/// Info-ZIP "extended timestamp": UTC modification time.
pub fn ext_mtime(extra: &[u8]) -> Option<i64> {
    let d = find(extra, ID_EXT_TIME)?;
    if d.len() >= 5 && d[0] & 1 != 0 {
        return Some(u32::from_le_bytes([d[1], d[2], d[3], d[4]]) as i64);
    }
    None
}

/// NTFS extra field: modification time as a FILETIME, converted to Unix seconds.
pub fn ntfs_mtime(extra: &[u8]) -> Option<i64> {
    let d = find(extra, ID_NTFS)?;
    let mut rest = d.get(4..)?; // reserved
    while rest.len() >= 4 {
        let tag = u16::from_le_bytes([rest[0], rest[1]]);
        let size = u16::from_le_bytes([rest[2], rest[3]]) as usize;
        let body = rest.get(4..4 + size)?;
        if tag == 1 && size >= 8 {
            let ft = u64::from_le_bytes(body[..8].try_into().ok()?);
            return Some((ft / 10_000_000) as i64 - 11_644_473_600);
        }
        rest = &rest[4 + size..];
    }
    None
}

/// Info-ZIP Unicode Path: `version(1) crc32-of-header-name(4) utf8-name`.
pub fn unicode_path(extra: &[u8], header_name: &[u8]) -> Option<String> {
    let d = find(extra, ID_UNICODE_PATH)?;
    if d.len() < 5 || d[0] != 1 {
        return None;
    }
    let crc = u32::from_le_bytes([d[1], d[2], d[3], d[4]]);
    if crc != crc32fast::hash(header_name) {
        return None;
    }
    String::from_utf8(d[5..].to_vec()).ok()
}
