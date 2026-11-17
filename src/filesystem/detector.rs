use anyhow::{Context, Result};

use crate::disk::reader::DiskImageReader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystem {
    Fat32,
    ExFat,
    Ntfs,
    Ext4,
    Apfs,
    Unknown,
}

impl std::fmt::Display for FileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Fat32 => "FAT32",
            Self::ExFat => "exFAT",
            Self::Ntfs => "NTFS",
            Self::Ext4 => "ext4",
            Self::Apfs => "APFS",
            Self::Unknown => "Unknown",
        };

        write!(f, "{name}")
    }
}

/// Detects a filesystem using the partition's starting LBA and size.
///
/// The source image is only opened for reading. No disk modifications occur.
pub fn detect(
    reader: &mut DiskImageReader,
    start_lba: u64,
    partition_size: u64,
) -> Result<FileSystem> {
    let partition_offset = start_lba
        .checked_mul(512)
        .context("Partition offset overflow")?;

    let partition_end = partition_offset
        .checked_add(partition_size)
        .context("Partition boundary overflow")?;

    if partition_end > reader.size() {
        anyhow::bail!("Partition exceeds disk image boundaries.");
    }

    if partition_size < 512 {
        return Ok(FileSystem::Unknown);
    }

    // Read the first sector relative to the partition start.
    let boot_sector = reader.read_bytes(partition_offset, 512)?;

    // NTFS OEM ID: bytes 3..11.
    if &boot_sector[3..11] == b"NTFS    " {
        return Ok(FileSystem::Ntfs);
    }

    // exFAT filesystem name: bytes 3..11.
    if &boot_sector[3..11] == b"EXFAT   " {
        return Ok(FileSystem::ExFat);
    }

    // FAT32 uses a BPB structure, not just a filesystem label.
    if is_fat32(&boot_sector) {
        return Ok(FileSystem::Fat32);
    }

    // APFS container superblock magic: NXSB at offset 32.
    if partition_size >= 40 {
        let apfs_magic = reader.read_bytes(partition_offset + 32, 4)?;

        if &apfs_magic == b"NXSB" {
            return Ok(FileSystem::Apfs);
        }
    }

    // ext4 superblock starts 1024 bytes after partition start.
    // Its magic value is at offset 56 within the superblock.
    let ext4_magic_offset = 1024 + 56;

    if partition_size >= ext4_magic_offset + 2 {
        let ext4_magic = reader.read_bytes(partition_offset + ext4_magic_offset, 2)?;

        if ext4_magic == [0x53, 0xEF] {
            return Ok(FileSystem::Ext4);
        }
    }

    Ok(FileSystem::Unknown)
}

fn is_fat32(boot_sector: &[u8]) -> bool {
    if boot_sector.len() < 512 {
        return false;
    }

    let bytes_per_sector = u16::from_le_bytes([boot_sector[11], boot_sector[12]]);

    let sectors_per_cluster = boot_sector[13];
    let reserved_sectors = u16::from_le_bytes([boot_sector[14], boot_sector[15]]);

    let number_of_fats = boot_sector[16];

    let root_entry_count = u16::from_le_bytes([boot_sector[17], boot_sector[18]]);

    let fat_size_16 = u16::from_le_bytes([boot_sector[22], boot_sector[23]]);

    let fat_size_32 = u32::from_le_bytes([
        boot_sector[36],
        boot_sector[37],
        boot_sector[38],
        boot_sector[39],
    ]);

    let has_boot_signature = boot_sector[510] == 0x55 && boot_sector[511] == 0xAA;

    has_boot_signature
        && bytes_per_sector == 512
        && sectors_per_cluster.is_power_of_two()
        && sectors_per_cluster != 0
        && reserved_sectors != 0
        && number_of_fats != 0
        && root_entry_count == 0
        && fat_size_16 == 0
        && fat_size_32 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn run_detection(image: Vec<u8>, start_lba: u64, partition_size: u64) -> Result<FileSystem> {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);

        let path =
            std::env::temp_dir().join(format!("recuvax-test-{}-{id}.img", std::process::id()));

        fs::write(&path, image)?;

        let result = {
            let mut reader = DiskImageReader::open(&path)?;
            detect(&mut reader, start_lba, partition_size)
        };

        let _ = fs::remove_file(&path);

        result
    }

    fn create_partition_image() -> Vec<u8> {
        // Partition begins at LBA 2.
        vec![0u8; 4096]
    }

    #[test]
    fn detects_ntfs() {
        let mut image = create_partition_image();
        let offset = 2 * 512;

        image[offset + 3..offset + 11].copy_from_slice(b"NTFS    ");

        let result = run_detection(image, 2, 2048).unwrap();

        assert_eq!(result, FileSystem::Ntfs);
    }

    #[test]
    fn detects_exfat() {
        let mut image = create_partition_image();
        let offset = 2 * 512;

        image[offset + 3..offset + 11].copy_from_slice(b"EXFAT   ");

        let result = run_detection(image, 2, 2048).unwrap();

        assert_eq!(result, FileSystem::ExFat);
    }

    #[test]
    fn detects_fat32() {
        let mut image = create_partition_image();
        let offset = 2 * 512;

        image[offset + 11..offset + 13].copy_from_slice(&512u16.to_le_bytes());
        image[offset + 13] = 8;
        image[offset + 14..offset + 16].copy_from_slice(&32u16.to_le_bytes());
        image[offset + 16] = 2;
        image[offset + 17..offset + 19].copy_from_slice(&0u16.to_le_bytes());
        image[offset + 22..offset + 24].copy_from_slice(&0u16.to_le_bytes());
        image[offset + 36..offset + 40].copy_from_slice(&1000u32.to_le_bytes());
        image[offset + 510] = 0x55;
        image[offset + 511] = 0xAA;

        let result = run_detection(image, 2, 2048).unwrap();

        assert_eq!(result, FileSystem::Fat32);
    }

    #[test]
    fn detects_ext4() {
        let mut image = create_partition_image();
        let offset = 2 * 512 + 1024 + 56;

        image[offset..offset + 2].copy_from_slice(&[0x53, 0xEF]);

        let result = run_detection(image, 2, 2048).unwrap();

        assert_eq!(result, FileSystem::Ext4);
    }

    #[test]
    fn detects_apfs() {
        let mut image = create_partition_image();
        let offset = 2 * 512 + 32;

        image[offset..offset + 4].copy_from_slice(b"NXSB");

        let result = run_detection(image, 2, 2048).unwrap();

        assert_eq!(result, FileSystem::Apfs);
    }

    #[test]
    fn returns_unknown_for_unrecognized_filesystem() {
        let image = create_partition_image();

        let result = run_detection(image, 2, 2048).unwrap();

        assert_eq!(result, FileSystem::Unknown);
    }

    #[test]
    fn rejects_partition_outside_image() {
        let image = create_partition_image();

        let result = run_detection(image, 2, 4096);

        assert!(result.is_err());
    }
}
