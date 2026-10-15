# Packrat

[English](README.en.md) · [Русский](README.md)

An archive manager for Windows (a replacement for 7-Zip + Bandizip + PeaZip): compression and extraction on all
cores, "smart" extraction without nested `Folder\Folder`, safe handling of untrusted archives, and dragging files
straight from the window into Explorer. Written in Rust, with no native C compression libraries.

![Archive window: tree, sizes, compression ratio](docs/screenshots/02-archive.png)

*All screenshots were taken with made-up data (`Packrat.exe --demo`) — no real file names appear on them.*

## Features (version 0.1.0)

- **Formats.** Read and write: ZIP (store, deflate, zstd) and TAR (including `.tar.gz`, `.tar.zst`, `.tar.xz`,
  `.tar.lz4`). Read: 7z (no password; LZMA2, BZip2 and PPMd verified on 7-Zip archives, other methods through the library), single `.gz`, `.bz2`, `.xz`, `.zst`,
  `.lz4` files. Creating 7z (LZMA2) works too. RAR is not supported: the unrar decoder's license forbids bundling it.
- **All cores.** Compression: ZIP entries and blocks of large files are compressed in parallel (deflate in
  independent blocks like pigz, zstd and lz4 in independent frames, 7z in parallel LZMA2 blocks, xz in its own
  blocks). Independent ZIP entries are extracted in parallel. The thread count is configurable (`--threads N`,
  Settings). Measured speed-ups: [docs/BENCHMARKS.md](docs/BENCHMARKS.md).
- **Smart Extract.** One top-level folder goes straight into the chosen folder (no `Folder\Folder`); a single
  file lands next to the archive; several loose items go into a new folder named after the archive. `./` entries
  and folders that differ only by letter case are handled correctly.
- **Nothing is overwritten without your decision.** Before extracting, Packrat builds a plan and shows which files
  already exist: "skip" (the default), "replace" or "keep both" (`name (2).txt`).
- **Path safety.** Entries with `..`, absolute paths, drive letters, UNC paths, reserved names (`CON`, `NUL`,
  `COM1`…), NTFS alternate streams (`name:stream`, `::$DATA`), trailing dots or spaces are not extracted; they end
  up in the "blocked" list. Symbolic links from archives are never created, and writing through an existing link
  or junction is refused.
- **Window.** Content tree (name, size, compression, date, method), sorting, multi-selection, extraction by button
  or by dragging the selection into Explorer, dropping an archive or files onto the window (opens the archive or
  offers to create a new one). Russian and English interface.
- **Command line.** `packrat-cli list | extract | create`, `--smart`, `--threads N`.
- **Auto-update.** Once a day one request to `api.github.com`; an "Update" button, download, SHA-256 check against
  `SHA256SUMS.txt`, file replacement and restart; `Packrat.exe --update` does the same without a window. It can be
  turned off in Settings.
- **Diagnostics, demo.** A log and crash reports in `%APPDATA%\Packrat\logs`; `Packrat.exe --demo` shows the
  program with made-up data.

## Install

Download `Packrat-Setup.exe` from [Releases](https://github.com/ahsoggg-creator/packrat/releases/latest) and run
it. No administrator rights are needed: the program is installed to `%LOCALAPPDATA%\Programs\Packrat`, a Start
menu shortcut is added, and an "Apps & features" entry lets you uninstall it.

There is also a portable version — just `Packrat.exe`; it can be installed later from Settings. `packrat-cli.exe`
is the console version.

### Code signing

The release executables are **not signed**: SmartScreen may show an "unknown publisher" warning ("More info" →
"Run anyway"). To check that a file is intact and matches what was published, compare it with `SHA256SUMS.txt` from
the same release. The repository has scripts for signing with a self-signed certificate (`scripts\make_cert.ps1`,
`scripts\trust_cert.ps1`, `scripts\release.ps1`); the private key stays with the author, and `*.pfx` files are
excluded from git.

## Diagnostics and privacy

Packrat sends nothing anywhere except the update check on GitHub (once a day, can be turned off in Settings).
There is no telemetry. Stored locally:

- `%APPDATA%\Packrat\logs\packrat.log` — program events (rotated at 512 KB). Passwords, names of files inside
  archives and their contents are not logged; the user's profile folder is replaced with `%USERPROFILE%`;
- `%APPDATA%\Packrat\logs\crashes\crash-<time>.txt` — a crash report: version, panic location, stack and the log tail.

Settings → Diagnostics opens this folder or copies the diagnostic text to the clipboard.

## Build

Requires Rust (stable, edition 2024) and MSVC.

```powershell
cargo build --release            # target\release\Packrat.exe and packrat-cli.exe
cargo test --workspace --release
powershell -ExecutionPolicy Bypass -File scripts\release.ps1   # checks, build, dist\ with checksums
powershell -ExecutionPolicy Bypass -File scripts\bench.ps1     # speed measurements
```

Layout:

- `crates/core` — formats, smart extraction, safety checks, tests (no UI);
- `crates/app` — egui GUI, installer, auto-update, diagnostics, demo mode;
- `crates/cli` — `packrat-cli`;
- `scripts/` — release build, measurements, certificate; `docs/` — measurements, release notes, screenshots.

The same exe is an installer when it is named `Packrat-Setup.exe` (or started with `--install`) and an
uninstaller with `--uninstall`. To regenerate the screenshots (made-up data, nothing real is read or written):

```powershell
Packrat.exe --screenshots docs\screenshots
```

## Limitations

- RAR is not supported (license). ZIP methods deflate64, bzip2 and LZMA cannot be read; passwords and encryption
  come in 0.2.0.
- zstd compression is pure Rust (`ruzstd`) with a single level (about `zstd -1`): fast, but weaker than the official
  encoder's higher levels.
- The listing of `.tar.*` files is built with one pass over the whole archive; for big archives it is not instant.
- 7z is created with LZMA2 only and without a password. `.tar.lz4` files do not open in stock 7-Zip (it has no LZ4).
- Dragging files from the window into Explorer: the data object is covered by a test, but the mouse gesture itself
  has not been verified automatically on a real system.
- Name-encoding detection, archive-bomb protection, in-archive file preview, the Explorer menu and file
  associations arrive in the next versions (0.2.0 and 0.3.0).

## License

At your option:

- Apache License 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))
