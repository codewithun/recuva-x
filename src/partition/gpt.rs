use crate::disk::reader::{DiskImageReader, SECTOR_SIZE};
use anyhow::{Context, Result, bail};

const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";
const MIN_HEADER_SIZE: u32 = 92;
const MAX_HEADER_SIZE: u32 = 512;
const MAX_ENTRY_COUNT: u32 = 4096;
const MAX_ENTRY_SIZE: u32 = 4096;
const MAX_TABLE_SIZE: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct GptPartition {
    pub index: usize,
    pub type_guid: String,
    pub unique_guid: String,
    pub first_lba: u64,
    pub last_lba: u64,
    pub name: String,
}

impl GptPartition {
    pub fn size_bytes(&self) -> u64 {
        (self.last_lba - self.first_lba + 1) * SECTOR_SIZE
    }
}

struct GptHeader {
    current_lba: u64,
    backup_lba: u64,
    first_usable_lba: u64,
    last_usable_lba: u64,
    disk_guid: [u8; 16],
    entries_lba: u64,
    entry_count: u32,
    entry_size: u32,
    entries_crc: u32,
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes: [u8; 4] = data
        .get(offset..offset + 4)
        .context("Invalid GPT field")?
        .try_into()?;

    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64> {
    let bytes: [u8; 8] = data
        .get(offset..offset + 8)
        .context("Invalid GPT field")?
        .try_into()?;

    Ok(u64::from_le_bytes(bytes))
}

fn format_guid(bytes: &[u8]) -> Result<String> {
    if bytes.len() != 16 {
        bail!("Invalid GUID length");
    }

    Ok(format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        u32::from_le_bytes(bytes[0..4].try_into()?),
        u16::from_le_bytes(bytes[4..6].try_into()?),
        u16::from_le_bytes(bytes[6..8].try_into()?),
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

fn read_header(reader: &mut DiskImageReader, lba: u64) -> Result<GptHeader> {
    let sector = reader.read_sector(lba)?;

    if &sector[0..8] != GPT_SIGNATURE {
        bail!("GPT signature not found at LBA {}.", lba);
    }

    let header_size = read_u32(&sector, 12)?;

    if !(MIN_HEADER_SIZE..=MAX_HEADER_SIZE).contains(&header_size) {
        bail!("Invalid GPT header size at LBA {}.", lba);
    }

    let stored_crc = read_u32(&sector, 16)?;

    let mut header_data = sector[..header_size as usize].to_vec();

    // CRC field must be zero while calculating the header CRC.
    header_data[16..20].fill(0);

    let calculated_crc = crc32fast::hash(&header_data);

    if calculated_crc != stored_crc {
        bail!(
            "GPT header CRC mismatch at LBA {}. Expected {:08X}, calculated {:08X}.",
            lba,
            stored_crc,
            calculated_crc
        );
    }

    let current_lba = read_u64(&sector, 24)?;
    let backup_lba = read_u64(&sector, 32)?;
    let first_usable_lba = read_u64(&sector, 40)?;
    let last_usable_lba = read_u64(&sector, 48)?;

    if current_lba != lba {
        bail!("GPT current LBA does not match its physical location.");
    }

    if backup_lba >= reader.sector_count() || backup_lba == current_lba {
        bail!("Invalid GPT backup header location.");
    }

    if first_usable_lba > last_usable_lba || last_usable_lba >= reader.sector_count() {
        bail!("Invalid GPT usable disk boundaries.");
    }

    let mut disk_guid = [0u8; 16];
    disk_guid.copy_from_slice(&sector[56..72]);

    let entries_lba = read_u64(&sector, 72)?;
    let entry_count = read_u32(&sector, 80)?;
    let entry_size = read_u32(&sector, 84)?;
    let entries_crc = read_u32(&sector, 88)?;

    if entry_count == 0 || entry_count > MAX_ENTRY_COUNT {
        bail!("Unsupported GPT partition entry count.");
    }

    if !(128..=MAX_ENTRY_SIZE).contains(&entry_size) || entry_size % 8 != 0 {
        bail!("Invalid GPT partition entry size.");
    }

    let table_size = u64::from(entry_count)
        .checked_mul(u64::from(entry_size))
        .context("GPT entry table size overflow")?;

    if table_size > MAX_TABLE_SIZE {
        bail!("GPT partition table is too large.");
    }

    Ok(GptHeader {
        current_lba,
        backup_lba,
        first_usable_lba,
        last_usable_lba,
        disk_guid,
        entries_lba,
        entry_count,
        entry_size,
        entries_crc,
    })
}

fn read_partition_table(
    reader: &mut DiskImageReader,
    header: &GptHeader,
    is_backup: bool,
) -> Result<Vec<u8>> {
    let table_size = u64::from(header.entry_count)
        .checked_mul(u64::from(header.entry_size))
        .context("GPT table size overflow")?;

    let table_sectors = table_size
        .checked_add(SECTOR_SIZE - 1)
        .context("GPT table sector calculation overflow")?
        / SECTOR_SIZE;

    let table_end = header
        .entries_lba
        .checked_add(table_sectors)
        .context("GPT table boundary overflow")?;

    if is_backup {
        if header.entries_lba <= header.last_usable_lba || table_end > header.current_lba {
            bail!("Backup GPT partition table overlaps an invalid region.");
        }
    } else if header.entries_lba < 2 || table_end > header.first_usable_lba {
        bail!("Primary GPT partition table overlaps an invalid region.");
    }

    let table_offset = header
        .entries_lba
        .checked_mul(SECTOR_SIZE)
        .context("GPT table offset overflow")?;

    let table = reader.read_bytes(table_offset, usize::try_from(table_size)?)?;

    let calculated_crc = crc32fast::hash(&table);

    if calculated_crc != header.entries_crc {
        bail!(
            "{} GPT partition table CRC mismatch. Expected {:08X}, calculated {:08X}.",
            if is_backup { "Backup" } else { "Primary" },
            header.entries_crc,
            calculated_crc
        );
    }

    Ok(table)
}

fn verify_backup_header(primary: &GptHeader, backup: &GptHeader) -> Result<()> {
    if backup.current_lba != primary.backup_lba {
        bail!("Backup GPT header is at an unexpected location.");
    }

    if backup.backup_lba != primary.current_lba {
        bail!("Backup GPT does not point back to the primary header.");
    }

    if backup.disk_guid != primary.disk_guid {
        bail!("Primary and backup GPT disk GUIDs do not match.");
    }

    if backup.first_usable_lba != primary.first_usable_lba
        || backup.last_usable_lba != primary.last_usable_lba
    {
        bail!("Primary and backup GPT usable boundaries do not match.");
    }

    if backup.entry_count != primary.entry_count
        || backup.entry_size != primary.entry_size
        || backup.entries_crc != primary.entries_crc
    {
        bail!("Primary and backup GPT partition table metadata do not match.");
    }

    Ok(())
}

pub fn parse(reader: &mut DiskImageReader) -> Result<Vec<GptPartition>> {
    if reader.sector_count() < 2 {
        bail!("Image is too small to contain a GPT header.");
    }

    println!("\nGPT Integrity Validation:");

    // Validate primary GPT header and CRC.
    let primary = read_header(reader, 1)?;
    println!("  Primary header CRC : VALID");

    // Validate primary partition table and CRC.
    let primary_table = read_partition_table(reader, &primary, false)?;
    println!("  Primary table CRC  : VALID");

    // Validate backup GPT header and CRC.
    let backup = read_header(reader, primary.backup_lba)?;
    println!("  Backup header CRC  : VALID");

    // Verify primary and backup metadata consistency.
    verify_backup_header(&primary, &backup)?;

    // Validate backup partition table and CRC.
    let backup_table = read_partition_table(reader, &backup, true)?;
    println!("  Backup table CRC   : VALID");

    if primary_table != backup_table {
        bail!("Primary and backup GPT partition tables contain different data.");
    }

    println!("  GPT consistency    : VALID");

    let mut partitions = Vec::new();

    for index in 0..primary.entry_count as usize {
        let offset = index * primary.entry_size as usize;
        let entry = &primary_table[offset..offset + primary.entry_size as usize];

        let type_guid = &entry[0..16];

        if type_guid.iter().all(|byte| *byte == 0) {
            continue;
        }

        let unique_guid = &entry[16..32];

        let first_lba = u64::from_le_bytes(entry[32..40].try_into()?);
        let last_lba = u64::from_le_bytes(entry[40..48].try_into()?);

        if first_lba > last_lba
            || first_lba < primary.first_usable_lba
            || last_lba > primary.last_usable_lba
        {
            bail!("Invalid GPT partition boundary at entry {}.", index + 1);
        }

        let name_bytes = &entry[56..primary.entry_size as usize];

        let utf16: Vec<u16> = name_bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .take_while(|value| *value != 0)
            .collect();

        let name = String::from_utf16_lossy(&utf16);

        partitions.push(GptPartition {
            index: index + 1,
            type_guid: format_guid(type_guid)?,
            unique_guid: format_guid(unique_guid)?,
            first_lba,
            last_lba,
            name,
        });
    }

    Ok(partitions)
}
