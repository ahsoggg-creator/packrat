//! Background work: archive operations run on a worker thread and report back through a channel.

use super::tree::Tree;
use packrat_core::create::CreateReport;
use packrat_core::extract::{ExtractOptions, ExtractReport, Plan};
use packrat_core::{Archive, Error, OpenOptions, Progress, open};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

pub struct Loaded {
    pub archive: Arc<dyn Archive>,
    pub path: Option<PathBuf>,
    pub name: String,
    pub file_len: u64,
    /// What the archive was opened with (name encoding, password); kept in memory only.
    pub opts: OpenOptions,
    pub tree: Tree,
    pub expanded: HashSet<usize>,
    pub selected: HashSet<usize>,
    pub last_clicked: Option<usize>,
}

impl Loaded {
    pub fn new(
        archive: Arc<dyn Archive>,
        path: Option<PathBuf>,
        name: String,
        file_len: u64,
        opts: OpenOptions,
    ) -> Self {
        let tree = Tree::build(archive.entries());
        // a single top-level folder starts opened
        let expanded = if tree.roots.len() == 1 {
            tree.roots.iter().copied().collect()
        } else {
            HashSet::new()
        };
        Self {
            archive,
            path,
            name,
            file_len,
            opts,
            tree,
            expanded,
            selected: HashSet::new(),
            last_clicked: None,
        }
    }

    pub fn has_encrypted(&self) -> bool {
        self.archive.entries().iter().any(|e| e.encrypted)
    }

    pub fn uses_zipcrypto(&self) -> bool {
        self.archive.entries().iter().any(|e| e.method.starts_with("ZipCrypto"))
    }
}

/// Why an archive could not be opened.
pub struct OpenFail {
    pub message: String,
    pub needs_password: bool,
    pub path: PathBuf,
}

pub enum Outcome {
    Opened(Result<Box<Loaded>, OpenFail>),
    /// An archive that was dropped on the window turned out not to be one: offer to create an archive instead.
    NotAnArchive(Vec<PathBuf>),
    Planned {
        plan: Box<Plan>,
        opts: Box<ExtractOptions>,
    },
    Extracted {
        report: Result<ExtractReport, String>,
        secs: f64,
        dest: PathBuf,
    },
    Created {
        result: Result<CreateReport, String>,
        output: PathBuf,
        secs: f64,
    },
}

pub struct Job {
    pub title: String,
    pub progress: Arc<Progress>,
    pub rx: Receiver<Outcome>,
}

impl Job {
    pub fn spawn(
        ctx: &eframe::egui::Context,
        title: String,
        work: impl FnOnce(Arc<Progress>) -> Outcome + Send + 'static,
    ) -> Job {
        let progress = Arc::new(Progress::new());
        let (tx, rx): (Sender<Outcome>, Receiver<Outcome>) = channel();
        let p = progress.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("packrat-job".into())
            .spawn(move || {
                let out = work(p);
                let _ = tx.send(out);
                ctx.request_repaint();
            })
            .expect("cannot start the worker thread");
        Job { title, progress, rx }
    }
}

pub fn open_loaded(path: PathBuf, opts: OpenOptions) -> Result<Box<Loaded>, OpenFail> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    match open::open_with(&path, &opts) {
        Ok(archive) => Ok(Box::new(Loaded::new(
            Arc::from(archive),
            Some(path),
            name,
            file_len,
            opts,
        ))),
        Err(e) => Err(OpenFail {
            needs_password: matches!(e, Error::PasswordRequired | Error::WrongPassword),
            message: e.to_string(),
            path,
        }),
    }
}
