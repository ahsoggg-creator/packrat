//! The only feature that can need administrator rights: extracting into a folder the user cannot write to
//! (for example under `C:\Program Files`). The main window never elevates. Instead one job description is written
//! to a file and `Packrat.exe --elevated-extract <file>` is started with the "runas" verb: one UAC prompt, one
//! separate window; the original window stays unprivileged.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub archive: PathBuf,
    pub dest: PathBuf,
    pub smart: bool,
    /// Paths of the entries to extract (`None` = everything).
    pub entries: Option<Vec<String>>,
    /// "skip", "replace" or "keep-both".
    pub resolution: String,
}

fn jobs_dir() -> PathBuf {
    std::env::temp_dir().join("Packrat")
}

/// Writes the job to a file only this user can reach through `%TEMP%`.
pub fn write(job: &Job) -> Result<PathBuf, String> {
    let dir = jobs_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = dir.join(format!("admin-job-{}-{millis}.json", std::process::id()));
    std::fs::write(&path, serde_json::to_vec_pretty(job).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Reads a job and deletes its file. Only files inside the Packrat temp folder are accepted.
pub fn read_and_delete(path: &Path) -> Result<Job, String> {
    let ok_place = path
        .canonicalize()
        .ok()
        .zip(jobs_dir().canonicalize().ok())
        .is_some_and(|(p, d)| p.starts_with(d));
    if !ok_place {
        return Err("the job file is not in the Packrat temporary folder".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(path);
    serde_json::from_slice(&bytes).map_err(|e| format!("bad job file: {e}"))
}

/// Starts this program elevated (UAC prompt) for the given job file.
pub fn launch(exe: &Path, job_file: &Path) -> Result<(), String> {
    crate::winint::run_elevated(exe, &format!("--elevated-extract \"{}\"", job_file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_survives_the_trip_and_the_file_is_removed() {
        let job = Job {
            archive: PathBuf::from(r"D:\Архивы\проект.zip"),
            dest: PathBuf::from(r"C:\Program Files\Tool"),
            smart: true,
            entries: Some(vec!["a/b.txt".into(), "c.txt".into()]),
            resolution: "keep-both".into(),
        };
        let file = write(&job).unwrap();
        assert!(file.exists());
        assert_eq!(read_and_delete(&file).unwrap(), job);
        assert!(!file.exists());
    }

    #[test]
    fn job_files_from_elsewhere_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("job.json");
        std::fs::write(&f, "{}").unwrap();
        assert!(read_and_delete(&f).is_err());
        assert!(f.exists(), "an untrusted file is left alone");
    }
}
