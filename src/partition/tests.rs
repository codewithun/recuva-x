use super::{gpt, mbr};
use crate::disk::reader::DiskImageReader;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

const SECTOR_SIZE: usize = 512;
const TOTAL_SECTORS: usize = 4096;
const ENTRY_COUNT: usize = 128;
const ENTRY_SIZE: usize = 128;
const TABLE_SIZE: usize = ENTRY_COUNT * ENTRY_SIZE;

static TEST_ID: AtomicU64 = AtomicU64::new(0);

fn with_reader<T>(image: &[u8], test: impl FnOnce(&mut DiskImageReader) -> T) -> T {
    let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("recuva-x-test-{}-{}.img", std::process::id(), id));

    fs::write(&path, image).expect("Failed to create test image");

    let mut reader = DiskImageReader::open(&path).expect("Failed to open test image");

    let result = test(&mut reader);

    drop(reader);
    fs::remove_file(path).expect("Failed to remove test image");

    result
}

fn create_mbr_image(with_partition: bool) -> Vec<u8> {
    let mut image = vec![0u8; TOTAL_SECTORS * SECTOR_SIZE];

    image[510] = 0x55;
    image[511] = 0xAA;

    if with_partition {
        let offset = 446;

        image[offset + 4] = 0x83;

        image[offset + 8..offset + 12].copy_from_slice(&2048u32.to_le_bytes());

        image[offset + 12..offset + 16].copy_from_slice(&1024u32.to_le_bytes());
    }

    image
}

fn create_gpt_image() -> Vec<u8> {
    let mut image = vec![0u8; TOTAL_SECTORS * SECTOR_SIZE];

    // Protective MBR.
    image[450] = 0xEE;
    image[454..458].copy_from_slice(&1u32.to_le_bytes());
    image[458..462].copy_from_slice(&((TOTAL_SECTORS - 1) as u32).to_le_bytes());

    image[510] = 0x55;
    image[511] = 0xAA;

    // Partition entry table.
    let mut table = vec![0u8; TABLE_SIZE];

    // Linux filesystem type GUID.
    let type_guid = [
        0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D,
        0xE4,
    ];

    table[0..16].copy_from_slice(&type_guid);
    table[16..32].fill(0x11);

    table[32..40].copy_from_slice(&2048u64.to_le_bytes());
    table[40..48].copy_from_slice(&3071u64.to_le_bytes());

    let name: Vec<u8> = "Linux Data"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();

    table[56..56 + name.len()].copy_from_slice(&name);

    let table_crc = crc32fast::hash(&table);

    let primary_table_lba = 2u64;
    let backup_table_lba = 4095 - 32;

    let primary_offset = primary_table_lba as usize * SECTOR_SIZE;
    let backup_offset = backup_table_lba as usize * SECTOR_SIZE;

    image[primary_offset..primary_offset + TABLE_SIZE].copy_from_slice(&table);

    image[backup_offset..backup_offset + TABLE_SIZE].copy_from_slice(&table);

    // Build primary and backup headers.
    fn build_header(current: u64, backup: u64, entries: u64, table_crc: u32) -> Vec<u8> {
        let mut header = vec![0u8; SECTOR_SIZE];

        header[0..8].copy_from_slice(b"EFI PART");
        header[8..12].copy_from_slice(&0x00010000u32.to_le_bytes());
        header[12..16].copy_from_slice(&92u32.to_le_bytes());

        header[24..32].copy_from_slice(&current.to_le_bytes());
        header[32..40].copy_from_slice(&backup.to_le_bytes());

        header[40..48].copy_from_slice(&34u64.to_le_bytes());
        header[48..56].copy_from_slice(&4062u64.to_le_bytes());

        header[56..72].fill(0x22);

        header[72..80].copy_from_slice(&entries.to_le_bytes());
        header[80..84].copy_from_slice(&(ENTRY_COUNT as u32).to_le_bytes());
        header[84..88].copy_from_slice(&(ENTRY_SIZE as u32).to_le_bytes());
        header[88..92].copy_from_slice(&table_crc.to_le_bytes());

        let crc = crc32fast::hash(&header[..92]);
        header[16..20].copy_from_slice(&crc.to_le_bytes());

        header
    }

    let primary_header = build_header(1, 4095, 2, table_crc);
    let backup_header = build_header(4095, 1, backup_table_lba, table_crc);

    image[SECTOR_SIZE..2 * SECTOR_SIZE].copy_from_slice(&primary_header);

    let backup_header_offset = 4095 * SECTOR_SIZE;

    image[backup_header_offset..backup_header_offset + SECTOR_SIZE].copy_from_slice(&backup_header);

    image
}

#[test]
fn mbr_parses_valid_partition() {
    let image = create_mbr_image(true);

    with_reader(&image, |reader| {
        let partitions = mbr::parse(reader).unwrap();

        assert_eq!(partitions.len(), 1);
        assert_eq!(partitions[0].partition_type, 0x83);
        assert_eq!(partitions[0].start_lba, 2048);
        assert_eq!(partitions[0].sectors, 1024);
        assert_eq!(partitions[0].size_bytes(), 524288);
    });
}

#[test]
fn mbr_handles_empty_partition_table() {
    let image = create_mbr_image(false);

    with_reader(&image, |reader| {
        let partitions = mbr::parse(reader).unwrap();
        assert!(partitions.is_empty());
    });
}

#[test]
fn mbr_rejects_partition_outside_image() {
    let mut image = create_mbr_image(true);

    let offset = 446 + 8;
    image[offset..offset + 4].copy_from_slice(&4000u32.to_le_bytes());

    with_reader(&image, |reader| {
        assert!(mbr::parse(reader).is_err());
    });
}

#[test]
fn gpt_parses_valid_partition_and_backup() {
    let image = create_gpt_image();

    with_reader(&image, |reader| {
        let partitions = gpt::parse(reader).unwrap();

        assert_eq!(partitions.len(), 1);
        assert_eq!(partitions[0].name, "Linux Data");
        assert_eq!(partitions[0].first_lba, 2048);
        assert_eq!(partitions[0].last_lba, 3071);
        assert_eq!(partitions[0].size_bytes(), 524288);
    });
}

#[test]
fn gpt_rejects_corrupted_primary_table() {
    let mut image = create_gpt_image();

    image[2 * SECTOR_SIZE + 56] ^= 0x01;

    with_reader(&image, |reader| {
        let error = gpt::parse(reader).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Primary GPT partition table CRC mismatch")
        );
    });
}

#[test]
fn gpt_rejects_corrupted_backup_header() {
    let mut image = create_gpt_image();

    let offset = 4095 * SECTOR_SIZE;
    image[offset + 56] ^= 0x01;

    with_reader(&image, |reader| {
        let error = gpt::parse(reader).unwrap_err();
        assert!(error.to_string().contains("GPT header CRC mismatch"));
    });
}

#[test]
fn gpt_rejects_corrupted_backup_table() {
    let mut image = create_gpt_image();

    let backup_table_lba = 4095 - 32;
    let offset = backup_table_lba * SECTOR_SIZE + 56;

    image[offset] ^= 0x01;

    with_reader(&image, |reader| {
        let error = gpt::parse(reader).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Backup GPT partition table CRC mismatch")
        );
    });
}

#[test]
fn gpt_rejects_inconsistent_primary_and_backup() {
    let mut image = create_gpt_image();

    let backup_table_lba = 4095 - 32;
    let backup_table_offset = backup_table_lba * SECTOR_SIZE;
    let backup_header_offset = 4095 * SECTOR_SIZE;

    image[backup_table_offset + 56] ^= 0x01;

    let table = &image[backup_table_offset..backup_table_offset + TABLE_SIZE];
    let table_crc = crc32fast::hash(table);

    image[backup_header_offset + 88..backup_header_offset + 92]
        .copy_from_slice(&table_crc.to_le_bytes());

    let mut header = image[backup_header_offset..backup_header_offset + 92].to_vec();

    header[16..20].fill(0);

    let header_crc = crc32fast::hash(&header);

    image[backup_header_offset + 16..backup_header_offset + 20]
        .copy_from_slice(&header_crc.to_le_bytes());

    with_reader(&image, |reader| {
        let error = gpt::parse(reader).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Primary and backup GPT partition table metadata do not match")
        );
    });
}
