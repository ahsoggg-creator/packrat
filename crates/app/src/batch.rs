//! Explorer starts one process per selected file ("%1"). For "add to archive" that would open ten windows; this
//! lets the first process collect the others' paths for a moment and act for all of them.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the leader waits for the other processes of one multi-selection to report in.
pub const SETTLE: Duration = Duration::from_millis(700);

/// Returns `Some(all paths)` in the one process that became the leader, `None` in the followers (which just exit).
pub fn gather(dir: &Path, kind: &str, mine: PathBuf, settle: Duration) -> Option<Vec<PathBuf>> {
    let _ = fs::create_dir_all(dir);
    let lock = dir.join(format!("batch-{kind}.lock"));
    let queue = dir.join(format!("batch-{kind}.txt"));
    // a lock left behind by a crashed leader must not block everybody forever
    if fs::metadata(&lock)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|e| e > Duration::from_secs(60)))
    {
        let _ = fs::remove_file(&lock);
    }
    // several processes append at once: a refused open (sharing violation) is retried; the line goes out in a
    // single write, since `writeln!` splits it into several and concurrent appends would interleave them
    let line = format!("{}\n", mine.display());
    for _ in 0..50 {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&queue)
            && f.write_all(line.as_bytes()).is_ok()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if OpenOptions::new().write(true).create_new(true).open(&lock).is_err() {
        return None;
    }
    std::thread::sleep(settle);
    let text = fs::read_to_string(&queue).unwrap_or_default();
    let _ = fs::remove_file(&queue);
    let _ = fs::remove_file(&lock);
    let mut paths: Vec<PathBuf> = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let p = PathBuf::from(line);
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    if paths.is_empty() {
        paths.push(mine);
    }
    paths.sort();
    Some(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_leader_collects_every_path() {
        let dir = tempfile::tempdir().unwrap();
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let d = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    gather(
                        &d,
                        "add",
                        PathBuf::from(format!("C:/files/f{i}.txt")),
                        Duration::from_millis(400),
                    )
                })
            })
            .collect();
        let results: Vec<Option<Vec<PathBuf>>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let leaders: Vec<&Vec<PathBuf>> = results.iter().flatten().collect();
        assert_eq!(leaders.len(), 1, "exactly one process acts");
        assert_eq!(leaders[0].len(), 6);
        // the next selection starts clean
        assert_eq!(
            gather(dir.path(), "add", PathBuf::from("C:/x.txt"), Duration::from_millis(10))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn kinds_do_not_mix_and_a_stale_lock_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let a = gather(dir.path(), "zip", PathBuf::from("a"), Duration::from_millis(5));
        let b = gather(dir.path(), "7z", PathBuf::from("b"), Duration::from_millis(5));
        assert_eq!(a.unwrap(), [PathBuf::from("a")]);
        assert_eq!(b.unwrap(), [PathBuf::from("b")]);
    }
}
