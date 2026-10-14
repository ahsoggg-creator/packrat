use packrat_core::create::{CreateOptions, Format, create_archive};
use packrat_core::extract::{self, ExtractOptions, Resolution};
use packrat_core::{Archive, EntryKind, Progress, open, timeutil};
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
packrat-cli — Packrat archive tool

USAGE:
  packrat-cli list <archive>
  packrat-cli extract <archive> [NAMES...] [-o DIR] [--smart] [--threads N]
                      [--skip | --overwrite | --keep-both]
  packrat-cli create <output> <paths...> [--format FMT] [--level N] [--threads N]
  packrat-cli --version | --help

FORMATS (create): zip (deflate), zip-zstd, zip-store, 7z, tar, tar.gz, tar.zst, tar.xz, tar.lz4
  The format is taken from the output extension when --format is not given.

EXTRACT:
  --smart      one root folder goes straight to the target, one file lands next to it,
               loose items get a folder named after the archive
  existing files are skipped unless --overwrite or --keep-both is given

EXIT CODES: 0 success, 1 finished with errors or blocked entries, 2 bad usage or fatal error
";

struct Args(Vec<String>);

impl Args {
    fn flag(&mut self, name: &str) -> bool {
        match self.0.iter().position(|a| a == name) {
            Some(i) => {
                self.0.remove(i);
                true
            }
            None => false,
        }
    }

    fn value(&mut self, names: &[&str]) -> Result<Option<String>, String> {
        let Some(i) = self.0.iter().position(|a| names.contains(&a.as_str())) else {
            return Ok(None);
        };
        if i + 1 >= self.0.len() {
            return Err(format!("{} needs a value", self.0[i]));
        }
        let v = self.0.remove(i + 1);
        self.0.remove(i);
        Ok(Some(v))
    }

    fn number(&mut self, names: &[&str]) -> Result<Option<usize>, String> {
        self.value(names)?
            .map(|v| {
                v.parse()
                    .map_err(|_| format!("{} must be a number, got '{v}'", names[0]))
            })
            .transpose()
    }
}

fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn main() -> ExitCode {
    let mut args = Args(std::env::args().skip(1).collect());
    if args.0.is_empty() {
        print!("{HELP}");
        return ExitCode::from(2);
    }
    if args.flag("--help") || args.flag("-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.flag("--version") || args.flag("-V") {
        println!("packrat-cli {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let cmd = args.0.remove(0);
    let result = match cmd.as_str() {
        "list" | "l" => cmd_list(args),
        "extract" | "x" => cmd_extract(args),
        "create" | "c" | "a" => cmd_create(args),
        other => Err(format!("unknown command '{other}' (see --help)")),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(m) => {
            eprintln!("packrat-cli: {m}");
            ExitCode::from(2)
        }
    }
}

fn open_archive(path: &str) -> Result<Box<dyn Archive>, String> {
    open::open(&PathBuf::from(path)).map_err(|e| format!("{path}: {e}"))
}

fn cmd_list(args: Args) -> Result<u8, String> {
    let path = args.0.first().cloned().ok_or("list: archive path is missing")?;
    let a = open_archive(&path)?;
    println!("{} — {}", path, a.kind().label());
    println!("{:>12} {:>12}  {:<16}  Name", "Size", "Packed", "Modified");
    let (mut files, mut dirs, mut size, mut packed) = (0u64, 0u64, 0u64, 0u64);
    for e in a.entries() {
        let date = e.mtime.map(timeutil::format_minutes).unwrap_or_default();
        let (s, p) = match e.kind {
            EntryKind::Dir => {
                dirs += 1;
                (String::new(), String::new())
            }
            _ => {
                files += 1;
                size += e.size;
                packed += e.packed.unwrap_or(0);
                (e.size.to_string(), e.packed.map(|p| p.to_string()).unwrap_or_default())
            }
        };
        let mark = match e.kind {
            EntryKind::Symlink => " [link]",
            EntryKind::Other => " [special]",
            _ => "",
        };
        let enc = if e.encrypted { " [encrypted]" } else { "" };
        println!("{s:>12} {p:>12}  {date:<16}  {}{mark}{enc}", e.path);
    }
    println!(
        "{files} files, {dirs} folders, {} unpacked, {} packed",
        human(size),
        human(packed)
    );
    Ok(0)
}

fn cmd_extract(mut args: Args) -> Result<u8, String> {
    let smart = args.flag("--smart");
    let overwrite = args.flag("--overwrite");
    let keep_both = args.flag("--keep-both");
    let skip = args.flag("--skip");
    if [overwrite, keep_both, skip].iter().filter(|b| **b).count() > 1 {
        return Err("choose only one of --skip, --overwrite, --keep-both".into());
    }
    let out = args.value(&["-o", "--output"])?.unwrap_or_else(|| ".".into());
    let threads = args.number(&["--threads", "-j"])?.unwrap_or(0);
    let path = args.0.first().cloned().ok_or("extract: archive path is missing")?;
    let wanted: Vec<String> = args.0.iter().skip(1).map(|s| s.replace('\\', "/")).collect();

    let a = open_archive(&path)?;
    let name = PathBuf::from(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut opts = ExtractOptions::new(&out, name);
    opts.smart = smart;
    opts.threads = threads;
    opts.default_resolution = if overwrite {
        Resolution::Replace
    } else if keep_both {
        Resolution::KeepBoth
    } else {
        Resolution::Skip
    };
    if !wanted.is_empty() {
        let sel: Vec<usize> = a
            .entries()
            .iter()
            .filter(|e| {
                let p = e.path.replace('\\', "/");
                wanted
                    .iter()
                    .any(|w| p == *w || p.starts_with(&format!("{}/", w.trim_end_matches('/'))))
            })
            .map(|e| e.index)
            .collect();
        if sel.is_empty() {
            return Err("none of the given names is in the archive".into());
        }
        opts.selection = Some(sel);
    }

    let progress = Progress::new();
    let started = std::time::Instant::now();
    let plan = extract::plan(&*a, &opts);
    println!("extracting to {}", plan.base.display());
    let report = extract::execute(&*a, &plan, &opts, &progress).map_err(|e| e.to_string())?;
    let secs = started.elapsed().as_secs_f64();
    println!(
        "{} files, {} folders, {} in {secs:.2} s",
        report.files,
        report.dirs,
        human(report.bytes)
    );
    for p in &report.skipped_existing {
        println!("skipped (exists): {p}");
    }
    for (from, to) in &report.kept_both {
        println!("kept both: {from} -> {to}");
    }
    for b in &report.blocked {
        eprintln!("BLOCKED {}: {}", b.path, b.message);
    }
    for f in &report.failed {
        eprintln!("FAILED {}: {}", f.path, f.message);
    }
    Ok(u8::from(!report.blocked.is_empty() || !report.failed.is_empty()))
}

fn cmd_create(mut args: Args) -> Result<u8, String> {
    let format_name = args.value(&["--format", "-f"])?;
    let level = args.number(&["--level", "-l"])?;
    let threads = args.number(&["--threads", "-j"])?.unwrap_or(0);
    if args.0.len() < 2 {
        return Err("create: need an output file and at least one path".into());
    }
    let output = PathBuf::from(args.0.remove(0));
    let inputs: Vec<PathBuf> = args.0.iter().map(PathBuf::from).collect();

    let lower = output.to_string_lossy().to_ascii_lowercase();
    let format = match format_name {
        Some(n) => Format::from_name(&n).ok_or_else(|| format!("unknown format '{n}'"))?,
        None => [
            "tar.gz", "tar.zst", "tar.xz", "tar.lz4", "tgz", "tzst", "txz", "tar", "7z", "zip",
        ]
        .iter()
        .find(|e| lower.ends_with(&format!(".{e}")))
        .and_then(|e| Format::from_name(e))
        .unwrap_or(Format::Zip(packrat_core::zip::ZipMethod::Deflate)),
    };
    let mut opts = CreateOptions::new(format);
    if let Some(l) = level {
        opts.level = l.clamp(1, 9) as u32;
    }
    opts.threads = threads;

    let started = std::time::Instant::now();
    let report = create_archive(&output, &inputs, &opts, &Progress::new()).map_err(|e| e.to_string())?;
    let secs = started.elapsed().as_secs_f64();
    println!(
        "{}: {} entries, {} -> {} in {secs:.2} s",
        output.display(),
        report.entries,
        human(report.bytes_in),
        human(report.bytes_out)
    );
    for (p, why) in &report.skipped {
        eprintln!("skipped {}: {why}", p.display());
    }
    Ok(u8::from(!report.skipped.is_empty()))
}
