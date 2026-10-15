//! The folder tree shown in the archive view. Pure data, no UI code, so it can be tested.

use packrat_core::{Entry, EntryKind};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    /// Index into the archive's entry list when the archive has an entry for this node.
    pub entry: Option<usize>,
    pub is_dir: bool,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Uncompressed size (sum over files for folders).
    pub size: u64,
    pub packed: u64,
    pub mtime: Option<i64>,
    /// Number of files below (1 for a file).
    pub files: u64,
    pub method: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Packed,
    Ratio,
    Modified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            descending: false,
        }
    }
}

#[derive(Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub roots: Vec<usize>,
}

fn components(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").collect()
}

impl Tree {
    pub fn build(entries: &[Entry]) -> Tree {
        let mut t = Tree::default();
        let mut index: HashMap<(Option<usize>, String), usize> = HashMap::new();
        for e in entries {
            let parts = components(&e.path);
            if parts.is_empty() {
                continue;
            }
            let mut parent: Option<usize> = None;
            for (i, part) in parts.iter().enumerate() {
                let last = i + 1 == parts.len();
                let is_dir = !last || e.kind == EntryKind::Dir;
                let key = (parent, part.to_lowercase());
                // files may repeat (several entries with one name); folders merge
                let reuse = if last && !is_dir {
                    None
                } else {
                    index.get(&key).copied()
                };
                let id = match reuse {
                    Some(id) => id,
                    None => {
                        let id = t.nodes.len();
                        t.nodes.push(Node {
                            name: (*part).to_owned(),
                            entry: None,
                            is_dir,
                            parent,
                            children: Vec::new(),
                            size: 0,
                            packed: 0,
                            mtime: None,
                            files: 0,
                            method: String::new(),
                        });
                        match parent {
                            Some(p) => t.nodes[p].children.push(id),
                            None => t.roots.push(id),
                        }
                        if is_dir {
                            index.insert(key, id);
                        }
                        id
                    }
                };
                if last {
                    let n = &mut t.nodes[id];
                    n.entry = Some(e.index);
                    n.mtime = e.mtime.or(n.mtime);
                    if !is_dir {
                        n.size = e.size;
                        n.packed = e.packed.unwrap_or(0);
                        n.files = 1;
                        n.method = e.method.clone();
                    }
                }
                parent = Some(id);
            }
        }
        t.aggregate();
        t
    }

    fn aggregate(&mut self) {
        // parents are created before their children, so a reverse pass sees every child before its parent
        for id in (0..self.nodes.len()).rev() {
            let Some(p) = self.nodes[id].parent else { continue };
            let (size, packed, files, mtime) = {
                let n = &self.nodes[id];
                (n.size, n.packed, n.files, n.mtime)
            };
            let parent = &mut self.nodes[p];
            parent.size += size;
            parent.packed += packed;
            parent.files += files;
            parent.mtime = parent.mtime.max(mtime);
        }
    }

    /// All archive entries at or below the given nodes (folders contribute their whole subtree).
    pub fn entries_under(&self, selected: &HashSet<usize>) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack: Vec<usize> = selected.iter().copied().collect();
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(e) = self.nodes[id].entry {
                out.push(e);
            }
            stack.extend(&self.nodes[id].children);
        }
        out.sort_unstable();
        out
    }

    fn cmp(&self, a: usize, b: usize, sort: Sort) -> std::cmp::Ordering {
        let (na, nb) = (&self.nodes[a], &self.nodes[b]);
        let ratio = |n: &Node| {
            if n.size == 0 {
                0.0
            } else {
                n.packed as f64 / n.size as f64
            }
        };
        let by = match sort.key {
            SortKey::Name => na.name.to_lowercase().cmp(&nb.name.to_lowercase()),
            SortKey::Size => na.size.cmp(&nb.size),
            SortKey::Packed => na.packed.cmp(&nb.packed),
            SortKey::Ratio => ratio(na).total_cmp(&ratio(nb)),
            SortKey::Modified => na.mtime.cmp(&nb.mtime),
        };
        let by = if sort.descending { by.reverse() } else { by };
        // folders always come first
        nb.is_dir
            .cmp(&na.is_dir)
            .then(by)
            .then_with(|| na.name.to_lowercase().cmp(&nb.name.to_lowercase()))
    }

    /// Visible rows in display order as `(node, depth)`.
    pub fn visible(&self, expanded: &HashSet<usize>, sort: Sort) -> Vec<(usize, usize)> {
        let mut out = Vec::with_capacity(self.nodes.len().min(4096));
        let mut stack: Vec<(usize, usize)> = {
            let mut r = self.roots.clone();
            r.sort_by(|&a, &b| self.cmp(b, a, sort));
            r.into_iter().map(|n| (n, 0)).collect()
        };
        while let Some((id, depth)) = stack.pop() {
            out.push((id, depth));
            if self.nodes[id].is_dir && expanded.contains(&id) {
                let mut kids = self.nodes[id].children.clone();
                kids.sort_by(|&a, &b| self.cmp(b, a, sort));
                stack.extend(kids.into_iter().map(|k| (k, depth + 1)));
            }
        }
        out
    }

    pub fn dir_ids(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.nodes.len()).filter(|&i| self.nodes[i].is_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(index: usize, path: &str, dir: bool, size: u64) -> Entry {
        let mut x = Entry::new(index, path.into(), if dir { EntryKind::Dir } else { EntryKind::File });
        x.size = size;
        x.packed = Some(size / 2);
        x
    }

    fn sample() -> Vec<Entry> {
        vec![
            e(0, "App/", true, 0),
            e(1, "App/a.txt", false, 100),
            e(2, "App/sub/b.txt", false, 300),
            e(3, "top.txt", false, 5),
            e(4, "./app/c.txt", false, 50),
        ]
    }

    #[test]
    fn builds_folders_merges_case_and_sums_sizes() {
        let t = Tree::build(&sample());
        assert_eq!(t.roots.len(), 2, "App and top.txt (app/ merges with App/)");
        let app = t.roots.iter().copied().find(|&i| t.nodes[i].name == "App").unwrap();
        assert!(t.nodes[app].is_dir);
        assert_eq!(t.nodes[app].size, 450);
        assert_eq!(t.nodes[app].files, 3);
        assert_eq!(t.nodes[app].children.len(), 3, "a.txt, sub, c.txt");
    }

    #[test]
    fn visible_rows_respect_expansion_and_sort() {
        let t = Tree::build(&sample());
        let none = HashSet::new();
        let rows = t.visible(&none, Sort::default());
        assert_eq!(
            rows.iter().map(|(n, _)| t.nodes[*n].name.as_str()).collect::<Vec<_>>(),
            ["App", "top.txt"]
        );
        let all: HashSet<usize> = t.dir_ids().collect();
        let rows = t.visible(&all, Sort::default());
        let names: Vec<_> = rows
            .iter()
            .map(|(n, d)| format!("{}{}", "-".repeat(*d), t.nodes[*n].name))
            .collect();
        assert_eq!(names, ["App", "-sub", "--b.txt", "-a.txt", "-c.txt", "top.txt"]);
        let by_size = t.visible(
            &all,
            Sort {
                key: SortKey::Size,
                descending: true,
            },
        );
        assert_eq!(t.nodes[by_size[0].0].name, "App");
        let first_child = by_size[1].0;
        assert_eq!(t.nodes[first_child].name, "sub", "folders stay first");
    }

    #[test]
    fn selection_expands_to_the_whole_subtree() {
        let t = Tree::build(&sample());
        let app = t.roots.iter().copied().find(|&i| t.nodes[i].name == "App").unwrap();
        let sel: HashSet<usize> = [app].into();
        assert_eq!(t.entries_under(&sel), [0, 1, 2, 4]);
    }

    #[test]
    fn duplicate_file_names_stay_separate() {
        let t = Tree::build(&[e(0, "x.txt", false, 1), e(1, "x.txt", false, 2)]);
        assert_eq!(t.roots.len(), 2);
    }
}
