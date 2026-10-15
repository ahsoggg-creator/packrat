//! Extraction: plan first (where everything goes, what is blocked, what already exists), then execute.
//! Nothing is ever written outside the destination, and nothing existing is overwritten unless the
//! chosen resolution says so.

use crate::archive::Archive;
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result};
use crate::par::resolve_threads;
use crate::progress::Progress;
use crate::safepath::{self, Unsafe};
use crate::safety::{self, Limits, Risk};
use crate::smart::{self, Layout};
use crate::timeutil;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What to do with a file that already exists at the destination.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resolution {
    /// Leave the existing file alone (the default: nothing is overwritten without a decision).
    #[default]
    Skip,
    Replace,
    /// Extract under a free name: `name (2).ext`.
    KeepBoth,
}

#[derive(Clone, Debug)]
pub struct ExtractOptions {
    pub dest: PathBuf,
    /// File name of the archive (for the folder created by Smart Extract).
    pub archive_name: String,
    pub smart: bool,
    /// Worker threads for random-access archives; 0 = all cores.
    pub threads: usize,
    /// Entry indices to extract; `None` = everything.
    pub selection: Option<Vec<usize>>,
    pub default_resolution: Resolution,
    pub resolutions: HashMap<usize, Resolution>,
    pub restore_times: bool,
    /// Resource limits behind the warnings in `Plan::risks`.
    pub limits: Limits,
    /// Size of the archive file in bytes (0 when unknown); used for the expansion-ratio checks.
    pub archive_len: u64,
    /// The user saw `Plan::risks` and chose to continue. Without it `execute` refuses to run when there are
    /// risks, and enforces the limits while unpacking.
    pub accept_risks: bool,
}

impl ExtractOptions {
    pub fn new(dest: impl Into<PathBuf>, archive_name: impl Into<String>) -> Self {
        Self {
            dest: dest.into(),
            archive_name: archive_name.into(),
            smart: true,
            threads: 0,
            selection: None,
            default_resolution: Resolution::Skip,
            resolutions: HashMap::new(),
            restore_times: true,
            limits: Limits::default(),
            archive_len: 0,
            accept_risks: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlannedItem {
    pub index: usize,
    pub rel: Vec<String>,
    pub target: PathBuf,
    pub kind: EntryKind,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Conflict {
    pub index: usize,
    pub target: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Plan {
    /// Folder that the archive content is placed in (the destination, or a new folder inside it).
    pub base: PathBuf,
    pub layout: Option<Layout>,
    pub items: Vec<PlannedItem>,
    pub blocked: Vec<Issue>,
    /// Files that already exist at their target.
    pub conflicts: Vec<Conflict>,
    pub total_bytes: u64,
    /// Warnings that need the user's confirmation before extracting (see `safety`).
    pub risks: Vec<Risk>,
}

#[derive(Debug, Default, Clone)]
pub struct ExtractReport {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub skipped_existing: Vec<String>,
    pub kept_both: Vec<(String, String)>,
    pub blocked: Vec<Issue>,
    pub failed: Vec<Issue>,
    pub cancelled: bool,
    /// Extraction was stopped because a resource limit was reached while unpacking.
    pub stopped: Option<String>,
}

pub fn plan(archive: &dyn Archive, opts: &ExtractOptions) -> Plan {
    let entries = archive.entries();
    let wanted: Vec<&Entry> = match &opts.selection {
        Some(sel) => sel.iter().filter_map(|&i| entries.get(i)).collect(),
        None => entries.iter().collect(),
    };

    let mut blocked = Vec::new();
    let mut parsed: Vec<(&Entry, Vec<String>)> = Vec::new();
    for e in &wanted {
        match safepath::sanitize(&e.path) {
            Ok(parts) => match e.kind {
                EntryKind::File | EntryKind::Dir => parsed.push((e, parts)),
                EntryKind::Symlink => blocked.push(Issue {
                    path: e.path.clone(),
                    message: "symbolic links are not extracted".into(),
                }),
                EntryKind::Other => blocked.push(Issue {
                    path: e.path.clone(),
                    message: "special files are not extracted".into(),
                }),
            },
            Err(Unsafe::Empty) => {}
            Err(why) => blocked.push(Issue {
                path: e.path.clone(),
                message: why.to_string(),
            }),
        }
    }

    let layout = opts
        .smart
        .then(|| smart::analyze(parsed.iter().map(|(e, p)| (p.as_slice(), e.is_dir()))));
    let base = match &layout {
        Some(Layout::Multiple) => {
            let stem = smart::archive_stem(&opts.archive_name);
            let mut candidate = opts.dest.join(&stem);
            let mut n = 2;
            // a file with that name is in the way: use a free name instead
            while candidate.exists() && !candidate.is_dir() && n < 1000 {
                candidate = opts.dest.join(format!("{stem} ({n})"));
                n += 1;
            }
            candidate
        }
        _ => opts.dest.clone(),
    };

    // case-insensitive top-level names are one folder: use one spelling
    let canonical_root = match &layout {
        Some(Layout::SingleRoot(name)) => Some(name.clone()),
        _ => None,
    };

    let mut items = Vec::with_capacity(parsed.len());
    let mut total_bytes = 0;
    for (e, mut parts) in parsed {
        if let Some(root) = &canonical_root
            && parts[0].eq_ignore_ascii_case(root)
        {
            parts[0] = root.clone();
        }
        let mut target = base.clone();
        target.extend(&parts);
        if e.is_file() {
            total_bytes += e.size;
        }
        items.push(PlannedItem {
            index: e.index,
            rel: parts,
            target,
            kind: e.kind,
            size: e.size,
        });
    }

    let conflicts = items
        .iter()
        .filter(|i| i.kind == EntryKind::File && fs::symlink_metadata(&i.target).is_ok())
        .map(|i| Conflict {
            index: i.index,
            target: i.target.clone(),
        })
        .collect();

    let risks = safety::inspect(
        archive,
        opts.selection.as_deref(),
        &base,
        &opts.limits,
        opts.archive_len,
    );

    Plan {
        base,
        layout,
        items,
        blocked,
        conflicts,
        total_bytes,
        risks,
    }
}

/// Enforces the resource limits while data is unpacked (only when the user has not accepted the risks).
struct Guard {
    stop: AtomicBool,
    total: AtomicU64,
    cap: Option<u64>,
    /// (archive length, max ratio, minimum size): for streams whose unpacked size is not known in advance.
    ratio: Option<(u64, f64, u64)>,
}

impl Guard {
    fn new(opts: &ExtractOptions) -> Self {
        let enforce = !opts.accept_risks;
        Guard {
            stop: AtomicBool::new(false),
            total: AtomicU64::new(0),
            cap: enforce.then_some(opts.limits.max_total_bytes),
            ratio: (enforce && opts.archive_len > 0).then_some((
                opts.archive_len,
                opts.limits.max_total_ratio,
                opts.limits.total_ratio_min_size,
            )),
        }
    }

    fn add(&self, n: u64) -> Result<()> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(Error::Blocked("extraction was stopped by a resource limit".into()));
        }
        let total = self.total.fetch_add(n, Ordering::Relaxed) + n;
        let hit = match (self.cap, self.ratio) {
            (Some(cap), _) if total > cap => Some(format!("the unpacked size passed the limit of {cap} bytes")),
            (_, Some((len, max, min))) if total >= min && total as f64 / len as f64 > max => {
                Some(format!("the data expands more than {max:.0}x (archive bomb?)"))
            }
            _ => None,
        };
        match hit {
            Some(msg) => {
                self.stop.store(true, Ordering::Relaxed);
                Err(Error::Blocked(msg))
            }
            None => Ok(()),
        }
    }
}

fn unique_name(target: &Path) -> PathBuf {
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = target
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 2..10_000 {
        let candidate = target.with_file_name(format!("{stem} ({n}){ext}"));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    target.with_file_name(format!("{stem} (copy){ext}"))
}

enum Outcome {
    Written { bytes: u64 },
    Skipped(String),
    KeptBoth(String, String),
    Failed(String, String),
    Cancelled,
    Stopped(String),
}

fn write_file(
    base: &Path,
    item: &PlannedItem,
    entry: &Entry,
    reader: &mut dyn Read,
    opts: &ExtractOptions,
    progress: &Progress,
    guard: &Guard,
) -> Outcome {
    let name = entry.path.clone();
    let fail = |m: String| Outcome::Failed(name.clone(), m);
    let parent = match safepath::ensure_dirs(base, &item.rel[..item.rel.len() - 1]) {
        Ok(p) => p,
        Err(m) => return fail(m),
    };
    let mut target = parent.join(&item.rel[item.rel.len() - 1]);
    let mut kept = None;
    match fs::symlink_metadata(&target) {
        Ok(meta) => {
            let resolution = opts
                .resolutions
                .get(&item.index)
                .copied()
                .unwrap_or(opts.default_resolution);
            match resolution {
                Resolution::Skip => return Outcome::Skipped(target.display().to_string()),
                Resolution::Replace => {
                    if meta.is_dir() && !meta.file_type().is_symlink() {
                        return fail("a folder with this name exists".into());
                    }
                    if let Err(e) = fs::remove_file(&target) {
                        return fail(format!("cannot replace the existing file: {e}"));
                    }
                }
                Resolution::KeepBoth => {
                    let new = unique_name(&target);
                    kept = Some((target.display().to_string(), new.display().to_string()));
                    target = new;
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return fail(e.to_string()),
    }

    progress.set_current(&entry.path);
    let mut file = match fs::OpenOptions::new().write(true).create_new(true).open(&target) {
        Ok(f) => f,
        Err(e) => return fail(format!("cannot create '{}': {e}", target.display())),
    };
    let mut written = 0u64;
    let result: io::Result<()> = (|| {
        let mut buf = vec![0u8; 256 << 10];
        loop {
            if progress.is_cancelled() {
                return Err(Error::Cancelled.into());
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            guard.add(n as u64)?;
            file.write_all(&buf[..n])?;
            written += n as u64;
            progress.add_bytes(n as u64);
        }
        file.flush()
    })();
    match result {
        Ok(()) => {
            if opts.restore_times
                && let Some(t) = entry.mtime
            {
                let _ = file.set_modified(timeutil::unix_to_system_time(t));
            }
            drop(file);
            progress.add_file();
            match kept {
                Some((from, to)) => Outcome::KeptBoth(from, to),
                None => Outcome::Written { bytes: written },
            }
        }
        Err(e) => {
            drop(file);
            let _ = fs::remove_file(&target);
            match Error::from(e) {
                Error::Cancelled => Outcome::Cancelled,
                Error::Blocked(m) => Outcome::Stopped(m),
                other => fail(other.to_string()),
            }
        }
    }
}

/// Extracts what `plan` describes. Problems with single entries go to the report; the call fails only
/// when the destination cannot be prepared.
pub fn execute(
    archive: &dyn Archive,
    plan: &Plan,
    opts: &ExtractOptions,
    progress: &Progress,
) -> Result<ExtractReport> {
    if !plan.risks.is_empty() && !opts.accept_risks {
        let list: Vec<String> = plan.risks.iter().map(Risk::describe).collect();
        return Err(Error::Blocked(format!("confirmation is required: {}", list.join("; "))));
    }
    let guard = Guard::new(opts);
    fs::create_dir_all(&plan.base)?;
    let mut report = ExtractReport {
        blocked: plan.blocked.clone(),
        ..Default::default()
    };
    progress.set_totals(
        plan.items.iter().filter(|i| i.kind == EntryKind::File).count() as u64,
        plan.total_bytes,
    );

    for item in plan.items.iter().filter(|i| i.kind == EntryKind::Dir) {
        match safepath::ensure_dirs(&plan.base, &item.rel) {
            Ok(_) => report.dirs += 1,
            Err(m) => report.failed.push(Issue {
                path: archive.entries()[item.index].path.clone(),
                message: m,
            }),
        }
    }

    let files: Vec<&PlannedItem> = plan.items.iter().filter(|i| i.kind == EntryKind::File).collect();
    let mut outcomes: Vec<Outcome> = Vec::with_capacity(files.len());
    let entries = archive.entries();

    let threads = resolve_threads(opts.threads);
    if archive.random_access() && threads > 1 && files.len() > 1 {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .map_err(|e| Error::Other(format!("cannot start worker threads: {e}")))?;
        let mut order = files.clone();
        order.sort_by_key(|i| std::cmp::Reverse(i.size)); // big files first keeps all threads busy to the end
        outcomes = pool.install(|| {
            order
                .par_iter()
                .map(|item| {
                    if progress.is_cancelled() {
                        return Outcome::Cancelled;
                    }
                    let entry = &entries[item.index];
                    match archive.open_entry(item.index) {
                        Ok(mut r) => write_file(&plan.base, item, entry, &mut r, opts, progress, &guard),
                        Err(e) => Outcome::Failed(entry.path.clone(), e.to_string()),
                    }
                })
                .collect()
        });
    } else {
        let by_index: HashMap<usize, &PlannedItem> = files.iter().map(|i| (i.index, *i)).collect();
        let stop = std::cell::Cell::new(false);
        let walked = archive.walk(&mut |e| by_index.contains_key(&e.index) && !stop.get(), &mut |e, r| {
            let item = by_index[&e.index];
            let o = write_file(&plan.base, item, e, r, opts, progress, &guard);
            if matches!(o, Outcome::Cancelled | Outcome::Stopped(_)) {
                stop.set(true);
                outcomes.push(o);
                return Err(Error::Cancelled);
            }
            outcomes.push(o);
            Ok(())
        });
        match walked {
            Ok(()) | Err(Error::Cancelled) => {}
            Err(e) => report.failed.push(Issue {
                path: opts.archive_name.clone(),
                message: e.to_string(),
            }),
        }
    }

    for o in outcomes {
        match o {
            Outcome::Written { bytes } => {
                report.files += 1;
                report.bytes += bytes;
            }
            Outcome::KeptBoth(from, to) => {
                report.files += 1;
                report.kept_both.push((from, to));
            }
            Outcome::Skipped(p) => report.skipped_existing.push(p),
            Outcome::Failed(path, message) => report.failed.push(Issue { path, message }),
            Outcome::Cancelled => report.cancelled = true,
            Outcome::Stopped(m) => report.stopped = Some(m),
        }
    }
    if progress.is_cancelled() {
        report.cancelled = true;
    }
    Ok(report)
}

/// Plan + execute with the default resolution for every conflict.
pub fn extract(archive: &dyn Archive, opts: &ExtractOptions, progress: &Progress) -> Result<ExtractReport> {
    let p = plan(archive, opts);
    execute(archive, &p, opts, progress)
}
