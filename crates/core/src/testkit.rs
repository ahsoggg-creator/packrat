//! Helpers for building archives by hand, so tests can produce exactly the (often malicious) layouts they need.
//! Not part of the stable API.

use crate::source::MemSource;

pub struct RawEntry {
    pub name: Vec<u8>,
    pub data: Vec<u8>,
    pub flags: u16,
    /// Unix mode placed in the external attributes (made-by = Unix) when set.
    pub unix_mode: Option<u32>,
    pub extra: Vec<u8>,
}

impl RawEntry {
    pub fn file(name: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> Self {
        Self {
            name: name.as_ref().to_vec(),
            data: data.as_ref().to_vec(),
            flags: 0,
            unix_mode: None,
            extra: Vec::new(),
        }
    }

    pub fn dir(name: impl AsRef<[u8]>) -> Self {
        let mut n = name.as_ref().to_vec();
        if !n.ends_with(b"/") {
            n.push(b'/');
        }
        Self {
            name: n,
            data: Vec::new(),
            flags: 0,
            unix_mode: None,
            extra: Vec::new(),
        }
    }

    pub fn utf8(mut self) -> Self {
        self.flags |= 0x0800;
        self
    }

    pub fn mode(mut self, mode: u32) -> Self {
        self.unix_mode = Some(mode);
        self
    }
}

/// Builds a ZIP with all entries stored (method 0) and correct CRCs.
pub fn raw_zip(entries: &[RawEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let offset = out.len() as u32;
        let crc = crc32fast::hash(&e.data);
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(e.flags.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(0x21u16.to_le_bytes());
        out.extend(crc.to_le_bytes());
        out.extend((e.data.len() as u32).to_le_bytes());
        out.extend((e.data.len() as u32).to_le_bytes());
        out.extend((e.name.len() as u16).to_le_bytes());
        out.extend((e.extra.len() as u16).to_le_bytes());
        out.extend(&e.name);
        out.extend(&e.extra);
        out.extend(&e.data);

        central.extend(0x0201_4b50u32.to_le_bytes());
        let made_by: u16 = if e.unix_mode.is_some() { (3 << 8) | 63 } else { 63 };
        central.extend(made_by.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(e.flags.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0x21u16.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend((e.data.len() as u32).to_le_bytes());
        central.extend((e.data.len() as u32).to_le_bytes());
        central.extend((e.name.len() as u16).to_le_bytes());
        central.extend((e.extra.len() as u16).to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(e.unix_mode.map_or(0, |m| m << 16).to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(&e.name);
        central.extend(&e.extra);
    }
    let cd_offset = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0, 0, 0, 0]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

pub fn mem_zip(entries: &[RawEntry]) -> MemSource {
    MemSource(raw_zip(entries))
}
