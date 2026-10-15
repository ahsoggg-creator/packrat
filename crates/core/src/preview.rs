//! Looking inside an archive without extracting it to disk.
//!
//! ZIP has a central directory, so one entry can be read without touching the rest of the file. Streaming formats
//! (TAR, solid 7z) have to be decoded up to the entry; that is marked as slow and refused beyond a limit.

use crate::archive::Archive;
use crate::entry::EntryKind;
use crate::error::{Error, Result};
use std::io::Read;

#[derive(Clone, Debug)]
pub struct Limits {
    /// At most this many bytes of the entry are read.
    pub max_bytes: u64,
    /// For streaming formats: how much data may be decoded before the entry is reached.
    pub max_skip: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 8 << 20,
            max_skip: 256 << 20,
        }
    }
}

#[derive(Debug)]
pub struct Preview {
    pub data: Vec<u8>,
    /// The entry is larger than what was read.
    pub truncated: bool,
    /// The archive had to be decoded from its start up to the entry: "may be slow".
    pub slow: bool,
}

/// Reads up to `limits.max_bytes` of entry `index`.
pub fn read_preview(archive: &dyn Archive, index: usize, limits: &Limits) -> Result<Preview> {
    let entry = archive
        .entries()
        .get(index)
        .ok_or_else(|| Error::Other(format!("no entry #{index}")))?;
    if entry.kind != EntryKind::File {
        return Err(Error::Unsupported("only files can be previewed".into()));
    }
    let slow = !archive.random_access() && archive.skip_cost(index) > 0;
    if !archive.random_access() {
        let cost = archive.skip_cost(index);
        if cost > limits.max_skip {
            return Err(Error::Unsupported(format!(
                "the archive would have to be decoded for {cost} bytes to reach this file (limit {})",
                limits.max_skip
            )));
        }
    }
    let mut data = Vec::new();
    if archive.random_access() {
        archive
            .open_entry(index)?
            .take(limits.max_bytes)
            .read_to_end(&mut data)
            .map_err(Error::from)?;
    } else {
        let mut found = false;
        let walked = archive.walk(&mut |e| e.index == index, &mut |_, r| {
            r.take(limits.max_bytes).read_to_end(&mut data).map_err(Error::from)?;
            found = true;
            // everything needed is read: stop the pass
            Err(Error::Cancelled)
        });
        match walked {
            Ok(()) | Err(Error::Cancelled) if found => {}
            Ok(()) => return Err(Error::Corrupt("entry not found while streaming".into())),
            Err(e) => return Err(e),
        }
    }
    let truncated = (data.len() as u64) < entry.size || (entry.size == 0 && data.len() as u64 >= limits.max_bytes);
    Ok(Preview { data, truncated, slow })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentKind {
    Image,
    Text,
    Binary,
}

/// Decides how to show `data` (the start of a file called `name`): image, text or hex.
pub fn classify(name: &str, data: &[u8]) -> ContentKind {
    let magic_image = data.starts_with(b"\x89PNG\r\n\x1a\n")
        || data.starts_with(&[0xff, 0xd8, 0xff])
        || data.starts_with(b"GIF87a")
        || data.starts_with(b"GIF89a")
        || data.starts_with(b"BM") && data.len() > 14 && name.to_ascii_lowercase().ends_with(".bmp")
        || (data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP");
    if magic_image {
        return ContentKind::Image;
    }
    let head = &data[..data.len().min(8192)];
    if head.is_empty() {
        return ContentKind::Text;
    }
    let text = head.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(head);
    // a cut-off multi-byte character at the end of the window is fine
    let valid = match std::str::from_utf8(text) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && e.valid_up_to() + 4 > text.len(),
    };
    let controls = text
        .iter()
        .filter(|&&b| b < 0x20 && !matches!(b, b'\n' | b'\r' | b'\t' | 0x0c))
        .count();
    if valid && !text.contains(&0) && controls * 50 < text.len().max(50) {
        ContentKind::Text
    } else {
        ContentKind::Binary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert_eq!(classify("a.png", b"\x89PNG\r\n\x1a\nrest"), ContentKind::Image);
        assert_eq!(classify("a.jpg", &[0xff, 0xd8, 0xff, 0xe0, 0]), ContentKind::Image);
        assert_eq!(
            classify("a.rs", "fn main() { println!(\"привет\"); }".as_bytes()),
            ContentKind::Text
        );
        assert_eq!(classify("a.bin", &[0, 1, 2, 3, 0xff, 0xfe, 0, 0]), ContentKind::Binary);
        assert_eq!(classify("empty", b""), ContentKind::Text);
        assert_eq!(
            classify("cut", &"я".repeat(10).as_bytes()[..19]),
            ContentKind::Text,
            "a cut-off letter at the end"
        );
        assert_eq!(classify("a.exe", b"MZ\x90\x00\x03\x00\x00\x00"), ContentKind::Binary);
    }
}
