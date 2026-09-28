use std::ffi::OsStr;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::writer::SourceOutput;
use crate::{Decompiler, DexFile};

/// Structural statistics collected while parsing a DEX file.
#[derive(Clone, Debug)]
pub struct InspectSummary {
    pub version: u16,
    pub strings: usize,
    pub types: usize,
    pub protos: usize,
    pub fields: usize,
    pub methods: usize,
    pub classes: usize,
    pub encoded_fields: usize,
    pub encoded_methods: usize,
    pub parse_duration: std::time::Duration,
}

/// Thread-safe callback invoked for APK export progress events.
pub type ProgressCallback = Arc<dyn Fn(ExportProgress) + Send + Sync + 'static>;

/// Options shared by directory and `RGOUT001` exports.
///
/// The default output includes DEX method annotations and runs sequentially.

#[derive(Clone)]
pub struct ExportOptions {
    pub thread_count: Option<usize>,
    pub include_annotations: bool,
    pub progress_callback: Option<ProgressCallback>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            thread_count: None,
            include_annotations: true,
            progress_callback: None,
        }
    }
}

impl ExportOptions {
    /// Uses all logical CPUs reported as available to this process.
    pub fn parallel() -> Self {
        Self {
            thread_count: Some(default_thread_count()),
            ..Self::default()
        }
    }

    /// Uses exactly `thread_count` workers. Zero is rejected by export calls.
    pub const fn with_threads(mut self, thread_count: usize) -> Self {
        self.thread_count = Some(thread_count);
        self
    }

    /// Produces annotation-free output compatible with the historical DAD benchmark mode.
    pub const fn dad_compatible(mut self) -> Self {
        self.include_annotations = false;
        self
    }

    /// Invokes `callback` whenever one DEX from an APK finishes.
    pub fn with_progress<F>(mut self, callback: F) -> Self
    where
        F: Fn(ExportProgress) + Send + Sync + 'static,
    {
        self.progress_callback = Some(Arc::new(callback));
        self
    }

    fn validated_threads(&self) -> crate::Result<Option<usize>> {
        if self.thread_count == Some(0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "thread count must be greater than zero",
            )
            .into());
        }
        Ok(self.thread_count)
    }
}

/// Progress events emitted while exporting an APK.
#[derive(Clone, Copy, Debug)]
pub enum ExportProgress {
    Started {
        total_dex_files: usize,
        worker_count: usize,
    },
    DexCompleted {
        completed_dex_files: usize,
        total_dex_files: usize,
        dex_number: u32,
        dex_duration: std::time::Duration,
    },
}

impl ExportProgress {
    pub fn percentage(self) -> f64 {
        match self {
            Self::Started { .. } => 0.0,
            Self::DexCompleted {
                completed_dex_files,
                total_dex_files,
                ..
            } => completed_dex_files as f64 * 100.0 / total_dex_files as f64,
        }
    }
}

/// Aggregate result from an APK or DEX export.
#[derive(Clone, Copy, Debug)]
pub struct ExportSummary {
    pub dex_files: usize,
    pub classes: usize,
    pub duration: std::time::Duration,
}

/// Parses a DEX and returns its structural summary without decompiling it.
pub fn inspect_file(path: impl AsRef<Path>) -> crate::Result<InspectSummary> {
    let bytes = std::fs::read(path)?;
    let started = std::time::Instant::now();
    let dex = DexFile::parse(&bytes)?;
    let parse_duration = started.elapsed();
    Ok(InspectSummary {
        version: dex.header.version,
        strings: dex.strings().len(),
        types: dex.type_ids().len(),
        protos: dex.protos().len(),
        fields: dex.field_ids().len(),
        methods: dex.method_ids().len(),
        classes: dex.classes().len(),
        encoded_fields: dex
            .classes()
            .iter()
            .map(|class| class.fields().count())
            .sum(),
        encoded_methods: dex
            .classes()
            .iter()
            .map(|class| class.methods().count())
            .sum(),
        parse_duration,
    })
}

/// Decompiles one class from a DEX with annotations and member-reference metadata.
pub fn decompile_class_file(
    path: impl AsRef<Path>,
    class_descriptor: &str,
) -> crate::Result<Option<SourceOutput>> {
    let bytes = std::fs::read(path)?;
    Decompiler::parse(&bytes)?.decompile_class_with_metadata(class_descriptor)
}

/// Decompiles one class using annotation-free DAD-compatible rendering.
pub fn decompile_class_file_dad(
    path: impl AsRef<Path>,
    class_descriptor: &str,
) -> crate::Result<Option<String>> {
    let bytes = std::fs::read(path)?;
    Decompiler::parse(&bytes)?.decompile_class_dad(class_descriptor)
}

fn print_inspect(path: &Path) -> crate::Result<()> {
    let summary = inspect_file(path)?;
    println!(
        "version={} strings={} types={} protos={} fields={} methods={} classes={} encoded_fields={} encoded_methods={} parse={:.6}s",
        summary.version,
        summary.strings,
        summary.types,
        summary.protos,
        summary.fields,
        summary.methods,
        summary.classes,
        summary.encoded_fields,
        summary.encoded_methods,
        summary.parse_duration.as_secs_f64(),
    );
    Ok(())
}

fn usage(program: &str) {
    eprintln!("usage: {program} inspect <file.dex>");
    eprintln!("       {program} decompile <file.dex> <class-descriptor>");
    eprintln!("       {program} decompile-dad <file.dex> <class-descriptor>");
    eprintln!("       {program} export-all <file.dex|file.apk> [output-directory]");
    eprintln!(
        "       {program} export-all-parallel <file.dex|file.apk> [thread-count] [output-directory]"
    );
    eprintln!("       {program} export-all-dad <file.dex|file.apk> [output-directory]");
    eprintln!(
        "       {program} export-all-dad-parallel <file.dex|file.apk> [thread-count] [output-directory]"
    );
    eprintln!("       (thread-count defaults to the available logical CPU count)");
}

fn default_thread_count() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
}

fn parse_thread_count(value: &OsStr) -> Option<usize> {
    value
        .to_str()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|thread_count| *thread_count != 0)
}

fn java_relative_path(descriptor: &str) -> Option<PathBuf> {
    let class_name = descriptor.strip_prefix('L')?.strip_suffix(';')?;
    if class_name.is_empty()
        || class_name.split('/').any(|component| {
            component.is_empty()
                || component == "."
                || component == ".."
                || component.contains(['\\', ':', '\0'])
        })
    {
        return None;
    }
    let mut path = PathBuf::new();
    for component in class_name.split('/') {
        path.push(component);
    }
    path.set_extension("java");
    Some(path)
}

fn write_java_class(output_directory: &Path, descriptor: &str, source: &str) -> crate::Result<()> {
    let relative = java_relative_path(descriptor).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid class descriptor for export: {descriptor}"),
        )
    })?;
    let path = output_directory.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, source)?;
    Ok(())
}

fn export_dex_to_directory(
    dex_bytes: &[u8],
    output_directory: &Path,
    thread_count: Option<usize>,
    include_annotations: bool,
) -> crate::Result<usize> {
    let decompiler = Decompiler::parse(dex_bytes)?;
    let classes = match (thread_count, include_annotations) {
        (Some(thread_count), true) => decompiler.decompile_all_parallel(thread_count)?,
        (Some(thread_count), false) => decompiler.decompile_all_dad_parallel(thread_count)?,
        (None, true) => decompiler.decompile_all()?,
        (None, false) => decompiler.decompile_all_dad()?,
    };
    let count = classes.len();
    for (descriptor, source) in classes {
        write_java_class(output_directory, &descriptor, &source.source)?;
    }
    Ok(count)
}

fn dex_entry_number(name: &str) -> Option<u32> {
    if name.contains('/') {
        return None;
    }
    let suffix = name.strip_prefix("classes")?.strip_suffix(".dex")?;
    if suffix.is_empty() {
        return Some(1);
    }
    let number = suffix.parse().ok()?;
    (number >= 2).then_some(number)
}

#[derive(Clone, Copy)]
struct DexZipEntry {
    number: u32,
    data_start: u64,
    compressed_size: u64,
    uncompressed_size: u64,
    crc32: u32,
    compression: u16,
}

fn zip_u16(data: &[u8], offset: usize) -> crate::Result<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| std::io::Error::other("truncated ZIP metadata"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn zip_u32(data: &[u8], offset: usize) -> crate::Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| std::io::Error::other("truncated ZIP metadata"))?;
    Ok(u32::from_le_bytes(
        bytes.try_into().expect("four-byte slice"),
    ))
}

fn zip_u64(data: &[u8], offset: usize) -> crate::Result<u64> {
    let bytes = data
        .get(offset..offset + 8)
        .ok_or_else(|| std::io::Error::other("truncated ZIP metadata"))?;
    Ok(u64::from_le_bytes(
        bytes.try_into().expect("eight-byte slice"),
    ))
}

fn zip64_entry_values(
    extra: &[u8],
    uncompressed_32: u32,
    compressed_32: u32,
    local_offset_32: u32,
) -> crate::Result<(u64, u64, u64)> {
    let mut offset = 0;
    while offset + 4 <= extra.len() {
        let tag = zip_u16(extra, offset)?;
        let size = usize::from(zip_u16(extra, offset + 2)?);
        let data = extra
            .get(offset + 4..offset + 4 + size)
            .ok_or_else(|| std::io::Error::other("truncated ZIP extra field"))?;
        if tag == 1 {
            let mut cursor = 0;
            let mut next = || -> crate::Result<u64> {
                let value = zip_u64(data, cursor)?;
                cursor += 8;
                Ok(value)
            };
            let uncompressed = if uncompressed_32 == u32::MAX {
                next()?
            } else {
                u64::from(uncompressed_32)
            };
            let compressed = if compressed_32 == u32::MAX {
                next()?
            } else {
                u64::from(compressed_32)
            };
            let local_offset = if local_offset_32 == u32::MAX {
                next()?
            } else {
                u64::from(local_offset_32)
            };
            return Ok((uncompressed, compressed, local_offset));
        }
        offset += 4 + size;
    }
    if uncompressed_32 == u32::MAX || compressed_32 == u32::MAX || local_offset_32 == u32::MAX {
        return Err(std::io::Error::other("missing ZIP64 entry metadata").into());
    }
    Ok((
        u64::from(uncompressed_32),
        u64::from(compressed_32),
        u64::from(local_offset_32),
    ))
}

fn apk_dex_entries(input: &Path) -> crate::Result<Vec<DexZipEntry>> {
    const MAX_EOCD_SIZE: u64 = 65_557;
    let mut apk = std::fs::File::open(input)?;
    let file_len = apk.metadata()?.len();
    let tail_len = file_len.min(MAX_EOCD_SIZE) as usize;
    apk.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0; tail_len];
    apk.read_exact(&mut tail)?;
    let eocd = tail
        .windows(4)
        .rposition(|signature| signature == b"PK\x05\x06")
        .ok_or_else(|| std::io::Error::other("APK has no ZIP end record"))?;
    let eocd_absolute = file_len - tail_len as u64 + eocd as u64;
    let entries_16 = zip_u16(&tail, eocd + 10)?;
    let directory_size_32 = zip_u32(&tail, eocd + 12)?;
    let directory_offset_32 = zip_u32(&tail, eocd + 16)?;
    let (entry_count, directory_size, directory_offset) = if entries_16 == u16::MAX
        || directory_size_32 == u32::MAX
        || directory_offset_32 == u32::MAX
    {
        let locator_absolute = eocd_absolute
            .checked_sub(20)
            .ok_or_else(|| std::io::Error::other("invalid ZIP64 locator"))?;
        apk.seek(SeekFrom::Start(locator_absolute))?;
        let mut locator = [0; 20];
        apk.read_exact(&mut locator)?;
        if &locator[..4] != b"PK\x06\x07" {
            return Err(std::io::Error::other("missing ZIP64 locator").into());
        }
        let zip64_offset = zip_u64(&locator, 8)?;
        apk.seek(SeekFrom::Start(zip64_offset))?;
        let mut zip64 = [0; 56];
        apk.read_exact(&mut zip64)?;
        if &zip64[..4] != b"PK\x06\x06" {
            return Err(std::io::Error::other("invalid ZIP64 end record").into());
        }
        (
            zip_u64(&zip64, 32)?,
            zip_u64(&zip64, 40)?,
            zip_u64(&zip64, 48)?,
        )
    } else {
        (
            u64::from(entries_16),
            u64::from(directory_size_32),
            u64::from(directory_offset_32),
        )
    };
    let directory_len = usize::try_from(directory_size)
        .map_err(|_| std::io::Error::other("ZIP directory is too large"))?;
    apk.seek(SeekFrom::Start(directory_offset))?;
    let mut directory = vec![0; directory_len];
    apk.read_exact(&mut directory)?;

    let mut dex_entries = Vec::new();
    let mut cursor = 0;
    for _ in 0..entry_count {
        if directory.get(cursor..cursor + 4) != Some(b"PK\x01\x02") {
            return Err(std::io::Error::other("invalid ZIP central directory entry").into());
        }
        let flags = zip_u16(&directory, cursor + 8)?;
        let compression = zip_u16(&directory, cursor + 10)?;
        let crc32 = zip_u32(&directory, cursor + 16)?;
        let compressed_32 = zip_u32(&directory, cursor + 20)?;
        let uncompressed_32 = zip_u32(&directory, cursor + 24)?;
        let name_len = usize::from(zip_u16(&directory, cursor + 28)?);
        let extra_len = usize::from(zip_u16(&directory, cursor + 30)?);
        let comment_len = usize::from(zip_u16(&directory, cursor + 32)?);
        let local_offset_32 = zip_u32(&directory, cursor + 42)?;
        let name_start = cursor + 46;
        let name_end = name_start + name_len;
        let extra_end = name_end + extra_len;
        let entry_end = extra_end + comment_len;
        let name = directory
            .get(name_start..name_end)
            .ok_or_else(|| std::io::Error::other("truncated ZIP entry name"))?;
        let extra = directory
            .get(name_end..extra_end)
            .ok_or_else(|| std::io::Error::other("truncated ZIP entry extra field"))?;
        if let Ok(name) = std::str::from_utf8(name)
            && let Some(number) = dex_entry_number(name)
        {
            if flags & 1 != 0 {
                return Err(std::io::Error::other("encrypted DEX entry is unsupported").into());
            }
            let (uncompressed_size, compressed_size, local_offset) =
                zip64_entry_values(extra, uncompressed_32, compressed_32, local_offset_32)?;
            apk.seek(SeekFrom::Start(local_offset))?;
            let mut local_header = [0; 30];
            apk.read_exact(&mut local_header)?;
            if &local_header[..4] != b"PK\x03\x04" {
                return Err(std::io::Error::other("invalid ZIP local header").into());
            }
            let local_name_len = u64::from(zip_u16(&local_header, 26)?);
            let local_extra_len = u64::from(zip_u16(&local_header, 28)?);
            dex_entries.push(DexZipEntry {
                number,
                data_start: local_offset + 30 + local_name_len + local_extra_len,
                compressed_size,
                uncompressed_size,
                crc32,
                compression,
            });
        }
        cursor = entry_end;
    }
    dex_entries.sort_unstable_by_key(|entry| entry.number);
    if dex_entries.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "APK contains no classes*.dex entries",
        )
        .into());
    }
    if dex_entries
        .windows(2)
        .any(|entries| entries[0].number == entries[1].number)
    {
        return Err(std::io::Error::other("APK contains duplicate classes*.dex entries").into());
    }
    Ok(dex_entries)
}

fn dex_entry_name(number: u32) -> String {
    if number == 1 {
        "classes.dex".to_owned()
    } else {
        format!("classes{number}.dex")
    }
}

fn report_apk_progress(
    callback: Option<&(dyn Fn(ExportProgress) + Send + Sync)>,
    completed: usize,
    total: usize,
    number: u32,
    duration: std::time::Duration,
) {
    if let Some(callback) = callback {
        callback(ExportProgress::DexCompleted {
            completed_dex_files: completed,
            total_dex_files: total,
            dex_number: number,
            dex_duration: duration,
        });
    }
}

fn read_apk_dex(
    apk: &mut std::fs::File,
    entry: DexZipEntry,
    decompressor: &mut libdeflater::Decompressor,
) -> crate::Result<Vec<u8>> {
    let compressed_size = usize::try_from(entry.compressed_size).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "compressed DEX is too large",
        )
    })?;
    let uncompressed_size = usize::try_from(entry.uncompressed_size)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "DEX is too large"))?;
    apk.seek(SeekFrom::Start(entry.data_start))?;
    let mut compressed = vec![0; compressed_size];
    apk.read_exact(&mut compressed)?;
    let dex_bytes = if entry.compression == 0 {
        if compressed_size != uncompressed_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stored DEX has inconsistent sizes",
            )
            .into());
        }
        compressed
    } else if entry.compression == 8 {
        let mut output = vec![0; uncompressed_size];
        let written = decompressor
            .deflate_decompress(&compressed, &mut output)
            .map_err(std::io::Error::other)?;
        if written != uncompressed_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "libdeflate returned an unexpected DEX size",
            )
            .into());
        }
        output
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "unsupported DEX ZIP compression method {}",
                entry.compression
            ),
        )
        .into());
    };
    if libdeflater::crc32(&dex_bytes) != entry.crc32 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "DEX ZIP CRC-32 mismatch",
        )
        .into());
    }
    Ok(dex_bytes)
}

fn process_apk_dexes_parallel<T, F>(
    input: &Path,
    thread_count: usize,
    progress_callback: Option<&(dyn Fn(ExportProgress) + Send + Sync)>,
    process: F,
) -> crate::Result<Vec<T>>
where
    T: Send,
    F: Fn(&[u8], usize) -> crate::Result<T> + Sync,
{
    let dex_entries = apk_dex_entries(input)?;

    let worker_count = thread_count.min(dex_entries.len()).max(1);
    let threads_per_dex = if dex_entries.len() == 1 {
        thread_count
    } else {
        1
    };
    let next = AtomicUsize::new(0);
    let completed_count = AtomicUsize::new(0);
    if let Some(callback) = progress_callback {
        callback(ExportProgress::Started {
            total_dex_files: dex_entries.len(),
            worker_count,
        });
    }
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let dex_entries = &dex_entries;
            let next = &next;
            let completed_count = &completed_count;
            let process = &process;
            handles.push(scope.spawn(move || {
                let mut apk = std::fs::File::open(input)?;
                let mut decompressor = libdeflater::Decompressor::new();
                let mut completed = Vec::new();
                loop {
                    let position = next.fetch_add(1, Ordering::Relaxed);
                    let Some(entry) = dex_entries.get(position).copied() else {
                        break;
                    };
                    let started = std::time::Instant::now();
                    let dex_bytes = read_apk_dex(&mut apk, entry, &mut decompressor)?;
                    completed.push((position, process(&dex_bytes, threads_per_dex)?));
                    let completed = completed_count.fetch_add(1, Ordering::Relaxed) + 1;
                    report_apk_progress(
                        progress_callback,
                        completed,
                        dex_entries.len(),
                        entry.number,
                        started.elapsed(),
                    );
                }
                Ok::<_, crate::Error>(completed)
            }));
        }

        let mut ordered: Vec<Option<T>> = std::iter::repeat_with(|| None)
            .take(dex_entries.len())
            .collect();
        for handle in handles {
            let completed = handle
                .join()
                .map_err(|_| std::io::Error::other("APK decompiler worker panicked"))??;
            for (position, result) in completed {
                ordered[position] = Some(result);
            }
        }
        ordered
            .into_iter()
            .map(|result| {
                result.ok_or_else(|| {
                    std::io::Error::other("APK decompiler worker returned no result").into()
                })
            })
            .collect()
    })
}

fn export_apk_to_directory(
    input: &Path,
    output_directory: &Path,
    thread_count: Option<usize>,
    include_annotations: bool,
    progress_callback: Option<&(dyn Fn(ExportProgress) + Send + Sync)>,
) -> crate::Result<(usize, usize)> {
    let dex_entries = apk_dex_entries(input)?;

    if let Some(callback) = progress_callback {
        callback(ExportProgress::Started {
            total_dex_files: dex_entries.len(),
            worker_count: thread_count.unwrap_or(1),
        });
    }
    let mut apk = std::fs::File::open(input)?;
    let mut decompressor = libdeflater::Decompressor::new();
    let mut class_count = 0;
    for (position, entry) in dex_entries.iter().copied().enumerate() {
        let started = std::time::Instant::now();
        let dex_bytes = read_apk_dex(&mut apk, entry, &mut decompressor)?;
        class_count += export_dex_to_directory(
            &dex_bytes,
            output_directory,
            thread_count,
            include_annotations,
        )?;
        report_apk_progress(
            progress_callback,
            position + 1,
            dex_entries.len(),
            entry.number,
            started.elapsed(),
        );
    }
    Ok((dex_entries.len(), class_count))
}

fn export_apk_parallel_to_directory(
    input: &Path,
    output_directory: &Path,
    thread_count: usize,
    include_annotations: bool,
    progress_callback: Option<&(dyn Fn(ExportProgress) + Send + Sync)>,
) -> crate::Result<(usize, usize)> {
    let counts = process_apk_dexes_parallel(
        input,
        thread_count,
        progress_callback,
        |dex_bytes, threads_per_dex| {
            export_dex_to_directory(
                dex_bytes,
                output_directory,
                (threads_per_dex > 1).then_some(threads_per_dex),
                include_annotations,
            )
        },
    )?;
    Ok((counts.len(), counts.into_iter().sum()))
}

fn write_source_outputs<W: Write>(
    output: &mut W,
    classes: Vec<(String, SourceOutput)>,
) -> crate::Result<()> {
    for (descriptor, source) in classes {
        write_class_record(output, &descriptor, &source.source)?;
    }
    Ok(())
}

fn write_class_record<W: Write>(
    output: &mut W,
    descriptor: &str,
    source: &str,
) -> crate::Result<()> {
    output.write_all(&(descriptor.len() as u32).to_le_bytes())?;
    output.write_all(&(source.len() as u64).to_le_bytes())?;
    output.write_all(descriptor.as_bytes())?;
    output.write_all(source.as_bytes())?;
    Ok(())
}

struct BinaryDexOutput {
    class_count: u32,
    payload: Vec<u8>,
}

fn decompile_dex_binary(
    dex_bytes: &[u8],
    thread_count: usize,
    include_annotations: bool,
) -> crate::Result<BinaryDexOutput> {
    let decompiler = Decompiler::parse(dex_bytes)?;
    let mut payload = Vec::new();
    let classes = match (thread_count > 1, include_annotations) {
        (true, true) => decompiler.decompile_all_parallel(thread_count)?,
        (true, false) => decompiler.decompile_all_dad_parallel(thread_count)?,
        (false, true) => decompiler.decompile_all()?,
        (false, false) => decompiler.decompile_all_dad()?,
    };
    let class_count = classes.len() as u32;
    write_source_outputs(&mut payload, classes)?;
    Ok(BinaryDexOutput {
        class_count,
        payload,
    })
}

fn cli_progress(progress: ExportProgress) {
    match progress {
        ExportProgress::Started {
            total_dex_files,
            worker_count,
        } => eprintln!("APK export: {total_dex_files} DEX files, {worker_count} worker(s)"),
        ExportProgress::DexCompleted {
            completed_dex_files,
            total_dex_files,
            dex_number,
            dex_duration,
        } => eprintln!(
            "[apk {completed_dex_files}/{total_dex_files} {:5.1}%] {} complete in {:.3}s",
            progress.percentage(),
            dex_entry_name(dex_number),
            dex_duration.as_secs_f64(),
        ),
    }
}

fn export_binary(input: &Path, options: ExportOptions) -> crate::Result<()> {
    let stdout = std::io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    export_to_writer(input, &mut output, options)?;
    output.flush()?;
    Ok(())
}

/// Exports every class from a DEX or APK into package directories as one `.java` file per class.
///
/// Annotations are included by default through [`ExportOptions::default`].
pub fn export_to_directory(
    input: impl AsRef<Path>,
    output_directory: impl AsRef<Path>,
    options: ExportOptions,
) -> crate::Result<ExportSummary> {
    let input = input.as_ref();
    let output_directory = output_directory.as_ref();
    let thread_count = options.validated_threads()?;
    std::fs::create_dir_all(output_directory)?;
    let started = std::time::Instant::now();
    let (dex_files, classes) = if input
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(OsStr::new("apk")))
    {
        if let Some(thread_count) = thread_count {
            export_apk_parallel_to_directory(
                input,
                output_directory,
                thread_count,
                options.include_annotations,
                options.progress_callback.as_deref(),
            )?
        } else {
            export_apk_to_directory(
                input,
                output_directory,
                None,
                options.include_annotations,
                options.progress_callback.as_deref(),
            )?
        }
    } else {
        let bytes = std::fs::read(input)?;
        (
            1,
            export_dex_to_directory(
                &bytes,
                output_directory,
                thread_count,
                options.include_annotations,
            )?,
        )
    };
    Ok(ExportSummary {
        dex_files,
        classes,
        duration: started.elapsed(),
    })
}

/// Writes all decompiled classes to an `RGOUT001` stream.
///
/// This is primarily useful for bulk integrations that want to avoid creating
/// hundreds of thousands of files. Annotations are included by default.
pub fn export_to_writer(
    input: impl AsRef<Path>,
    output: &mut impl Write,
    options: ExportOptions,
) -> crate::Result<ExportSummary> {
    let input = input.as_ref();
    let thread_count = options.validated_threads()?;
    let started = std::time::Instant::now();
    let apk = input
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(OsStr::new("apk")));
    let (dex_files, dex_outputs) = if apk {
        let dex_outputs = if let Some(thread_count) = thread_count {
            process_apk_dexes_parallel(
                input,
                thread_count,
                options.progress_callback.as_deref(),
                |dex_bytes, threads_per_dex| {
                    decompile_dex_binary(dex_bytes, threads_per_dex, options.include_annotations)
                },
            )?
        } else {
            let dex_entries = apk_dex_entries(input)?;
            if let Some(callback) = options.progress_callback.as_deref() {
                callback(ExportProgress::Started {
                    total_dex_files: dex_entries.len(),
                    worker_count: 1,
                });
            }
            let mut apk = std::fs::File::open(input)?;
            let mut decompressor = libdeflater::Decompressor::new();
            let mut outputs = Vec::with_capacity(dex_entries.len());
            for (position, entry) in dex_entries.iter().copied().enumerate() {
                let dex_started = std::time::Instant::now();
                let dex_bytes = read_apk_dex(&mut apk, entry, &mut decompressor)?;
                outputs.push(decompile_dex_binary(
                    &dex_bytes,
                    1,
                    options.include_annotations,
                )?);
                report_apk_progress(
                    options.progress_callback.as_deref(),
                    position + 1,
                    dex_entries.len(),
                    entry.number,
                    dex_started.elapsed(),
                );
            }
            outputs
        };
        (dex_outputs.len(), dex_outputs)
    } else {
        let bytes = std::fs::read(input)?;
        (
            1,
            vec![decompile_dex_binary(
                &bytes,
                thread_count.unwrap_or(1),
                options.include_annotations,
            )?],
        )
    };
    let classes = dex_outputs.iter().try_fold(0_usize, |count, dex| {
        count.checked_add(dex.class_count as usize).ok_or_else(|| {
            crate::Error::from(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "too many classes",
            ))
        })
    })?;
    let duration = started.elapsed();
    output.write_all(b"RGOUT001")?;
    output.write_all(&duration.as_secs_f64().to_le_bytes())?;
    output.write_all(&(classes as u32).to_le_bytes())?;
    for dex in dex_outputs {
        output.write_all(&dex.payload)?;
    }
    Ok(ExportSummary {
        dex_files,
        classes,
        duration,
    })
}

pub fn run() -> crate::Result<()> {
    let mut args = std::env::args_os();
    let program = args
        .next()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "radguard".to_owned());
    let command = args.next();
    let path = args.next();
    match command.as_deref() {
        Some(command) if command == std::ffi::OsStr::new("inspect") => {
            let Some(path) = path else {
                usage(&program);
                std::process::exit(2);
            };
            if args.next().is_some() {
                usage(&program);
                std::process::exit(2);
            }
            print_inspect(Path::new(&path))
        }
        Some(command) if command == std::ffi::OsStr::new("decompile") => {
            let Some(path) = path else {
                usage(&program);
                std::process::exit(2);
            };
            let Some(class) = args.next().and_then(|value| value.into_string().ok()) else {
                usage(&program);
                std::process::exit(2);
            };
            if args.next().is_some() {
                usage(&program);
                std::process::exit(2);
            }
            let source = decompile_class_file(Path::new(&path), &class)?
                .map(|output| output.source)
                .unwrap_or_else(|| {
                    format!("Error: Class {class} not found in the reconstructed DEX.")
                });
            print!("{source}");
            Ok(())
        }
        Some(command) if command == std::ffi::OsStr::new("decompile-dad") => {
            let Some(path) = path else {
                usage(&program);
                std::process::exit(2);
            };
            let Some(class) = args.next().and_then(|value| value.into_string().ok()) else {
                usage(&program);
                std::process::exit(2);
            };
            if args.next().is_some() {
                usage(&program);
                std::process::exit(2);
            }
            let source = decompile_class_file_dad(Path::new(&path), &class)?.unwrap_or_else(|| {
                format!("Error: Class {class} not found in the reconstructed DEX.")
            });
            print!("{source}");
            Ok(())
        }
        Some(command)
            if command == std::ffi::OsStr::new("export-all")
                || command == std::ffi::OsStr::new("export-all-dad") =>
        {
            let dad_compatible = command == std::ffi::OsStr::new("export-all-dad");
            let Some(path) = path else {
                usage(&program);
                std::process::exit(2);
            };
            let output_directory = args.next();
            if args.next().is_some() {
                usage(&program);
                std::process::exit(2);
            }
            let options = if dad_compatible {
                ExportOptions::default().dad_compatible()
            } else {
                ExportOptions::default()
            }
            .with_progress(cli_progress);
            if let Some(output_directory) = output_directory {
                let summary =
                    export_to_directory(Path::new(&path), Path::new(&output_directory), options)?;
                eprintln!(
                    "exported {} classes from {} DEX file(s) to {} in {:.6}s",
                    summary.classes,
                    summary.dex_files,
                    Path::new(&output_directory).display(),
                    summary.duration.as_secs_f64(),
                );
                return Ok(());
            }
            export_binary(Path::new(&path), options)
        }
        Some(command)
            if command == std::ffi::OsStr::new("export-all-parallel")
                || command == std::ffi::OsStr::new("export-all-dad-parallel") =>
        {
            let dad_compatible = command == std::ffi::OsStr::new("export-all-dad-parallel");
            let Some(path) = path else {
                usage(&program);
                std::process::exit(2);
            };
            let first_optional = args.next();
            let second_optional = args.next();
            if args.next().is_some() {
                usage(&program);
                std::process::exit(2);
            }
            let (thread_count, output_directory) = match (first_optional, second_optional) {
                (None, None) => (default_thread_count(), None),
                (Some(value), None) => match parse_thread_count(&value) {
                    Some(thread_count) => (thread_count, None),
                    None => (default_thread_count(), Some(value)),
                },
                (Some(value), Some(output_directory)) => {
                    let Some(thread_count) = parse_thread_count(&value) else {
                        usage(&program);
                        std::process::exit(2);
                    };
                    (thread_count, Some(output_directory))
                }
                (None, Some(_)) => unreachable!("second argument requires the first argument"),
            };
            let options = if dad_compatible {
                ExportOptions::default().dad_compatible()
            } else {
                ExportOptions::default()
            }
            .with_threads(thread_count)
            .with_progress(cli_progress);
            if let Some(output_directory) = output_directory {
                let summary =
                    export_to_directory(Path::new(&path), Path::new(&output_directory), options)?;
                eprintln!(
                    "exported {} classes from {} DEX file(s) to {} in {:.6}s",
                    summary.classes,
                    summary.dex_files,
                    Path::new(&output_directory).display(),
                    summary.duration.as_secs_f64(),
                );
                return Ok(());
            }
            export_binary(Path::new(&path), options)
        }
        _ => {
            usage(&program);
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{dex_entry_number, java_relative_path, parse_thread_count};
    use std::ffi::OsStr;
    use std::path::Path;

    #[test]
    fn java_export_path_follows_package_structure() {
        assert_eq!(
            java_relative_path("Lcom/example/Outer$Inner;"),
            Some(Path::new("com/example/Outer$Inner.java").to_owned())
        );
        assert_eq!(java_relative_path("L../Escape;"), None);
        assert_eq!(java_relative_path("L..\\Escape;"), None);
        assert_eq!(java_relative_path("LC:/Escape;"), None);
        assert_eq!(java_relative_path("not-a-descriptor"), None);
    }

    #[test]
    fn apk_dex_entries_use_multidex_order() {
        assert_eq!(dex_entry_number("classes.dex"), Some(1));
        assert_eq!(dex_entry_number("classes2.dex"), Some(2));
        assert_eq!(dex_entry_number("classes10.dex"), Some(10));
        assert_eq!(dex_entry_number("classes0.dex"), None);
        assert_eq!(dex_entry_number("assets/classes2.dex"), None);
        assert_eq!(dex_entry_number("classes.txt"), None);
    }

    #[test]
    fn optional_parallel_argument_distinguishes_threads_from_directory() {
        assert_eq!(parse_thread_count(OsStr::new("8")), Some(8));
        assert_eq!(parse_thread_count(OsStr::new("0")), None);
        assert_eq!(parse_thread_count(OsStr::new("output")), None);
    }
}
