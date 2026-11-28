use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMethod {
    QuickRecovery,
    DeepRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRecord {
    pub filename: String,
    pub format: String,
    pub size_bytes: u64,
    pub output_path: String,
    pub source_image: String,
    pub source_offset: Option<u64>,
    pub source_cluster: Option<u32>,
    pub method: RecoveryMethod,
    pub recovered_at: u64,
}

impl RecoveryRecord {
    pub fn new(
        output_path: impl AsRef<Path>,
        source_image: impl AsRef<Path>,
        format: impl Into<String>,
        size_bytes: u64,
        source_offset: Option<u64>,
        source_cluster: Option<u32>,
        method: RecoveryMethod,
    ) -> Self {
        let output_path = output_path.as_ref();

        Self {
            filename: output_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            format: format.into(),
            size_bytes,
            output_path: output_path.to_string_lossy().into_owned(),
            source_image: source_image.as_ref().to_string_lossy().into_owned(),
            source_offset,
            source_cluster,
            method,
            recovered_at: current_timestamp(),
        }
    }
}

pub fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_recovery_record() {
        let record = RecoveryRecord::new(
            "/tmp/recovered/photo.jpg",
            "/tmp/disk.img",
            "JPEG",
            1024,
            Some(2048),
            Some(5),
            RecoveryMethod::QuickRecovery,
        );

        assert_eq!(record.filename, "photo.jpg");
        assert_eq!(record.format, "JPEG");
        assert_eq!(record.size_bytes, 1024);
        assert_eq!(record.source_offset, Some(2048));
        assert_eq!(record.source_cluster, Some(5));
        assert_eq!(record.method, RecoveryMethod::QuickRecovery);
        assert!(record.recovered_at > 0);
    }

    #[test]
    fn serializes_recovery_method_as_snake_case() {
        let method = serde_json::to_string(&RecoveryMethod::DeepRecovery).unwrap();

        assert_eq!(method, "\"deep_recovery\"");
    }

    #[test]
    fn supports_unknown_source_location() {
        let record = RecoveryRecord::new(
            "/tmp/recovered/file.bin",
            "/tmp/disk.img",
            "BIN",
            100,
            None,
            None,
            RecoveryMethod::DeepRecovery,
        );

        assert_eq!(record.source_offset, None);
        assert_eq!(record.source_cluster, None);
    }
}
