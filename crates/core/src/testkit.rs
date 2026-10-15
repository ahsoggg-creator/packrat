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
    /// Compress the data with deflate (otherwise it is stored).
    pub deflate: bool,
    /// MS-DOS `(date, time)` for the entry; a fixed date when unset.
    pub dos: Option<(u16, u16)>,
    /// Make the central directory point at the data of this earlier entry (overlapping entries).
    pub share_data_with: Option<usize>,
}

impl RawEntry {
    pub fn file(name: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> Self {
        Self {
            name: name.as_ref().to_vec(),
            data: data.as_ref().to_vec(),
            flags: 0,
            unix_mode: None,
            extra: Vec::new(),
            deflate: false,
            dos: None,
            share_data_with: None,
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
            deflate: false,
            dos: None,
            share_data_with: None,
        }
    }

    pub fn utf8(mut self) -> Self {
        self.flags |= 0x0800;
        self
    }

    pub fn sharing_data_with(mut self, earlier: usize) -> Self {
        self.share_data_with = Some(earlier);
        self
    }

    pub fn deflated(mut self) -> Self {
        self.deflate = true;
        self
    }

    pub fn dated(mut self, year: u16, month: u16, day: u16, hour: u16, minute: u16) -> Self {
        self.dos = Some((((year - 1980) << 9) | (month << 5) | day, (hour << 11) | (minute << 5)));
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
    let mut offsets: Vec<u32> = Vec::new();
    for e in entries {
        let offset = out.len() as u32;
        offsets.push(offset);
        let crc = crc32fast::hash(&e.data);
        let (method, body): (u16, Vec<u8>) = if e.deflate && !e.data.is_empty() {
            (8, crate::codec::deflate::compress_block(&e.data, 6, true))
        } else {
            (0, e.data.clone())
        };
        let (date, time) = e.dos.unwrap_or((0x21, 0));
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(e.flags.to_le_bytes());
        out.extend(method.to_le_bytes());
        out.extend(time.to_le_bytes());
        out.extend(date.to_le_bytes());
        out.extend(crc.to_le_bytes());
        out.extend((body.len() as u32).to_le_bytes());
        out.extend((e.data.len() as u32).to_le_bytes());
        out.extend((e.name.len() as u16).to_le_bytes());
        out.extend((e.extra.len() as u16).to_le_bytes());
        out.extend(&e.name);
        out.extend(&e.extra);
        out.extend(&body);

        central.extend(0x0201_4b50u32.to_le_bytes());
        let made_by: u16 = if e.unix_mode.is_some() { (3 << 8) | 63 } else { 63 };
        central.extend(made_by.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(e.flags.to_le_bytes());
        central.extend(method.to_le_bytes());
        central.extend(time.to_le_bytes());
        central.extend(date.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend((body.len() as u32).to_le_bytes());
        central.extend((e.data.len() as u32).to_le_bytes());
        central.extend((e.name.len() as u16).to_le_bytes());
        central.extend((e.extra.len() as u16).to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(e.unix_mode.map_or(0, |m| m << 16).to_le_bytes());
        central.extend(e.share_data_with.map_or(offset, |i| offsets[i]).to_le_bytes());
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
