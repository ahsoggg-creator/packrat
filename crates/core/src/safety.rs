//! Archive-bomb and resource checks that run before anything is written.
//!
//! Nothing here refuses to extract on its own: `inspect` reports `Risk`s, the caller shows them, and the user decides
//! ("cancel" or "continue at your own risk"). What is never optional: an entry cannot produce more bytes than it
//! declares, and no path can leave the destination (see `safepath`).

use crate::archive::Archive;
use crate::entry::EntryKind;
use crate::open::open_source_with;
use crate::options::OpenOptions;
use crate::source::MemSource;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Limits {
    /// An entry that unpacks to more than this many times its packed size is suspicious …
    pub max_ratio: f64,
    /// … but only when it is at least this big (small files compress extremely well and harmlessly).
    pub ratio_min_size: u64,
    /// The whole archive: unpacked size divided by archive size.
    pub max_total_ratio: f64,
    pub total_ratio_min_size: u64,
    /// Total unpacked size that needs confirmation.
    pub max_total_bytes: u64,
    pub max_entries: u64,
    /// Archives inside archives: how deep the nesting may go.
    pub max_nesting: u32,
    /// Free space that must remain after extraction.
    pub free_space_margin: u64,
    /// Budget for looking inside nested archives.
    pub nested_probe_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_ratio: 250.0,
            ratio_min_size: 64 << 20,
            max_total_ratio: 150.0,
            total_ratio_min_size: 1 << 30,
            max_total_bytes: 50 << 30,
            max_entries: 1_000_000,
            max_nesting: 3,
            free_space_margin: 256 << 20,
            nested_probe_bytes: 256 << 20,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Risk {
    /// One entry expands far more than normal data does.
    HighRatio {
        path: String,
        ratio: f64,
        size: u64,
    },
    /// The whole archive expands far more than normal data does.
    HighTotalRatio {
        ratio: f64,
        size: u64,
    },
    TotalTooLarge {
        bytes: u64,
        limit: u64,
    },
    NotEnoughSpace {
        need: u64,
        free: u64,
    },
    TooManyEntries {
        count: u64,
        limit: u64,
    },
    /// Entries whose stored data overlaps: a trick that makes a small file unpack to a huge one.
    OverlappingEntries {
        count: usize,
    },
    /// Archives nested deeper than the limit.
    DeepNesting {
        depth: u32,
        limit: u32,
    },
    /// Archives inside the archive would unpack to this much more.
    NestedExpansion {
        bytes: u64,
    },
}

impl Risk {
    /// A short English description (the GUI translates from the variant).
    pub fn describe(&self) -> String {
        match self {
            Risk::HighRatio { path, ratio, size } => format!("'{path}' expands {ratio:.0}x (to {size} bytes)"),
            Risk::HighTotalRatio { ratio, size } => format!("the archive expands {ratio:.0}x (to {size} bytes)"),
            Risk::TotalTooLarge { bytes, limit } => format!("{bytes} bytes will be written (limit {limit})"),
            Risk::NotEnoughSpace { need, free } => format!("needs {need} bytes but only {free} are free"),
            Risk::TooManyEntries { count, limit } => format!("{count} entries (limit {limit})"),
            Risk::OverlappingEntries { count } => format!("{count} entries share stored data (overlapping entries)"),
            Risk::DeepNesting { depth, limit } => format!("archives are nested {depth} levels deep (limit {limit})"),
            Risk::NestedExpansion { bytes } => format!("nested archives would expand to {bytes} more bytes"),
        }
    }
}

/// Free bytes on the volume that holds `path` (or its nearest existing parent). `None` when unknown.
pub fn free_space(path: &Path) -> Option<u64> {
    let mut p = path;
    while !p.exists() {
        p = p.parent()?;
    }
    free_space_of_existing(p)
}

#[cfg(windows)]
fn free_space_of_existing(p: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = p
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut free_to_caller = 0u64;
    // SAFETY: `wide` is NUL-terminated and the out pointers refer to live locals.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free_to_caller)
}

#[cfg(not(windows))]
fn free_space_of_existing(_: &Path) -> Option<u64> {
    None
}

fn looks_like_archive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        ".zip", ".7z", ".tar", ".tgz", ".gz", ".xz", ".zst", ".bz2", ".lz4", ".tzst", ".txz", ".jar",
    ]
    .iter()
    .any(|e| n.ends_with(e))
}

struct Probe {
    budget: u64,
    archives: u32,
    max_depth: u32,
    expansion: u64,
}

fn probe_nested(archive: &dyn Archive, depth: u32, limits: &Limits, probe: &mut Probe) {
    probe.max_depth = probe.max_depth.max(depth);
    if depth > limits.max_nesting + 1 {
        return; // deep enough to report; looking further only costs time
    }
    for e in archive.entries() {
        if e.kind != EntryKind::File || !looks_like_archive(&e.path) || e.size == 0 {
            continue;
        }
        if probe.budget < e.size || probe.archives >= 2000 {
            return;
        }
        let Ok(reader) = archive.open_entry(e.index) else {
            continue;
        };
        let mut data = Vec::with_capacity(e.size as usize);
        if reader.take(e.size).read_to_end(&mut data).is_err() || data.len() as u64 != e.size {
            continue;
        }
        probe.budget -= e.size;
        probe.archives += 1;
        let Ok(inner) = open_source_with(Arc::new(MemSource(data)), &e.path, &OpenOptions::default()) else {
            continue;
        };
        let inner_total: u64 = inner.entries().iter().map(|x| x.size).sum();
        probe.expansion = probe.expansion.saturating_add(inner_total);
        probe_nested(&*inner, depth + 1, limits, probe);
    }
}

/// Everything worth warning about before extracting `selection` (all entries when `None`) into `dest`.
/// `archive_len` is the size of the archive file (0 when unknown).
pub fn inspect(
    archive: &dyn Archive,
    selection: Option<&[usize]>,
    dest: &Path,
    limits: &Limits,
    archive_len: u64,
) -> Vec<Risk> {
    let entries = archive.entries();
    let chosen: Vec<_> = match selection {
        Some(sel) => sel.iter().filter_map(|&i| entries.get(i)).collect(),
        None => entries.iter().collect(),
    };
    let mut risks = Vec::new();

    if chosen.len() as u64 > limits.max_entries {
        risks.push(Risk::TooManyEntries {
            count: chosen.len() as u64,
            limit: limits.max_entries,
        });
    }

    let mut total: u64 = 0;
    let mut worst: Option<Risk> = None;
    for e in chosen.iter().filter(|e| e.kind == EntryKind::File) {
        total = total.saturating_add(e.size);
        if let Some(packed) = e.packed
            && packed > 0
            && e.size >= limits.ratio_min_size
        {
            let ratio = e.size as f64 / packed as f64;
            if ratio > limits.max_ratio && !matches!(&worst, Some(Risk::HighRatio { ratio: r, .. }) if *r >= ratio) {
                worst = Some(Risk::HighRatio {
                    path: e.path.clone(),
                    ratio,
                    size: e.size,
                });
            }
        }
    }
    risks.extend(worst);

    if archive_len > 0 && total >= limits.total_ratio_min_size {
        let ratio = total as f64 / archive_len as f64;
        if ratio > limits.max_total_ratio {
            risks.push(Risk::HighTotalRatio { ratio, size: total });
        }
    }
    if total > limits.max_total_bytes {
        risks.push(Risk::TotalTooLarge {
            bytes: total,
            limit: limits.max_total_bytes,
        });
    }
    if let Some(free) = free_space(dest)
        && total.saturating_add(limits.free_space_margin) > free
    {
        risks.push(Risk::NotEnoughSpace { need: total, free });
    }

    let overlaps = archive.overlapping_entries();
    if overlaps > 0 {
        risks.push(Risk::OverlappingEntries { count: overlaps });
    }

    let mut probe = Probe {
        budget: limits.nested_probe_bytes,
        archives: 0,
        max_depth: 0,
        expansion: 0,
    };
    probe_nested(archive, 1, limits, &mut probe);
    if probe.max_depth > limits.max_nesting + 1 {
        risks.push(Risk::DeepNesting {
            depth: probe.max_depth - 1,
            limit: limits.max_nesting,
        });
    }
    if probe.expansion > limits.max_total_bytes {
        risks.push(Risk::NestedExpansion { bytes: probe.expansion });
    }
    risks
}
