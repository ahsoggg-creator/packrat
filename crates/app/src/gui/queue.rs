//! Several archives chosen in Explorer are handled one after another in one window: extracted next to themselves,
//! extracted into one folder, or checked. A clean run ends silently by opening the folder.

use super::{AfterOpen, App, ResultInfo};
use crate::i18n::{Key, tf};
use crate::{diag, winint};
use eframe::egui;
use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QueueKind {
    /// Smart extraction next to each archive.
    Here,
    /// Smart extraction into a folder asked once.
    To,
    Test,
}

pub(crate) struct Queue {
    kind: QueueKind,
    pending: VecDeque<PathBuf>,
    total: usize,
    /// The archive being handled (its dialogs may still be on screen).
    current: Option<PathBuf>,
    /// How `current` ended: `Some(true)` clean, `Some(false)` with problems; `None` cancelled or checked only.
    outcome: Option<bool>,
    clean: usize,
    problems: Vec<String>,
    /// The destination chosen for the first archive of a `To` queue; the others reuse it.
    shared_dest: Option<(PathBuf, bool)>,
    last_dest: Option<PathBuf>,
    /// What to say when the only archive went well but the folder is not opened.
    single_report: Option<ResultInfo>,
    /// Started from Explorer: the window is compact and closes when everything is done.
    pub(super) quiet: bool,
}

impl Queue {
    pub(super) fn new(kind: QueueKind, items: Vec<PathBuf>, quiet: bool) -> Queue {
        Queue {
            kind,
            total: items.len(),
            pending: items.into(),
            current: None,
            outcome: None,
            clean: 0,
            problems: Vec::new(),
            shared_dest: None,
            last_dest: None,
            single_report: None,
            quiet,
        }
    }

    /// 1-based position of the archive being handled.
    pub(super) fn position(&self) -> usize {
        self.total - self.pending.len()
    }

    pub(super) fn total(&self) -> usize {
        self.total
    }

    pub(super) fn current_name(&self) -> String {
        self.current
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Notes how the archive in hand ended; a clean report is held back, since the queue reports once, at the end.
    pub(super) fn record_extract(&mut self, clean: bool, dest: PathBuf, held: Option<ResultInfo>) {
        self.outcome = Some(clean);
        self.last_dest = Some(dest);
        self.single_report = held;
    }

    pub(super) fn remember_dest(&mut self, dest: PathBuf, smart: bool) {
        if self.kind == QueueKind::To && self.shared_dest.is_none() {
            self.shared_dest = Some((dest, smart));
        }
    }
}

impl App {
    pub(super) fn start_queue(&mut self, ctx: &egui::Context, kind: QueueKind, items: Vec<PathBuf>, quiet: bool) {
        if items.is_empty() {
            return;
        }
        self.queue = Some(Queue::new(kind, items, quiet));
        self.next_in_queue(ctx);
    }

    fn next_in_queue(&mut self, ctx: &egui::Context) {
        let Some(q) = self.queue.as_mut() else { return };
        let Some(path) = q.pending.pop_front() else {
            self.finish_queue(ctx);
            return;
        };
        q.current = Some(path.clone());
        q.outcome = None;
        self.after_open = match q.kind {
            QueueKind::Here => AfterOpen::Extract {
                dest: path.parent().map(std::path::Path::to_path_buf).unwrap_or_default(),
                smart: true,
                selected_only: false,
            },
            QueueKind::To => match &q.shared_dest {
                Some((dest, smart)) => AfterOpen::Extract {
                    dest: dest.clone(),
                    smart: *smart,
                    selected_only: false,
                },
                None => AfterOpen::ExtractDialog,
            },
            QueueKind::Test => AfterOpen::Verify,
        };
        self.open_path(ctx, path);
    }

    /// Called every frame: once the archive in hand has nothing left on screen, the next one starts.
    pub(super) fn advance_queue(&mut self, ctx: &egui::Context) {
        let Some(q) = &self.queue else { return };
        if q.current.is_none() {
            return;
        }
        let busy = self.job.is_some()
            || self.extract.is_some()
            || self.conflicts.is_some()
            || self.password_dialog.is_some()
            || self.risk_dialog.is_some()
            || self.encoding_dialog.is_some()
            || self.create.is_some()
            || self.result.is_some();
        if busy {
            return;
        }
        if let Some(q) = self.queue.as_mut() {
            match q.outcome.take() {
                Some(true) => q.clean += 1,
                // cancelled, wrong password or not an archive: nothing was extracted
                Some(false) | None if q.kind != QueueKind::Test => q.problems.push(q.current_name()),
                _ => {}
            }
        }
        self.next_in_queue(ctx);
    }

    fn finish_queue(&mut self, ctx: &egui::Context) {
        let Some(q) = self.queue.take() else { return };
        diag::info(&format!(
            "queue finished: {} of {} clean, {} with problems",
            q.clean,
            q.total,
            q.problems.len()
        ));
        if q.kind == QueueKind::Test {
            // every check has shown its own result
            if q.quiet {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        let all_clean = q.clean == q.total;
        let folder = q.last_dest.clone();
        if all_clean && self.settings.open_after_extract {
            if let Some(f) = &folder {
                winint::show_in_explorer(f);
            }
            if q.quiet {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        if q.total == 1 {
            // the single archive already reported its problems; a clean one reports now
            match q.single_report {
                Some(r) if all_clean => self.result = Some(r),
                _ if q.quiet => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                _ => return,
            }
        } else {
            let line = if all_clean {
                tf(Key::BatchAllDone, &[&q.total.to_string()])
            } else {
                tf(Key::BatchSomeDone, &[&q.clean.to_string(), &q.total.to_string()])
            };
            self.result = Some(ResultInfo {
                ok: all_clean,
                lines: vec![line],
                details: q.problems.iter().map(|n| format!("⚠ {n}")).collect(),
                folder,
                ..Default::default()
            });
        }
        self.quit_on_close = q.quiet;
    }
}
