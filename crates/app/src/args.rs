//! Command line of the GUI program, including what the Explorer menu starts.

use crate::gui::StartAction;
use std::path::PathBuf;

pub struct Parsed {
    pub open: Vec<PathBuf>,
    pub action: StartAction,
    pub demo: bool,
    pub screenshots: Option<PathBuf>,
    /// The paths arrived together in one start (no need to wait for sibling processes of one selection).
    pub together: bool,
}

/// Flags that take a value and are handled elsewhere (their value must not be mistaken for an archive).
const VALUE_FLAGS: [&str; 1] = ["--screenshots"];

pub fn parse(args: &[String]) -> Parsed {
    let mut p = Parsed {
        open: Vec::new(),
        action: StartAction::None,
        demo: false,
        screenshots: None,
        together: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().map(PathBuf::from);
        match a.as_str() {
            "--demo" => p.demo = true,
            "--screenshots" => p.screenshots = value(),
            "--extract-here" => {
                if let Some(v) = value() {
                    p.action = StartAction::ExtractHere(vec![v]);
                }
            }
            "--extract-to" => {
                if let Some(v) = value() {
                    p.action = StartAction::ExtractTo(vec![v]);
                }
            }
            "--test" => {
                if let Some(v) = value() {
                    p.action = StartAction::Test(vec![v]);
                }
            }
            "--add" => {
                if let Some(v) = value() {
                    p.action = StartAction::Add(vec![v]);
                }
            }
            "--add-all" => {
                // "Send to" hands over every selected path in this one start
                let items: Vec<PathBuf> = it.by_ref().map(PathBuf::from).collect();
                if !items.is_empty() {
                    p.action = StartAction::Add(items);
                    p.together = true;
                }
            }
            "--quick" => {
                let format = it.next().cloned().unwrap_or_default();
                if let Some(v) = it.next().map(PathBuf::from) {
                    p.action = StartAction::Quick { format, items: vec![v] };
                }
            }
            "--elevated-extract" => {
                if let Some(v) = value() {
                    p.action = StartAction::Elevated(v);
                }
            }
            other if !other.starts_with("--") && !VALUE_FLAGS.contains(&other) => p.open.push(PathBuf::from(other)),
            _ => {}
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plain_paths_are_archives_to_open() {
        let p = parse(&args(&[r"C:\a.zip", r"D:\b.7z"]));
        assert_eq!(p.open.len(), 2);
        assert!(matches!(p.action, StartAction::None));
    }

    #[test]
    fn explorer_actions_carry_their_path() {
        assert!(
            matches!(parse(&args(&["--extract-here", r"C:\a b.zip"])).action, StartAction::ExtractHere(p) if p == [PathBuf::from(r"C:\a b.zip")])
        );
        assert!(matches!(
            parse(&args(&["--extract-to", "x.zip"])).action,
            StartAction::ExtractTo(_)
        ));
        assert!(matches!(
            parse(&args(&["--test", "x.zip"])).action,
            StartAction::Test(_)
        ));
        assert!(matches!(parse(&args(&["--add", "f.txt"])).action, StartAction::Add(v) if v.len() == 1));
        let sent = parse(&args(&["--add-all", r"C:.txt", r"C: c.txt", r"C:\dir"]));
        assert!(sent.together && matches!(sent.action, StartAction::Add(v) if v.len() == 3));
        assert!(matches!(parse(&args(&["--add-all"])).action, StartAction::None));
        match parse(&args(&["--quick", "tar.zst", r"C:\data"])).action {
            StartAction::Quick { format, items } => {
                assert_eq!(format, "tar.zst");
                assert_eq!(items, [PathBuf::from(r"C:\data")]);
            }
            _ => panic!("quick action expected"),
        }
        assert!(matches!(
            parse(&args(&["--elevated-extract", "job.json"])).action,
            StartAction::Elevated(_)
        ));
    }

    #[test]
    fn values_of_flags_are_not_archives() {
        let p = parse(&args(&["--screenshots", r"D:\shots", "--demo"]));
        assert!(p.open.is_empty() && p.demo);
        assert_eq!(p.screenshots, Some(PathBuf::from(r"D:\shots")));
    }
}
