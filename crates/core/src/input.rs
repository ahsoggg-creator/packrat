//! Collecting files for a new archive.

use crate::error::{Error, Result};
use crate::timeutil;
use std::fs;
use std::path::{Path, PathBuf};

/// Paths that could not be added, with the reason.
pub type Skipped = Vec<(PathBuf, String)>;

#[derive(Clone, Debug)]
pub struct InputItem {
    pub disk_path: PathBuf,
    /// Name inside the archive: `/` separators, directories end with `/`.
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<i64>,
}

/// Walks the given files and folders. Names are relative to each path's parent folder, so adding
/// `C:\data\photos` yields `photos/…`. Symbolic links and junctions are skipped, never followed.
/// Returns the items and the paths that could not be read.
pub fn collect_inputs(paths: &[PathBuf]) -> Result<(Vec<InputItem>, Skipped)> {
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    for p in paths {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| Error::Other(format!("cannot add '{}': no file name", p.display())))?;
        walk(p, &name, &mut items, &mut skipped);
    }
    Ok((items, skipped))
}

fn walk(path: &Path, name: &str, items: &mut Vec<InputItem>, skipped: &mut Skipped) {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            skipped.push((path.to_path_buf(), e.to_string()));
            return;
        }
    };
    if meta.file_type().is_symlink() {
        skipped.push((path.to_path_buf(), "symbolic link".into()));
        return;
    }
    let mtime = meta.modified().ok().map(timeutil::system_time_to_unix);
    if meta.is_dir() {
        items.push(InputItem {
            disk_path: path.to_path_buf(),
            name: format!("{name}/"),
            is_dir: true,
            size: 0,
            mtime,
        });
        let mut children: Vec<_> = match fs::read_dir(path) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(e) => {
                skipped.push((path.to_path_buf(), e.to_string()));
                return;
            }
        };
        children.sort_by_key(|c| c.file_name());
        for c in children {
            let child = c.file_name().to_string_lossy().into_owned();
            walk(&c.path(), &format!("{name}/{child}"), items, skipped);
        }
    } else if meta.is_file() {
        items.push(InputItem {
            disk_path: path.to_path_buf(),
            name: name.to_owned(),
            is_dir: false,
            size: meta.len(),
            mtime,
        });
    } else {
        skipped.push((path.to_path_buf(), "not a regular file".into()));
    }
}
