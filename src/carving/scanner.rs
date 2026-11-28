use anyhow::{Context, Result, bail};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::disk::reader::DiskImageReader;

use super::signatures::FileFormat;
use super::validator;

const CHUNK_SIZE: usize = 1024 * 1024;
const MAX_FILE_SIZE: usize = 64 * 1024 * 1024;
const MAX_ACTIVE_CANDIDATES: usize = 16;
const MAX_TOTAL_BUFFERED_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug)]
pub struct CarvedFile {
    pub format: FileFormat,
    pub path: PathBuf,
    pub size: u64,
    pub source_offset: u64,
}

struct Candidate {
    format: FileFormat,
    offset: u64,
    data: Vec<u8>,
}

// Check whether a candidate has reached the maximum allowed size.
fn candidate_is_too_large(candidate_size: usize) -> bool {
    candidate_size >= MAX_FILE_SIZE
}

// Check whether a new candidate can be accepted.
fn candidate_limit_reason(
    active_candidates: usize,
    buffered_bytes: usize,
    header_len: usize,
) -> Option<&'static str> {
    if active_candidates >= MAX_ACTIVE_CANDIDATES {
        Some("too many active candidates")
    } else if buffered_bytes.saturating_add(header_len) > MAX_TOTAL_BUFFERED_BYTES {
        Some("memory limit reached")
    } else {
        None
    }
}

// Save recovered data without overwriting existing files.
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

// Validate a completed candidate before saving it.
fn validate_candidate(
    candidate: Candidate,
    output_dir: &Path,
    sequence: &mut u64,
) -> Result<Option<CarvedFile>> {
    let validation = match candidate.format {
        FileFormat::Jpeg => validator::validate_jpeg(&candidate.data),
        FileFormat::Png => validator::validate_png(&candidate.data),
        FileFormat::Pdf => validator::validate_pdf(&candidate.data),
    };

    if let Err(reason) = validation {
        println!(
            "Skipping invalid {} candidate at offset {}: {}",
            candidate.format.name(),
            candidate.offset,
            reason
        );

        return Ok(None);
    }

    let size = candidate.data.len() as u64;

    let path = save_carved_file(output_dir, candidate.format, sequence, &candidate.data)?;

    Ok(Some(CarvedFile {
        format: candidate.format,
        path,
        size,
        source_offset: candidate.offset,
    }))
}

// Scan a disk image in a single sequential pass.
pub fn scan_image(
    reader: &mut DiskImageReader,
    output_dir: impl AsRef<Path>,
) -> Result<Vec<CarvedFile>> {
    let output_dir = output_dir.as_ref();

    if reader.size() == 0 {
        bail!("Disk image is empty.");
    }

    fs::create_dir_all(output_dir).context("Failed to create recovery output directory")?;

    println!("Starting single-pass file carving...");
    println!("Supported formats: JPEG, PNG, PDF");

    let image_size = reader.size();

    let max_header_len = FileFormat::all()
        .iter()
        .map(|format| format.header().len())
        .max()
        .unwrap_or(1);

    // Reusable image read buffer.
    let mut buffer = vec![0u8; CHUNK_SIZE];

    // Rolling window used to detect file headers.
    let mut window = Vec::with_capacity(max_header_len);

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut total_buffered_bytes = 0usize;

    let mut recovered = Vec::new();
    let mut sequence = 1u64;

    let mut offset = 0u64;
    let mut dropped_candidates = 0u64;

    while offset < image_size {
        let remaining = image_size - offset;
        let length = usize::try_from(remaining.min(CHUNK_SIZE as u64))?;

        reader.read_into(offset, &mut buffer[..length])?;

        for (index, byte) in buffer[..length].iter().enumerate() {
            let absolute_offset = offset + index as u64;

            // Remove candidates that have reached the maximum file size.
            let mut candidate_index = 0;

            while candidate_index < candidates.len() {
                if candidate_is_too_large(candidates[candidate_index].data.len()) {
                    let removed = candidates.remove(candidate_index);

                    total_buffered_bytes -= removed.data.len();
                    dropped_candidates += 1;

                    println!(
                        "Skipping {} candidate at offset {}: maximum file size exceeded",
                        removed.format.name(),
                        removed.offset
                    );
                } else {
                    candidate_index += 1;
                }
            }

            // Enforce the aggregate buffered-data limit.
            while !candidates.is_empty()
                && total_buffered_bytes.saturating_add(candidates.len()) > MAX_TOTAL_BUFFERED_BYTES
            {
                let removed = candidates.remove(0);

                total_buffered_bytes -= removed.data.len();
                dropped_candidates += 1;

                println!(
                    "Dropping oldest {} candidate at offset {}: memory limit reached",
                    removed.format.name(),
                    removed.offset
                );
            }

            // Append the current byte to every active candidate.
            for candidate in &mut candidates {
                candidate.data.push(*byte);
            }

            total_buffered_bytes += candidates.len();

            // Check whether any candidate has reached its footer.
            let mut candidate_index = 0;

            while candidate_index < candidates.len() {
                let format = candidates[candidate_index].format;
                let footer = format.footer();

                let completed = candidates[candidate_index].data.ends_with(footer);

                if completed {
                    let candidate = candidates.remove(candidate_index);

                    total_buffered_bytes -= candidate.data.len();

                    if let Some(file) = validate_candidate(candidate, output_dir, &mut sequence)? {
                        println!(
                            "Recovered {}: {} ({} bytes)",
                            file.format.name(),
                            file.path.display(),
                            file.size
                        );

                        recovered.push(file);
                    }
                } else {
                    candidate_index += 1;
                }
            }

            // Maintain the rolling window for header detection.
            window.push(*byte);

            if window.len() > max_header_len {
                window.remove(0);
            }

            // Detect all supported headers without rescanning the image.
            for format in FileFormat::all() {
                let header = format.header();

                if window.len() < header.len() {
                    continue;
                }

                if &window[window.len() - header.len()..] != header {
                    continue;
                }

                let candidate_offset = absolute_offset + 1 - header.len() as u64;

                // Enforce candidate and buffered-data limits.
                if let Some(reason) =
                    candidate_limit_reason(candidates.len(), total_buffered_bytes, header.len())
                {
                    dropped_candidates += 1;

                    println!(
                        "Skipping {} candidate at offset {}: {}",
                        format.name(),
                        candidate_offset,
                        reason
                    );

                    continue;
                }

                candidates.push(Candidate {
                    format: *format,
                    offset: candidate_offset,
                    data: header.to_vec(),
                });

                total_buffered_bytes += header.len();

                println!(
                    "Detected {} header at offset {}",
                    format.name(),
                    candidate_offset
                );
            }
        }

        offset += length as u64;
    }

    // Report candidates that never reached a footer.
    if !candidates.is_empty() {
        for candidate in &candidates {
            println!(
                "Incomplete {} candidate at offset {}: no valid footer found",
                candidate.format.name(),
                candidate.offset
            );
        }
    }

    if dropped_candidates > 0 {
        println!(
            "Skipped or dropped {} candidate(s) due to configured limits.",
            dropped_candidates
        );
    }

    println!("\nFile carving completed.");

    for format in FileFormat::all() {
        let count = recovered
            .iter()
            .filter(|file| file.format == *format)
            .count();

        println!("Recovered {} {} file(s).", count, format.name());
    }

    println!("Total recovered files: {}", recovered.len());

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

    fn run_scan(image: &[u8], label: &str) -> (Vec<CarvedFile>, PathBuf, PathBuf) {
        let image_path = temporary_path(&format!("{label}-image"));
        let output_dir = temporary_path(&format!("{label}-output"));

        fs::write(&image_path, image).unwrap();

        let mut reader = DiskImageReader::open(&image_path).unwrap();

        let recovered = scan_image(&mut reader, &output_dir).unwrap();

        drop(reader);

        (recovered, image_path, output_dir)
    }

    fn cleanup(image_path: PathBuf, output_dir: PathBuf) {
        let _ = fs::remove_file(image_path);
        let _ = fs::remove_dir_all(output_dir);
    }

    #[test]
    fn finds_pattern_across_chunk_boundary() {
        let mut image = vec![0xAA; CHUNK_SIZE - 2];

        image.extend_from_slice(&validator::test_jpeg());

        let (recovered, image_path, output_dir) = run_scan(&image, "boundary");

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].format, FileFormat::Jpeg);

        cleanup(image_path, output_dir);
    }

    #[test]
    fn carves_synthetic_png() {
        let mut image = vec![0xAA; 100];

        image.extend_from_slice(&validator::test_png());
        image.extend_from_slice(&[0xBB; 100]);

        let (recovered, image_path, output_dir) = run_scan(&image, "png");

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].format, FileFormat::Png);

        let data = fs::read(&recovered[0].path).unwrap();

        assert!(validator::validate_png(&data).is_ok());

        cleanup(image_path, output_dir);
    }

    #[test]
    fn carves_synthetic_jpeg() {
        let mut image = vec![0xAA; 100];

        image.extend_from_slice(&validator::test_jpeg());
        image.extend_from_slice(&[0xBB; 100]);

        let (recovered, image_path, output_dir) = run_scan(&image, "jpeg");

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].format, FileFormat::Jpeg);

        let data = fs::read(&recovered[0].path).unwrap();

        assert!(validator::validate_jpeg(&data).is_ok());

        cleanup(image_path, output_dir);
    }

    #[test]
    fn carves_synthetic_pdf() {
        let mut image = vec![0xAA; 100];

        image.extend_from_slice(&validator::test_pdf());
        image.extend_from_slice(&[0xBB; 100]);

        let (recovered, image_path, output_dir) = run_scan(&image, "pdf");

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].format, FileFormat::Pdf);

        let data = fs::read(&recovered[0].path).unwrap();

        assert!(validator::validate_pdf(&data).is_ok());

        cleanup(image_path, output_dir);
    }

    #[test]
    fn scans_multiple_formats_in_one_image() {
        let mut image = vec![0xAA; 100];

        image.extend_from_slice(&validator::test_jpeg());
        image.extend_from_slice(&[0xBB; 100]);

        image.extend_from_slice(&validator::test_png());
        image.extend_from_slice(&[0xCC; 100]);

        image.extend_from_slice(&validator::test_pdf());

        let (recovered, image_path, output_dir) = run_scan(&image, "multiple");

        assert_eq!(recovered.len(), 3);

        assert!(recovered.iter().any(|file| file.format == FileFormat::Jpeg));
        assert!(recovered.iter().any(|file| file.format == FileFormat::Png));
        assert!(recovered.iter().any(|file| file.format == FileFormat::Pdf));

        cleanup(image_path, output_dir);
    }

    #[test]
    fn skips_invalid_jpeg() {
        let mut image = vec![0xAA; 100];

        image.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0x11, 0x22, 0x33, 0xFF, 0xD9]);

        let (recovered, image_path, output_dir) = run_scan(&image, "invalid-jpeg");

        assert!(recovered.is_empty());

        assert!(!output_dir.exists() || fs::read_dir(&output_dir).unwrap().next().is_none());

        cleanup(image_path, output_dir);
    }

    #[test]
    fn skips_invalid_png() {
        let mut image = vec![0xAA; 100];

        image.extend_from_slice(FileFormat::Png.header());
        image.extend_from_slice(b"INVALID PNG DATA");
        image.extend_from_slice(FileFormat::Png.footer());

        let (recovered, image_path, output_dir) = run_scan(&image, "invalid-png");

        assert!(recovered.is_empty());

        assert!(!output_dir.exists() || fs::read_dir(&output_dir).unwrap().next().is_none());

        cleanup(image_path, output_dir);
    }

    #[test]
    fn skips_invalid_pdf() {
        let mut image = vec![0xAA; 100];

        image.extend_from_slice(b"%PDF-1.7\nINVALID PDF DATA\n%%EOF");

        let (recovered, image_path, output_dir) = run_scan(&image, "invalid-pdf");

        assert!(recovered.is_empty());

        assert!(!output_dir.exists() || fs::read_dir(&output_dir).unwrap().next().is_none());

        cleanup(image_path, output_dir);
    }

    #[test]
    fn does_not_overwrite_existing_recovered_file() {
        let image_path = temporary_path("no-overwrite-image");
        let output_dir = temporary_path("no-overwrite-output");

        fs::write(&image_path, validator::test_jpeg()).unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let existing_path = output_dir.join("recovered_000001.jpg");
        let original_content = b"DO NOT OVERWRITE THIS FILE";

        fs::write(&existing_path, original_content).unwrap();

        let mut reader = DiskImageReader::open(&image_path).unwrap();

        let recovered = scan_image(&mut reader, &output_dir).unwrap();

        assert_eq!(recovered.len(), 1);

        assert_eq!(fs::read(&existing_path).unwrap(), original_content);

        assert_eq!(
            recovered[0].path.file_name().unwrap(),
            "recovered_000002.jpg"
        );

        drop(reader);
        cleanup(image_path, output_dir);
    }

    #[test]
    fn finds_valid_file_after_false_header() {
        let mut image = vec![0xAA; 100];

        // False JPEG candidate.
        image.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0x11, 0x22, 0x33]);

        // A valid JPEG appears before the false candidate's footer.
        image.extend_from_slice(&validator::test_jpeg());

        let (recovered, image_path, output_dir) = run_scan(&image, "false-header");

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].format, FileFormat::Jpeg);

        let recovered_data = fs::read(&recovered[0].path).unwrap();

        assert_eq!(recovered_data, validator::test_jpeg());

        cleanup(image_path, output_dir);
    }

    #[test]
    fn ignores_candidate_without_footer() {
        let mut image = vec![0xAA; 100];

        image.extend_from_slice(FileFormat::Jpeg.header());
        image.extend_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55]);

        let (recovered, image_path, output_dir) = run_scan(&image, "missing-footer");

        assert!(recovered.is_empty());

        assert!(!output_dir.exists() || fs::read_dir(&output_dir).unwrap().next().is_none());

        cleanup(image_path, output_dir);
    }

    #[test]
    fn enforces_maximum_candidate_size() {
        assert!(!candidate_is_too_large(MAX_FILE_SIZE - 1));
        assert!(candidate_is_too_large(MAX_FILE_SIZE));
        assert!(candidate_is_too_large(MAX_FILE_SIZE + 1));
    }

    #[test]
    fn enforces_active_candidate_limit() {
        assert_eq!(
            candidate_limit_reason(MAX_ACTIVE_CANDIDATES - 1, 0, 8,),
            None
        );

        assert_eq!(
            candidate_limit_reason(MAX_ACTIVE_CANDIDATES, 0, 8,),
            Some("too many active candidates")
        );
    }

    #[test]
    fn enforces_total_buffered_data_limit() {
        assert_eq!(
            candidate_limit_reason(0, MAX_TOTAL_BUFFERED_BYTES - 8, 8,),
            None
        );

        assert_eq!(
            candidate_limit_reason(0, MAX_TOTAL_BUFFERED_BYTES - 7, 8,),
            Some("memory limit reached")
        );
    }
}
