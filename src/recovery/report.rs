use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::metadata::{RecoveryRecord, current_timestamp};

#[derive(Debug, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub schema_version: u32,
    pub generated_at: u64,
    pub total_files: usize,
    pub total_size_bytes: u64,
    pub records: Vec<RecoveryRecord>,
}

impl RecoveryReport {
    pub fn new(records: Vec<RecoveryRecord>) -> Self {
        let total_size_bytes = records
            .iter()
            .map(|record| record.size_bytes)
            .fold(0u64, u64::saturating_add);

        Self {
            schema_version: 1,
            generated_at: current_timestamp(),
            total_files: records.len(),
            total_size_bytes,
            records,
        }
    }
}

/// Export recovery records to a JSON report without overwriting existing reports.
pub fn write_report(output_dir: impl AsRef<Path>, records: &[RecoveryRecord]) -> Result<PathBuf> {
    let output_dir = output_dir.as_ref();

    fs::create_dir_all(output_dir).context("Failed to create recovery report directory")?;

    let report = RecoveryReport::new(records.to_vec());

    let mut sequence = 0u64;

    loop {
        let filename = if sequence == 0 {
            "recovery-report.json".to_string()
        } else {
            format!("recovery-report-{sequence:06}.json")
        };

        let path = output_dir.join(filename);

        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                sequence = sequence.saturating_add(1);

                if sequence == u64::MAX {
                    anyhow::bail!("Unable to allocate a unique recovery report filename");
                }

                continue;
            }
            Err(error) => {
                return Err(error).context("Failed to create recovery report");
            }
        };

        let result = (|| -> Result<()> {
            serde_json::to_writer_pretty(&mut file, &report)
                .context("Failed to serialize recovery report")?;

            file.write_all(b"\n")
                .context("Failed to finish recovery report")?;

            file.flush().context("Failed to flush recovery report")?;

            Ok(())
        })();

        if let Err(error) = result {
            drop(file);
            let _ = fs::remove_file(&path);

            return Err(error);
        }

        return Ok(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::metadata::RecoveryMethod;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temporary_directory() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);

        std::env::temp_dir().join(format!("recuvax-report-test-{}-{id}", std::process::id()))
    }

    fn sample_record() -> RecoveryRecord {
        RecoveryRecord::new(
            "/tmp/recovered/photo.jpg",
            "/tmp/disk.img",
            "JPEG",
            2048,
            Some(4096),
            None,
            RecoveryMethod::DeepRecovery,
        )
    }

    #[test]
    fn creates_valid_json_report() {
        let directory = temporary_directory();
        let records = vec![sample_record()];

        let path = write_report(&directory, &records).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let report: RecoveryReport = serde_json::from_str(&content).unwrap();

        assert_eq!(report.schema_version, 1);
        assert_eq!(report.total_files, 1);
        assert_eq!(report.total_size_bytes, 2048);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].filename, "photo.jpg");

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn does_not_overwrite_existing_report() {
        let directory = temporary_directory();

        fs::create_dir_all(&directory).unwrap();

        let original_path = directory.join("recovery-report.json");
        fs::write(&original_path, b"ORIGINAL REPORT").unwrap();

        let path = write_report(&directory, &[sample_record()]).unwrap();

        assert_eq!(path.file_name().unwrap(), "recovery-report-000001.json");

        assert_eq!(fs::read(&original_path).unwrap(), b"ORIGINAL REPORT");

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn creates_empty_report() {
        let directory = temporary_directory();

        let path = write_report(&directory, &[]).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let report: RecoveryReport = serde_json::from_str(&content).unwrap();

        assert_eq!(report.total_files, 0);
        assert_eq!(report.total_size_bytes, 0);
        assert!(report.records.is_empty());

        let _ = fs::remove_dir_all(directory);
    }
}
