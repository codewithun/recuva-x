use anyhow::{Context, Result, bail};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const SECTOR_SIZE: u64 = 512;

pub struct DiskImageReader {
    file: File,
    path: PathBuf,
    size: u64,
}

impl DiskImageReader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        let metadata = std::fs::metadata(path)
            .with_context(|| format!("Cannot access image: {}", path.display()))?;

        if !metadata.is_file() {
            bail!("The selected path is not a regular file.");
        }

        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .with_context(|| "Failed to open disk image in read-only mode")?;

        Ok(Self {
            file,
            path: path.to_path_buf(),
            size: metadata.len(),
        })
    }

    pub fn read_bytes(&mut self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let length_u64 = u64::try_from(length).context("Requested read length is too large")?;

        let end = offset
            .checked_add(length_u64)
            .context("Read offset overflow")?;

        if end > self.size {
            bail!("Read operation exceeds image boundaries.");
        }

        self.file
            .seek(SeekFrom::Start(offset))
            .context("Failed to seek to requested offset")?;

        let mut buffer = vec![0u8; length];

        self.file
            .read_exact(&mut buffer)
            .context("Failed to read requested bytes")?;

        Ok(buffer)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn sector_count(&self) -> u64 {
        self.size / SECTOR_SIZE
    }

    pub fn read_sector(&mut self, sector_number: u64) -> Result<[u8; 512]> {
        if sector_number >= self.sector_count() {
            bail!("Sector number is outside the image boundaries.");
        }

        let offset = sector_number
            .checked_mul(SECTOR_SIZE)
            .context("Sector offset overflow")?;

        self.file
            .seek(SeekFrom::Start(offset))
            .context("Failed to seek to requested sector")?;

        let mut buffer = [0u8; 512];

        self.file
            .read_exact(&mut buffer)
            .context("Failed to read complete sector")?;

        Ok(buffer)
    }
}
