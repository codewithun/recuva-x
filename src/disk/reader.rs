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

    pub fn read_into(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        let length = u64::try_from(buffer.len()).context("Requested read length is too large")?;

        let end = offset.checked_add(length).context("Read offset overflow")?;

        if end > self.size {
            bail!("Read operation exceeds image boundaries.");
        }

        self.file
            .seek(SeekFrom::Start(offset))
            .context("Failed to seek to requested offset")?;

        self.file
            .read_exact(buffer)
            .context("Failed to read requested bytes")?;

        Ok(())
    }

    pub fn read_bytes(&mut self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let mut buffer = vec![0u8; length];

        self.read_into(offset, &mut buffer)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_test_image() -> (PathBuf, File) {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);

        let path =
            std::env::temp_dir().join(format!("recuvax-reader-test-{}-{id}", std::process::id()));

        let mut file = File::create(&path).unwrap();
        file.write_all(b"0123456789ABCDEF").unwrap();

        (path, file)
    }

    #[test]
    fn reads_into_reusable_buffer() {
        let (path, _file) = create_test_image();

        let mut reader = DiskImageReader::open(&path).unwrap();
        let mut buffer = [0u8; 4];

        reader.read_into(4, &mut buffer).unwrap();

        assert_eq!(&buffer, b"4567");

        reader.read_into(8, &mut buffer).unwrap();

        assert_eq!(&buffer, b"89AB");

        drop(reader);
        drop(_file);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_read_into_outside_image() {
        let (path, _file) = create_test_image();

        let mut reader = DiskImageReader::open(&path).unwrap();
        let mut buffer = [0u8; 8];

        assert!(reader.read_into(12, &mut buffer).is_err());

        drop(reader);
        drop(_file);

        let _ = std::fs::remove_file(path);
    }
}
