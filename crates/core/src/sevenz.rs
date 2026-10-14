//! 7z through the pure-Rust `sevenz-rust2`. Reading is block by block (a solid block has to be decoded from
//! its start); writing packs files into solid blocks that are compressed on worker threads.

use crate::archive::{Archive, EntryReader, Kind};
use crate::entry::{Entry, EntryKind};
use crate::error::{Error, Result};
use crate::input::InputItem;
use crate::par::OrderedPool;
use crate::progress::Progress;
use crate::source::{SharedSource, SourceReader};
use crate::timeutil;
use sevenz_rust2::encoder_options::{EncoderOptions, Lzma2Options};
use sevenz_rust2::{ArchiveEntry, BlockDecoder, EncoderConfiguration, EncoderMethod, NtTime, Password};
use std::io::{self, Read, Seek, Write};
use std::sync::Arc;

const MAX_BUFFERED_ENTRY: u64 = 512 << 20;

fn map_err(e: sevenz_rust2::Error) -> Error {
    use sevenz_rust2::Error as E;
    match e {
        E::Io(e, _) | E::FileOpen(e, _) => Error::Io(e),
        E::PasswordRequired => Error::Unsupported("the archive is password protected".into()),
        E::MaybeBadPassword(_) => Error::Other("wrong password or damaged data".into()),
        E::UnsupportedCompressionMethod(m) => Error::Unsupported(format!("7z compression method {m}")),
        E::Unsupported(m) => Error::Unsupported(m.into_owned()),
        E::ChecksumVerificationFailed => Error::Integrity("7z checksum mismatch".into()),
        other => Error::Corrupt(other.to_string()),
    }
}

pub struct SevenZArchive {
    src: SharedSource,
    archive: sevenz_rust2::Archive,
    entries: Vec<Entry>,
    password: Password,
}

fn entry_from(index: usize, f: &ArchiveEntry) -> Entry {
    let attrs = f.has_windows_attributes.then_some(f.windows_attributes);
    let unix_mode = attrs.filter(|a| a & 0x8000 != 0).map(|a| a >> 16);
    let kind = if f.is_directory {
        EntryKind::Dir
    } else if unix_mode.is_some_and(|m| m & 0o170_000 == 0o120_000)
        || attrs.is_some_and(|a| a & 0x400 != 0 && f.has_stream)
    {
        EntryKind::Symlink
    } else {
        EntryKind::File
    };
    let mut e = Entry::new(index, f.name.clone(), kind);
    e.size = f.size;
    e.packed = (f.compressed_size > 0).then_some(f.compressed_size);
    e.crc32 = f.has_crc.then_some(f.crc as u32);
    e.method = "7z".into();
    e.mode = unix_mode;
    if f.has_last_modified_date {
        let st: std::time::SystemTime = f.last_modified_date.into();
        e.mtime = Some(timeutil::system_time_to_unix(st));
    }
    e
}

impl SevenZArchive {
    pub fn from_source(src: SharedSource) -> Result<Self> {
        let password = Password::empty();
        let mut rd = SourceReader::new(src.clone());
        let archive = sevenz_rust2::Archive::read(&mut rd, &password).map_err(map_err)?;
        let entries = archive
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| entry_from(i, f))
            .collect();
        Ok(Self {
            src,
            archive,
            entries,
            password,
        })
    }

    pub fn is_solid(&self) -> bool {
        self.archive.is_solid
    }
}

impl Archive for SevenZArchive {
    fn kind(&self) -> Kind {
        Kind::SevenZ
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn open_entry(&self, index: usize) -> Result<EntryReader<'_>> {
        let e = self
            .entries
            .get(index)
            .ok_or_else(|| Error::Other(format!("no entry #{index}")))?;
        if e.size > MAX_BUFFERED_ENTRY {
            return Err(Error::Unsupported(format!(
                "'{}' is too large to open from a 7z archive",
                e.path
            )));
        }
        let mut data = Vec::with_capacity(e.size as usize);
        self.walk(&mut |x| x.index == index, &mut |_, r| {
            r.read_to_end(&mut data)?;
            Ok(())
        })?;
        Ok(Box::new(io::Cursor::new(data)))
    }

    fn walk(
        &self,
        want: &mut dyn FnMut(&Entry) -> bool,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let map = &self.archive.stream_map;
        let mut failure: Option<Error> = None;

        // files without data belong to no block
        for (i, block) in map.file_block_index.iter().enumerate() {
            if block.is_none() && want(&self.entries[i]) {
                visit(&self.entries[i], &mut io::empty())?;
            }
        }

        let mut wanted_blocks = vec![false; self.archive.blocks.len()];
        for (i, block) in map.file_block_index.iter().enumerate() {
            if let Some(b) = block
                && want(&self.entries[i])
            {
                wanted_blocks[*b] = true;
            }
        }

        let mut source = SourceReader::new(self.src.clone());
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
        let base = self.archive.files.as_ptr() as usize;
        let size = std::mem::size_of::<ArchiveEntry>();
        for block_index in (0..self.archive.blocks.len()).filter(|&b| wanted_blocks[b]) {
            let mut each = |ae: &ArchiveEntry, r: &mut dyn Read| -> std::result::Result<bool, sevenz_rust2::Error> {
                // the decoder hands out references into `archive.files`, so the address gives the index
                let index = (ae as *const ArchiveEntry as usize - base) / size;
                let entry = &self.entries[index];
                if !want(entry) {
                    return Ok(true);
                }
                match visit(entry, r) {
                    Ok(()) => Ok(true),
                    Err(e) => {
                        failure = Some(e);
                        Ok(false)
                    }
                }
            };
            let finished = BlockDecoder::new(threads, block_index, &self.archive, &self.password, &mut source)
                .for_each_entries(&mut each)
                .map_err(map_err)?;
            if !finished {
                break;
            }
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SevenZOptions {
    /// LZMA2 level 1..=9 (memory per worker grows quickly above 7).
    pub level: u32,
    pub threads: usize,
    /// Target size of one solid block; blocks are compressed in parallel.
    pub block_bytes: u64,
}

impl Default for SevenZOptions {
    fn default() -> Self {
        Self {
            level: 5,
            threads: 0,
            block_bytes: 64 << 20,
        }
    }
}

enum Piece {
    Dir(ArchiveEntry),
    Empty(ArchiveEntry),
    Block(
        std::result::Result<sevenz_rust2::PreparedBlock, sevenz_rust2::Error>,
        u64,
        u64,
    ),
}

fn entry_for(item: &InputItem) -> ArchiveEntry {
    let name = item.name.trim_end_matches('/').to_owned();
    let mut e = if item.is_dir {
        ArchiveEntry::new_directory(&name)
    } else {
        ArchiveEntry::new_file(&name)
    };
    if let Some(t) = item.mtime.and_then(|t| u64::try_from(t).ok()) {
        let st = std::time::UNIX_EPOCH + std::time::Duration::from_secs(t);
        if let Ok(nt) = NtTime::try_from(st) {
            e.has_last_modified_date = true;
            e.last_modified_date = nt;
        }
    }
    e
}

pub fn write_7z<W: Write + Seek>(out: W, items: &[InputItem], opts: &SevenZOptions, progress: &Progress) -> Result<()> {
    progress.set_totals(
        items.iter().filter(|i| !i.is_dir).count() as u64,
        items.iter().map(|i| i.size).sum(),
    );
    let level = opts.level.clamp(1, 9);

    // plan the solid blocks
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut cur_bytes = 0u64;
    for (i, it) in items.iter().enumerate() {
        if it.is_dir || it.size == 0 {
            continue;
        }
        cur.push(i);
        cur_bytes += it.size;
        if cur_bytes >= opts.block_bytes {
            groups.push(std::mem::take(&mut cur));
            cur_bytes = 0;
        }
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    let threads = crate::par::resolve_threads(opts.threads);
    let lzma = if groups.len() == 1 && threads > 1 {
        Lzma2Options::from_level_mt(level, threads as u32, (opts.block_bytes / 4).max(4 << 20))
    } else {
        Lzma2Options::from_level(level)
    };
    let methods = Arc::new(vec![
        EncoderConfiguration::new(EncoderMethod::LZMA2).with_options(EncoderOptions::Lzma2(lzma)),
    ]);

    let mut writer = sevenz_rust2::ArchiveWriter::new(out).map_err(map_err)?;
    writer.set_content_methods((*methods).clone());
    let mut pool = OrderedPool::<Piece>::new(opts.threads)?;
    let mut deliver = |piece: Piece| -> Result<()> {
        progress.check()?;
        match piece {
            Piece::Dir(e) => {
                writer.push_archive_entry::<io::Empty>(e, None).map_err(map_err)?;
            }
            Piece::Empty(e) => {
                writer.push_archive_entry::<io::Empty>(e, None).map_err(map_err)?;
                progress.add_file();
            }
            Piece::Block(block, bytes, files) => {
                writer.push_prepared_block(block.map_err(map_err)?).map_err(map_err)?;
                progress.add_bytes(bytes);
                for _ in 0..files {
                    progress.add_file();
                }
            }
        }
        Ok(())
    };

    // dirs and empty files go in item order; each group is queued when its first file is reached
    let mut group_of = vec![usize::MAX; items.len()];
    for (g, members) in groups.iter().enumerate() {
        for &i in members {
            group_of[i] = g;
        }
    }
    for (i, it) in items.iter().enumerate() {
        progress.check()?;
        if it.is_dir {
            pool.push_ready(Piece::Dir(entry_for(it)), &mut deliver)?;
        } else if it.size == 0 {
            pool.push_ready(Piece::Empty(entry_for(it)), &mut deliver)?;
        } else if groups[group_of[i]][0] == i {
            let members: Vec<InputItem> = groups[group_of[i]].iter().map(|&m| items[m].clone()).collect();
            let bytes: u64 = members.iter().map(|m| m.size).sum();
            let files = members.len() as u64;
            let methods = methods.clone();
            pool.submit(
                move || {
                    let entries: Vec<ArchiveEntry> = members.iter().map(entry_for).collect();
                    let readers = members
                        .iter()
                        .map(|m| std::fs::File::open(&m.disk_path).map(sevenz_rust2::SourceReader::new))
                        .collect::<io::Result<Vec<_>>>();
                    let block = match readers {
                        Ok(r) => sevenz_rust2::prepare_block(methods, entries, r),
                        Err(e) => Err(sevenz_rust2::Error::Io(e, "cannot open a file to pack".into())),
                    };
                    Piece::Block(block, bytes, files)
                },
                &mut deliver,
            )?;
        }
    }
    pool.finish(&mut deliver)?;
    writer.finish().map_err(Error::from)?;
    Ok(())
}
