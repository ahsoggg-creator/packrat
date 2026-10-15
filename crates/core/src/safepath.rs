//! Turning a name stored in an archive into a path that cannot leave the destination folder.
//! The rules are those of Windows (the strictest), applied on every platform.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsafe {
    /// Starts with `/`, `\` or a UNC prefix.
    Absolute,
    /// `C:` at the start.
    DriveLetter,
    /// A `..` component.
    ParentDir,
    /// `CON`, `NUL`, `COM1`, ... (with or without extension).
    Reserved(String),
    /// `name:stream` or `::$DATA`: NTFS alternate data streams.
    AlternateStream,
    /// Windows silently strips trailing dots and spaces, which would let a name change meaning.
    TrailingDotOrSpace,
    InvalidChar(char),
    TooLong,
    Empty,
}

impl fmt::Display for Unsafe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unsafe::Absolute => f.write_str("absolute path"),
            Unsafe::DriveLetter => f.write_str("drive letter in the path"),
            Unsafe::ParentDir => f.write_str("'..' component (path traversal)"),
            Unsafe::Reserved(n) => write!(f, "reserved Windows device name '{n}'"),
            Unsafe::AlternateStream => f.write_str("NTFS alternate data stream"),
            Unsafe::TrailingDotOrSpace => f.write_str("name ends with a dot or a space"),
            Unsafe::InvalidChar(c) => write!(f, "character {c:?} is not allowed in file names"),
            Unsafe::TooLong => f.write_str("name is too long"),
            Unsafe::Empty => f.write_str("empty name"),
        }
    }
}

const MAX_COMPONENT: usize = 255;
const MAX_TOTAL: usize = 32_000;

fn is_reserved(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = stem.to_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) {
        return true;
    }
    let digit_ok = |c: char| ('1'..='9').contains(&c) || matches!(c, '¹' | '²' | '³');
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            let mut chars = rest.chars();
            if let (Some(d), None) = (chars.next(), chars.next())
                && digit_ok(d)
            {
                return true;
            }
        }
    }
    false
}

/// Validates a stored name and splits it into components. `.` components and empty ones (`a//b`, `./a`)
/// are dropped; a name that reduces to nothing is `Empty` (a root-folder entry such as `./`).
pub fn sanitize(raw: &str) -> Result<Vec<String>, Unsafe> {
    let name = raw.replace('\\', "/");
    if name.starts_with('/') {
        return Err(Unsafe::Absolute);
    }
    let first = name.split('/').next().unwrap_or("");
    let b = first.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Err(Unsafe::DriveLetter);
    }
    if name.len() > MAX_TOTAL {
        return Err(Unsafe::TooLong);
    }
    let mut parts = Vec::new();
    for comp in name.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            return Err(Unsafe::ParentDir);
        }
        if comp.contains(':') {
            return Err(Unsafe::AlternateStream);
        }
        if let Some(c) = comp
            .chars()
            .find(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*') || (*c as u32) < 0x20)
        {
            return Err(Unsafe::InvalidChar(c));
        }
        if comp.ends_with('.') || comp.ends_with(' ') {
            return Err(Unsafe::TrailingDotOrSpace);
        }
        if is_reserved(comp) {
            return Err(Unsafe::Reserved(comp.to_owned()));
        }
        if comp.len() > MAX_COMPONENT {
            return Err(Unsafe::TooLong);
        }
        parts.push(comp.to_owned());
    }
    if parts.is_empty() {
        return Err(Unsafe::Empty);
    }
    Ok(parts)
}

/// True for symbolic links and junctions (anything that can redirect a path elsewhere).
fn is_redirect(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// Creates the directories `rel[..]` below `root`, refusing to pass through a symbolic link or junction
/// or through a file. `root` itself is trusted (the user chose it).
/// Why a folder could not be prepared.
#[derive(Debug, Clone)]
pub struct DirError {
    pub message: String,
    /// The operating system said "access denied": a retry with more rights might work.
    pub denied: bool,
}

impl From<String> for DirError {
    fn from(message: String) -> Self {
        DirError { message, denied: false }
    }
}

pub fn ensure_dirs(root: &Path, rel: &[String]) -> Result<PathBuf, DirError> {
    let mut cur = root.to_path_buf();
    for part in rel {
        cur.push(part);
        match fs::symlink_metadata(&cur) {
            Ok(m) if is_redirect(&m) => {
                return Err(format!("'{}' is a link; refusing to write through it", cur.display()).into());
            }
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(format!("'{}' exists and is not a folder", cur.display()).into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(&cur) {
                Ok(()) => {}
                // another thread may have created it just now
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if fs::symlink_metadata(&cur)
                        .map(|m| is_redirect(&m) || !m.is_dir())
                        .unwrap_or(true)
                    {
                        return Err(format!("'{}' is not a plain folder", cur.display()).into());
                    }
                }
                Err(e) => {
                    return Err(DirError {
                        message: format!("cannot create '{}': {e}", cur.display()),
                        denied: e.kind() == std::io::ErrorKind::PermissionDenied,
                    });
                }
            },
            Err(e) => return Err(format!("cannot inspect '{}': {e}", cur.display()).into()),
        }
    }
    Ok(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_names() {
        assert_eq!(sanitize("a/b/c.txt").unwrap(), ["a", "b", "c.txt"]);
        assert_eq!(sanitize("./a//b/").unwrap(), ["a", "b"]);
        assert_eq!(sanitize("dir\\sub\\f.txt").unwrap(), ["dir", "sub", "f.txt"]);
        assert_eq!(sanitize("имя/файл.txt").unwrap(), ["имя", "файл.txt"]);
        assert_eq!(sanitize(".hidden").unwrap(), [".hidden"]);
        assert_eq!(sanitize("a/..b/c...d").unwrap(), ["a", "..b", "c...d"]);
    }

    #[test]
    fn rejects_traversal_and_absolute() {
        assert_eq!(sanitize("../evil.txt"), Err(Unsafe::ParentDir));
        assert_eq!(sanitize("a/../../evil.txt"), Err(Unsafe::ParentDir));
        assert_eq!(sanitize("a\\..\\..\\evil.txt"), Err(Unsafe::ParentDir));
        assert_eq!(sanitize("/etc/passwd"), Err(Unsafe::Absolute));
        assert_eq!(sanitize("\\windows\\system32\\x.dll"), Err(Unsafe::Absolute));
        assert_eq!(sanitize("\\\\server\\share\\x"), Err(Unsafe::Absolute));
        assert_eq!(sanitize("//server/share/x"), Err(Unsafe::Absolute));
        assert_eq!(sanitize("C:\\Windows\\x.dll"), Err(Unsafe::DriveLetter));
        assert_eq!(sanitize("c:evil.txt"), Err(Unsafe::DriveLetter));
    }

    #[test]
    fn rejects_windows_tricks() {
        assert!(matches!(sanitize("CON"), Err(Unsafe::Reserved(_))));
        assert!(matches!(sanitize("dir/nul.txt"), Err(Unsafe::Reserved(_))));
        assert!(matches!(sanitize("com1"), Err(Unsafe::Reserved(_))));
        assert!(matches!(sanitize("LPT9.log"), Err(Unsafe::Reserved(_))));
        assert!(matches!(sanitize("COM¹"), Err(Unsafe::Reserved(_))));
        assert!(matches!(sanitize("aux .txt"), Err(Unsafe::Reserved(_))));
        assert_eq!(sanitize("com10").unwrap(), ["com10"]);
        assert_eq!(sanitize("console.txt").unwrap(), ["console.txt"]);
        assert_eq!(sanitize("file.txt::$DATA"), Err(Unsafe::AlternateStream));
        assert_eq!(sanitize("file.txt:hidden"), Err(Unsafe::AlternateStream));
        assert_eq!(sanitize("name."), Err(Unsafe::TrailingDotOrSpace));
        assert_eq!(sanitize("dir/name "), Err(Unsafe::TrailingDotOrSpace));
        assert_eq!(sanitize("a<b"), Err(Unsafe::InvalidChar('<')));
        assert_eq!(sanitize("a\u{1}b"), Err(Unsafe::InvalidChar('\u{1}')));
    }

    #[test]
    fn empty_and_long() {
        assert_eq!(sanitize("./"), Err(Unsafe::Empty));
        assert_eq!(sanitize(""), Err(Unsafe::Empty));
        assert_eq!(sanitize(&"x".repeat(300)), Err(Unsafe::TooLong));
    }
}
