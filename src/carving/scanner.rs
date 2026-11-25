use anyhow::{Context, Result, bail};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::disk::reader::DiskImageReader;

use super::signatures::FileFormat;
use super::validator;

const CHUNK_SIZE: usize = 1024 * 1024;
const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct CarvedFile {
    pub format: FileFormat,
    pub path: PathBuf,
    pub size: u64,
}

fn find_pattern(
    reader: &mut DiskImageReader,
    start: u64,
    end: u64,
    pattern: &[u8],
) -> Result<Option<u64>> {
    if pattern.is_empty() || start >= end {
        return Ok(None);
    }

    let mut cursor = start;
    let mut carry = Vec::new();

    while cursor < end {
        let remaining = end - cursor;
        let length = usize::try_from(remaining.min(CHUNK_SIZE as u64))?;

        let chunk = reader.read_bytes(cursor, length)?;

        let mut data = carry.clone();
        data.extend_from_slice(&chunk);

        let base_offset = cursor - carry.len() as u64;

        if data.len() >= pattern.len() {
            for index in 0..=data.len() - pattern.len() {
                let absolute_offset = base_offset + index as u64;

                if absolute_offset >= start
                    && absolute_offset + pattern.len() as u64 <= end
                    && &data[index..index + pattern.len()] == pattern
                {
                    return Ok(Some(absolute_offset));
                }
            }
        }

        let keep = pattern.len().saturating_sub(1);

        carry = if keep == 0 {
            Vec::new()
        } else {
            data[data.len().saturating_sub(keep)..].to_vec()
        };

        cursor += length as u64;
    }

    Ok(None)
}

fn save_carved_file(
    output_dir: &Path,
    format: FileFormat,
    sequence: &mut u64,
    data: &[u8],
) -> Result<PathBuf> {
    loop {
        let filename = format!("recovered_{:06}.{}", *sequence, format.extension());

        *sequence += 1;

        let path = output_dir.join(filename);

        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                continue;
            }
            Err(error) => {
                return Err(error).context("Failed to create recovered file");
            }
        };

        if let Err(error) = file.write_all(data) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error).context("Failed to write recovered file");
        }

        return Ok(path);
    }
}

fn scan_format(
    reader: &mut DiskImageReader,
    output_dir: &Path,
    format: FileFormat,
) -> Result<Vec<CarvedFile>> {
    let mut recovered = Vec::new();
    let mut cursor = 0u64;
    let image_size = reader.size();
    let mut sequence = 1u64;

    while cursor < image_size {
        let Some(header_offset) = find_pattern(reader, cursor, image_size, format.header())? else {
            break;
        };

        let footer_start = header_offset
            .checked_add(format.header().len() as u64)
            .context("Header offset overflow")?;

        let maximum_end = header_offset
            .checked_add(MAX_FILE_SIZE)
            .unwrap_or(u64::MAX)
            .min(image_size);

        let footer_offset = find_pattern(reader, footer_start, maximum_end, format.footer())?;

        let Some(footer_offset) = footer_offset else {
            // Skip this candidate and continue searching.
            cursor = header_offset + 1;
            continue;
        };

        let file_end = footer_offset
            .checked_add(format.footer().len() as u64)
            .context("Footer offset overflow")?;

        let file_size = file_end - header_offset;
        let read_length =
            usize::try_from(file_size).context("Recovered file is too large for this platform")?;

        let data = reader.read_bytes(header_offset, read_length)?;

        let validation = match format {
            FileFormat::Png => validator::validate_png(&data),
            FileFormat::Jpeg => validator::validate_jpeg(&data),
            FileFormat::Pdf => validator::validate_pdf(&data),
        };

        if let Err(reason) = validation {
            println!(
                "Skipping invalid {} candidate at offset {}: {}",
                format.name(),
                header_offset,
                reason
            );

            cursor = header_offset + 1;
            continue;
        }

        if format == FileFormat::Png {
            if let Err(reason) = validator::validate_png(&data) {
                println!(
                    "Skipping invalid PNG candidate at offset {}: {}",
                    header_offset, reason
                );

                cursor = header_offset + 1;
                continue;
            }
        }

        let path = save_carved_file(output_dir, format, &mut sequence, &data)?;

        recovered.push(CarvedFile {
            format,
            path,
            size: file_size,
        });

        cursor = file_end;
    }

    Ok(recovered)
}

pub fn scan_image(
    reader: &mut DiskImageReader,
    output_dir: impl AsRef<Path>,
) -> Result<Vec<CarvedFile>> {
    let output_dir = output_dir.as_ref();

    if reader.size() == 0 {
        bail!("Disk image is empty.");
    }

    fs::create_dir_all(output_dir).context("Failed to create recovery output directory")?;

    let mut recovered = Vec::new();

    for format in FileFormat::all() {
        println!("Scanning for {} files...", format.name());

        let files = scan_format(reader, output_dir, *format)?;

        println!("Found {} {} file(s).", files.len(), format.name());

        recovered.extend(files);
    }

    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temporary_path(label: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);

        std::env::temp_dir().join(format!("recuvax-{label}-{}-{id}", std::process::id()))
    }

    #[test]
    fn finds_pattern_across_chunk_boundary() {
        let image_path = temporary_path("boundary");

        let mut image = vec![0xAA; CHUNK_SIZE - 3];
        image.extend_from_slice(b"HEADER");
        image.extend_from_slice(&[0xBB; 20]);

        fs::write(&image_path, image).unwrap();

        let mut reader = DiskImageReader::open(&image_path).unwrap();

        let image_size = reader.size();

        let result = find_pattern(&mut reader, 0, image_size, b"HEADER");

        drop(reader);
        let _ = fs::remove_file(image_path);

        assert_eq!(result.unwrap(), Some((CHUNK_SIZE - 3) as u64));
    }

    #[test]
    fn carves_synthetic_png() {
        let image_path = temporary_path("png-image");
        let output_dir = temporary_path("png-output");

        let mut image = vec![0xAA; 100];
        image.extend_from_slice(&validator::test_png());
        image.extend_from_slice(&[0xBB; 100]);

        fs::write(&image_path, image).unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let mut reader = DiskImageReader::open(&image_path).unwrap();

        let result = scan_format(&mut reader, &output_dir, FileFormat::Png).unwrap();

        assert_eq!(result.len(), 1);

        let recovered = fs::read(&result[0].path).unwrap();
        assert!(validator::validate_png(&recovered).is_ok());

        drop(reader);
        let _ = fs::remove_file(image_path);
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn carves_synthetic_jpeg() {
        let image_path = temporary_path("jpeg-image");
        let output_dir = temporary_path("jpeg-output");

        let mut image = vec![0xAA; 100];
        image.extend_from_slice(&validator::test_jpeg());
        image.extend_from_slice(&[0xBB; 100]);

        fs::write(&image_path, image).unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let mut reader = DiskImageReader::open(&image_path).unwrap();

        let result = scan_format(&mut reader, &output_dir, FileFormat::Jpeg).unwrap();

        assert_eq!(result.len(), 1);

        let recovered = fs::read(&result[0].path).unwrap();
        assert!(validator::validate_jpeg(&recovered).is_ok());

        drop(reader);
        let _ = fs::remove_file(image_path);
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn skips_invalid_jpeg() {
        let image_path = temporary_path("invalid-jpeg-image");
        let output_dir = temporary_path("invalid-jpeg-output");

        let mut image = vec![0xAA; 100];
        image.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0x11, 0x22, 0x33, 0xFF, 0xD9]);

        fs::write(&image_path, image).unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let mut reader = DiskImageReader::open(&image_path).unwrap();

        let result = scan_format(&mut reader, &output_dir, FileFormat::Jpeg).unwrap();

        assert!(result.is_empty());
        assert!(fs::read_dir(&output_dir).unwrap().next().is_none());

        drop(reader);
        let _ = fs::remove_file(image_path);
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn carves_synthetic_pdf() {
        let image_path = temporary_path("pdf-image");
        let output_dir = temporary_path("pdf-output");

        let mut image = vec![0xAA; 100];
        image.extend_from_slice(&validator::test_pdf());
        image.extend_from_slice(&[0xBB; 100]);

        fs::write(&image_path, image).unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let mut reader = DiskImageReader::open(&image_path).unwrap();

        let result = scan_format(&mut reader, &output_dir, FileFormat::Pdf).unwrap();

        assert_eq!(result.len(), 1);

        let recovered = fs::read(&result[0].path).unwrap();

        assert!(recovered.starts_with(FileFormat::Pdf.header()));
        assert!(recovered.ends_with(FileFormat::Pdf.footer()));

        drop(reader);
        let _ = fs::remove_file(image_path);
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn skips_invalid_png() {
        let image_path = temporary_path("invalid-png-image");
        let output_dir = temporary_path("invalid-png-output");

        let mut image = vec![0xAA; 100];
        image.extend_from_slice(FileFormat::Png.header());
        image.extend_from_slice(b"INVALID PNG DATA");
        image.extend_from_slice(FileFormat::Png.footer());

        fs::write(&image_path, image).unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let mut reader = DiskImageReader::open(&image_path).unwrap();

        let result = scan_format(&mut reader, &output_dir, FileFormat::Png).unwrap();

        assert!(result.is_empty());

        let output_files: Vec<_> = fs::read_dir(&output_dir).unwrap().collect();
        assert!(output_files.is_empty());

        drop(reader);
        let _ = fs::remove_file(image_path);
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn skips_invalid_pdf() {
        let image_path = temporary_path("invalid-pdf-image");
        let output_dir = temporary_path("invalid-pdf-output");

        let mut image = vec![0xAA; 100];
        image.extend_from_slice(b"%PDF-1.7\nINVALID PDF DATA\n%%EOF");
        image.extend_from_slice(&[0xBB; 100]);

        fs::write(&image_path, image).unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let mut reader = DiskImageReader::open(&image_path).unwrap();

        let result = scan_format(&mut reader, &output_dir, FileFormat::Pdf).unwrap();

        assert!(result.is_empty());

        let output_files: Vec<_> = fs::read_dir(&output_dir).unwrap().collect();
        assert!(output_files.is_empty());

        drop(reader);
        let _ = fs::remove_file(image_path);
        let _ = fs::remove_dir_all(output_dir);
    }
}
