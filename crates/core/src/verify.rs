//! Integrity check: every selected file is read to the end and checked against what the archive says about it
//! (ZIP and 7z store CRC-32 per entry; for TAR and compressed streams the stream's own checks apply).

use crate::archive::Archive;
use crate::entry::EntryKind;
use crate::error::{Error, Result};
use crate::extract::Issue;
use crate::par::resolve_threads;
use crate::progress::Progress;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct VerifyReport {
    /// Files that were read completely and matched.
    pub ok: u64,
    pub bytes: u64,
    pub failures: Vec<Issue>,
    pub cancelled: bool,
}

impl VerifyReport {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty() && !self.cancelled
    }
}

fn read_all(r: &mut dyn Read, progress: &Progress) -> io::Result<u64> {
    let mut buf = vec![0u8; 256 << 10];
    let mut total = 0u64;
    loop {
        if progress.is_cancelled() {
            return Err(Error::Cancelled.into());
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        total += n as u64;
        progress.add_bytes(n as u64);
    }
}

/// Reads every selected file (all when `selection` is `None`) and reports the ones that fail.
pub fn verify(archive: &dyn Archive, selection: Option<&[usize]>, threads: usize, progress: &Progress) -> VerifyReport {
    let entries = archive.entries();
    let chosen: Vec<usize> = match selection {
        Some(s) => s.iter().copied().filter(|&i| i < entries.len()).collect(),
        None => (0..entries.len()).collect(),
    };
    let files: Vec<usize> = chosen
        .into_iter()
        .filter(|&i| entries[i].kind == EntryKind::File)
        .collect();
    progress.set_totals(files.len() as u64, files.iter().map(|&i| entries[i].size).sum());

    let mut report = VerifyReport::default();
    let record = |index: usize, res: std::result::Result<u64, Error>, report: &mut VerifyReport| match res {
        Ok(n) => {
            report.ok += 1;
            report.bytes += n;
            progress.add_file();
        }
        Err(Error::Cancelled) => report.cancelled = true,
        Err(e) => report.failures.push(Issue {
            path: entries[index].path.clone(),
            message: e.to_string(),
        }),
    };

    let threads = resolve_threads(threads);
    let pool = (archive.random_access() && threads > 1 && files.len() > 1)
        .then(|| rayon::ThreadPoolBuilder::new().num_threads(threads).build().ok())
        .flatten();
    if let Some(pool) = pool {
        let results: Vec<(usize, std::result::Result<u64, Error>)> = pool.install(|| {
            files
                .par_iter()
                .map(|&i| {
                    let r = archive
                        .open_entry(i)
                        .and_then(|mut r| read_all(&mut r, progress).map_err(Error::from));
                    (i, r)
                })
                .collect()
        });
        for (i, r) in results {
            record(i, r, &mut report);
        }
        return report;
    }
    let wanted: std::collections::HashSet<usize> = files.iter().copied().collect();
    let mut results: Vec<(usize, std::result::Result<u64, Error>)> = Vec::new();
    let walked = archive.walk(&mut |e| wanted.contains(&e.index), &mut |e, r| {
        let res = read_all(r, progress).map_err(Error::from);
        let stop = matches!(res, Err(Error::Cancelled));
        results.push((e.index, res));
        if stop { Err(Error::Cancelled) } else { Ok(()) }
    });
    for (i, r) in results {
        record(i, r, &mut report);
    }
    if let Err(e) = walked
        && !matches!(e, Error::Cancelled)
    {
        // the stream itself is damaged: everything after this point is unreadable
        report.failures.push(Issue {
            path: String::new(),
            message: e.to_string(),
        });
    }
    report
}

/// SHA-256 of a file as lowercase hex.
pub fn sha256_file(path: &Path, progress: &Progress) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        progress.check()?;
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        progress.add_bytes(n as u64);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
