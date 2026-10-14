//! Smart Extract: look at the top level of an archive and decide where its content should go, so that
//! nobody ends up with `Folder\Folder\...`.

use crate::safepath;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Exactly one top-level folder: extract it straight into the destination.
    SingleRoot(String),
    /// Exactly one file at the top level and nothing else: extract it next to the archive.
    SingleFile(String),
    /// Several items (or none): put them into a new folder named after the archive.
    Multiple,
}

/// `items` are sanitized `(components, is_dir)` pairs. Top-level names that differ only by case are the same
/// folder on Windows, so they are counted once.
pub fn analyze<'a>(items: impl IntoIterator<Item = (&'a [String], bool)>) -> Layout {
    let mut tops: Vec<(String, String, bool)> = Vec::new(); // (lowercase, spelling, is_dir)
    for (parts, is_dir) in items {
        let Some(first) = parts.first() else { continue };
        let key = first.to_lowercase();
        let top_is_dir = is_dir || parts.len() > 1;
        match tops.iter_mut().find(|(k, _, _)| *k == key) {
            Some(t) => t.2 |= top_is_dir,
            None => tops.push((key, first.clone(), top_is_dir)),
        }
    }
    match tops.as_slice() {
        [(_, name, true)] => Layout::SingleRoot(name.clone()),
        [(_, name, false)] => Layout::SingleFile(name.clone()),
        _ => Layout::Multiple,
    }
}

/// Folder name for a multi-item archive: the archive's file name without its extension(s).
pub fn archive_stem(file_name: &str) -> String {
    let lower = file_name.to_ascii_lowercase();
    let mut stem = file_name;
    for ext in [
        ".tar.gz", ".tar.zst", ".tar.xz", ".tar.bz2", ".tar.lz4", ".tgz", ".txz", ".tbz2", ".tzst",
    ] {
        if lower.ends_with(ext) {
            stem = &file_name[..file_name.len() - ext.len()];
            break;
        }
    }
    if stem.len() == file_name.len() {
        stem = match file_name.rfind('.') {
            Some(i) if i > 0 => &file_name[..i],
            _ => file_name,
        };
    }
    let stem = stem.trim_end_matches(['.', ' ']);
    match safepath::sanitize(stem) {
        Ok(parts) if parts.len() == 1 => parts.into_iter().next().unwrap_or_else(|| "archive".into()),
        _ => "archive".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<String> {
        safepath::sanitize(s).unwrap()
    }

    fn layout(names: &[&str]) -> Layout {
        let parsed: Vec<(Vec<String>, bool)> = names
            .iter()
            .filter_map(|n| safepath::sanitize(n).ok().map(|parts| (parts, n.ends_with('/'))))
            .collect();
        analyze(parsed.iter().map(|(p, d)| (p.as_slice(), *d)))
    }

    #[test]
    fn single_root() {
        assert_eq!(
            layout(&["App/", "App/a.txt", "App/sub/b.txt"]),
            Layout::SingleRoot("App".into())
        );
        assert_eq!(
            layout(&["App/a.txt", "App/b.txt"]),
            Layout::SingleRoot("App".into()),
            "no explicit dir entry"
        );
        assert_eq!(layout(&["App/"]), Layout::SingleRoot("App".into()), "empty folder only");
    }

    #[test]
    fn single_file_and_multiple() {
        assert_eq!(layout(&["readme.txt"]), Layout::SingleFile("readme.txt".into()));
        assert_eq!(layout(&["a.txt", "b.txt"]), Layout::Multiple);
        assert_eq!(layout(&["App/a.txt", "other.txt"]), Layout::Multiple);
        assert_eq!(layout(&["A/x", "B/y"]), Layout::Multiple);
        assert_eq!(layout(&[]), Layout::Multiple);
    }

    #[test]
    fn dot_slash_entries_and_case() {
        assert_eq!(
            layout(&["./", "./App/", "./App/a.txt"]),
            Layout::SingleRoot("App".into())
        );
        assert_eq!(
            layout(&["App/a.txt", "app/b.txt", "APP/"]),
            Layout::SingleRoot("App".into())
        );
        assert_eq!(layout(&["./readme.txt"]), Layout::SingleFile("readme.txt".into()));
        assert_eq!(p("./App/a")[0], "App");
    }

    #[test]
    fn stems() {
        assert_eq!(archive_stem("photos.zip"), "photos");
        assert_eq!(archive_stem("src.tar.gz"), "src");
        assert_eq!(archive_stem("SRC.TAR.ZST"), "SRC");
        assert_eq!(archive_stem("a.b.7z"), "a.b");
        assert_eq!(archive_stem("noext"), "noext");
        assert_eq!(archive_stem(".zip"), ".zip");
        assert_eq!(archive_stem("CON.zip"), "archive");
        assert_eq!(archive_stem("x...zip"), "x");
    }
}
