use anyhow::{Context, Result, bail};

use crate::disk::reader::DiskImageReader;

const BOOT_SECTOR_SIZE: usize = 512;
const SECTOR_SIZE: u64 = 512;
const FAT_ENTRY_SIZE: u64 = 4;
const MAX_CLUSTER_SIZE: u64 = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fat32Metadata {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub cluster_size_bytes: u64,
    pub reserved_sectors: u16,
    pub number_of_fats: u8,
    pub total_sectors: u32,
    pub fat_size_sectors: u32,
    pub root_cluster: u32,
    pub data_cluster_count: u64,
    pub fat_capacity_entries: u64,
    pub volume_size_bytes: u64,
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16> {
    let bytes: [u8; 2] = data
        .get(offset..offset + 2)
        .context("Invalid FAT32 16-bit field")?
        .try_into()?;

    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes: [u8; 4] = data
        .get(offset..offset + 4)
        .context("Invalid FAT32 32-bit field")?
        .try_into()?;

    Ok(u32::from_le_bytes(bytes))
}

/// Reads FAT32 filesystem metadata without modifying the source image.
pub fn read(
    reader: &mut DiskImageReader,
    start_lba: u64,
    partition_size: u64,
) -> Result<Fat32Metadata> {
    let partition_offset = start_lba
        .checked_mul(SECTOR_SIZE)
        .context("FAT32 partition offset overflow")?;

    let partition_end = partition_offset
        .checked_add(partition_size)
        .context("FAT32 partition boundary overflow")?;

    if partition_end > reader.size() {
        bail!("FAT32 partition exceeds disk image boundaries.");
    }

    if partition_size < BOOT_SECTOR_SIZE as u64 {
        bail!("FAT32 partition is too small for a boot sector.");
    }

    let boot_sector = reader.read_bytes(partition_offset, BOOT_SECTOR_SIZE)?;

    // Validate boot sector signature.
    if boot_sector[510] != 0x55 || boot_sector[511] != 0xAA {
        bail!("Invalid FAT32 boot sector signature.");
    }

    // Read BIOS Parameter Block (BPB).
    let bytes_per_sector = read_u16(&boot_sector, 11)?;
    let sectors_per_cluster = boot_sector[13];
    let reserved_sectors = read_u16(&boot_sector, 14)?;
    let number_of_fats = boot_sector[16];
    let root_entry_count = read_u16(&boot_sector, 17);
    let total_sectors_16 = read_u16(&boot_sector, 19);
    let fat_size_16 = read_u16(&boot_sector, 22);
    let total_sectors = read_u32(&boot_sector, 32)?;
    let fat_size_sectors = read_u32(&boot_sector, 36)?;
    let root_cluster = read_u32(&boot_sector, 44)?;

    let root_entry_count = root_entry_count?;
    let total_sectors_16 = total_sectors_16?;
    let fat_size_16 = fat_size_16?;

    // Validate bytes per sector.
    if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096) {
        bail!("Unsupported FAT32 bytes-per-sector value.");
    }

    // Validate sectors per cluster.
    if sectors_per_cluster == 0 || !sectors_per_cluster.is_power_of_two() {
        bail!("Invalid FAT32 sectors-per-cluster value.");
    }

    let cluster_size_bytes = u64::from(bytes_per_sector)
        .checked_mul(u64::from(sectors_per_cluster))
        .context("FAT32 cluster size overflow")?;

    if cluster_size_bytes > MAX_CLUSTER_SIZE {
        bail!("FAT32 cluster size exceeds supported limit.");
    }

    if reserved_sectors == 0 {
        bail!("FAT32 reserved sector count cannot be zero.");
    }

    if number_of_fats == 0 {
        bail!("FAT32 must contain at least one FAT.");
    }

    // Validate FAT32-specific BPB fields.
    if root_entry_count != 0 || total_sectors_16 != 0 || fat_size_16 != 0 {
        bail!("Invalid FAT32 BPB structure.");
    }

    if total_sectors == 0 || fat_size_sectors == 0 {
        bail!("FAT32 volume size or FAT size cannot be zero.");
    }

    // Calculate volume size.
    let volume_size_bytes = u64::from(total_sectors)
        .checked_mul(u64::from(bytes_per_sector))
        .context("FAT32 volume size overflow")?;

    if volume_size_bytes > partition_size {
        bail!("FAT32 volume exceeds partition boundaries.");
    }

    // Calculate FAT region.
    let total_fat_sectors = u64::from(number_of_fats)
        .checked_mul(u64::from(fat_size_sectors))
        .context("FAT32 total FAT size overflow")?;

    let data_start_sector = u64::from(reserved_sectors)
        .checked_add(total_fat_sectors)
        .context("FAT32 data region overflow")?;

    if data_start_sector >= u64::from(total_sectors) {
        bail!("FAT32 data region is outside volume boundaries.");
    }

    // Calculate data clusters.
    let data_sectors = u64::from(total_sectors) - data_start_sector;

    let data_cluster_count = data_sectors / u64::from(sectors_per_cluster);

    if data_cluster_count == 0 {
        bail!("FAT32 volume contains no data clusters.");
    }

    // Calculate FAT capacity.
    let fat_size_bytes = u64::from(fat_size_sectors)
        .checked_mul(u64::from(bytes_per_sector))
        .context("FAT32 FAT size overflow")?;

    let fat_capacity_entries = fat_size_bytes / FAT_ENTRY_SIZE;

    if fat_capacity_entries < data_cluster_count + 2 {
        bail!("FAT32 FAT capacity is insufficient.");
    }

    // Validate root directory cluster.
    if root_cluster < 2 || u64::from(root_cluster) > data_cluster_count + 1 {
        bail!("Invalid FAT32 root directory cluster.");
    }

    Ok(Fat32Metadata {
        bytes_per_sector,
        sectors_per_cluster,
        cluster_size_bytes,
        reserved_sectors,
        number_of_fats,
        total_sectors,
        fat_size_sectors,
        root_cluster,
        data_cluster_count,
        fat_capacity_entries,
        volume_size_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_fat32_image() -> Vec<u8> {
        let start_offset = 2 * 512;

        let mut image = vec![0u8; start_offset + 1024 * 1024];

        let boot = &mut image[start_offset..start_offset + 512];

        boot[11..13].copy_from_slice(&512u16.to_le_bytes());
        boot[13] = 8;
        boot[14..16].copy_from_slice(&32u16.to_le_bytes());
        boot[16] = 2;
        boot[17..19].copy_from_slice(&0u16.to_le_bytes());
        boot[19..21].copy_from_slice(&0u16.to_le_bytes());
        boot[22..24].copy_from_slice(&0u16.to_le_bytes());
        boot[32..36].copy_from_slice(&2048u32.to_le_bytes());
        boot[36..40].copy_from_slice(&4u32.to_le_bytes());
        boot[44..48].copy_from_slice(&2u32.to_le_bytes());

        boot[510] = 0x55;
        boot[511] = 0xAA;

        image
    }

    fn run_reader(image: Vec<u8>, partition_size: u64) -> Result<Fat32Metadata> {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);

        let path = std::env::temp_dir().join(format!(
            "recuvax-fat32-test-{}-{id}.img",
            std::process::id()
        ));

        fs::write(&path, image)?;

        let result = {
            let mut reader = DiskImageReader::open(&path)?;
            read(&mut reader, 2, partition_size)
        };

        let _ = fs::remove_file(&path);

        result
    }

    #[test]
    fn reads_valid_fat32_metadata() {
        let image = create_fat32_image();

        let metadata = run_reader(image, 1024 * 1024).unwrap();

        assert_eq!(metadata.bytes_per_sector, 512);
        assert_eq!(metadata.sectors_per_cluster, 8);
        assert_eq!(metadata.cluster_size_bytes, 4096);
        assert_eq!(metadata.reserved_sectors, 32);
        assert_eq!(metadata.number_of_fats, 2);
        assert_eq!(metadata.total_sectors, 2048);
        assert_eq!(metadata.fat_size_sectors, 4);
        assert_eq!(metadata.root_cluster, 2);
        assert_eq!(metadata.volume_size_bytes, 1024 * 1024);
    }

    #[test]
    fn rejects_invalid_boot_signature() {
        let mut image = create_fat32_image();

        image[2 * 512 + 510] = 0;

        assert!(run_reader(image, 1024 * 1024).is_err());
    }

    #[test]
    fn rejects_volume_exceeding_partition() {
        let image = create_fat32_image();

        assert!(run_reader(image, 512 * 1024).is_err());
    }

    #[test]
    fn rejects_invalid_sectors_per_cluster() {
        let mut image = create_fat32_image();

        image[2 * 512 + 13] = 3;

        assert!(run_reader(image, 1024 * 1024).is_err());
    }

    #[test]
    fn rejects_insufficient_fat_capacity() {
        let mut image = create_fat32_image();

        image[2 * 512 + 36..2 * 512 + 40].copy_from_slice(&1u32.to_le_bytes());

        assert!(run_reader(image, 1024 * 1024).is_err());
    }

    #[test]
    fn rejects_invalid_root_cluster() {
        let mut image = create_fat32_image();

        image[2 * 512 + 44..2 * 512 + 48].copy_from_slice(&u32::MAX.to_le_bytes());

        assert!(run_reader(image, 1024 * 1024).is_err());
    }
}
