use anyhow::{Context, Result, bail};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::filesystem::directory::entries::DirectoryEntry;
use crate::filesystem::directory::fat32::Fat32ClusterReader;

/// Recover one FAT32 file into a separate output directory.
pub fn recover_file(
    reader: &mut Fat32ClusterReader<'_>,
    entry: &DirectoryEntry,
    output_dir: impl AsRef<Path>,
) -> Result<PathBuf> {
    if entry.is_directory() {
        bail!("Cannot recover a directory as a regular file");
    }

    let filename = entry.name.trim();

    if filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains('/')
        || filename.contains('\\')
        || filename.chars().any(char::is_control)
    {
        bail!("Invalid filename in FAT32 directory entry");
    }

    let output_dir = output_dir.as_ref();

    fs::create_dir_all(output_dir).context("Failed to create recovery directory")?;

    let output_path = output_dir.join(filename);

    let cluster_chain = if entry.size == 0 {
        Vec::new()
    } else {
        if entry.first_cluster < 2 {
            bail!("Invalid first cluster for non-empty file");
        }

        reader.read_cluster_chain(entry.first_cluster)?
    };

    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .context("Cannot create output file; it may already exist")?;

    let result = (|| -> Result<()> {
        let mut remaining = u64::from(entry.size);

        for cluster in cluster_chain {
            if remaining == 0 {
                break;
            }

            let data = reader.read_cluster(cluster)?;
            let bytes_to_write = remaining.min(data.len() as u64) as usize;

            output
                .write_all(&data[..bytes_to_write])
                .context("Failed to write recovered file data")?;

            remaining -= bytes_to_write as u64;
        }

        if remaining != 0 {
            bail!("Cluster chain is too short for the original file size");
        }

        output.flush().context("Failed to flush recovered file")?;

        Ok(())
    })();

    if let Err(error) = result {
        drop(output);
        let _ = fs::remove_file(&output_path);
        return Err(error);
    }

    Ok(output_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::reader::DiskImageReader;
    use crate::filesystem::directory::fat32::Fat32ClusterReader;

    #[test]
    #[ignore = "Requires local FAT32 test image"]
    fn recovers_real_fat32_files() {
        let image_path = "test-images/test-fat32-directory.img";
        let output_dir = std::env::temp_dir().join("recuvax-recovery-test");

        let _ = fs::remove_dir_all(&output_dir);

        let mut disk = DiskImageReader::open(image_path).unwrap();

        let mut fat = Fat32ClusterReader::new(&mut disk, 2048, 102_400_000).unwrap();

        let entries = fat.read_directory(2).unwrap();

        for name in ["hello.txt", "notes with spaces.txt"] {
            let entry = entries.iter().find(|entry| entry.name == name).unwrap();

            let recovered = recover_file(&mut fat, entry, &output_dir).unwrap();

            println!("Recovered: {}", recovered.display());
        }

        let documents = entries
            .iter()
            .find(|entry| entry.name == "Documents")
            .unwrap();

        let document_entries = fat.read_directory(documents.first_cluster).unwrap();

        let report = document_entries
            .iter()
            .find(|entry| entry.name == "report.txt")
            .unwrap();

        let recovered = recover_file(&mut fat, report, &output_dir).unwrap();

        println!("Recovered: {}", recovered.display());

        assert_eq!(
            fs::read(output_dir.join("hello.txt")).unwrap(),
            b"Hello from RecuvaX\n"
        );

        assert_eq!(
            fs::read(output_dir.join("notes with spaces.txt")).unwrap(),
            b"Testing FAT32 directory parser\n"
        );

        assert_eq!(fs::read(recovered).unwrap(), b"This is a test document\n");

        let _ = fs::remove_dir_all(&output_dir);
    }
}
