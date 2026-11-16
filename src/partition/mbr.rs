use anyhow::{Result, bail};

use crate::disk::reader::{DiskImageReader, SECTOR_SIZE};

#[derive(Debug)]
pub struct MbrPartition {
    pub index: usize,
    pub bootable: bool,
    pub partition_type: u8,
    pub start_lba: u32,
    pub sectors: u32,
}

impl MbrPartition {
    pub fn size_bytes(&self) -> u64 {
        u64::from(self.sectors) * SECTOR_SIZE
    }
}

pub fn parse(reader: &mut DiskImageReader) -> Result<Vec<MbrPartition>> {
    let sector = reader.read_sector(0)?;

    if sector[510] != 0x55 || sector[511] != 0xAA {
        bail!("Valid MBR signature not found.");
    }

    let mut partitions = Vec::new();

    for index in 0..4 {
        let offset = 446 + (index * 16);
        let entry = &sector[offset..offset + 16];

        let partition_type = entry[4];

        let start_lba = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);

        let sectors = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]);

        if partition_type == 0 || sectors == 0 {
            continue;
        }

        let end_lba = u64::from(start_lba)
            .checked_add(u64::from(sectors))
            .ok_or_else(|| anyhow::anyhow!("MBR partition boundary overflow"))?;

        if end_lba > reader.sector_count() {
            bail!("MBR partition {} exceeds image boundaries.", index + 1);
        }

        partitions.push(MbrPartition {
            index: index + 1,
            bootable: entry[0] == 0x80,
            partition_type,
            start_lba,
            sectors,
        });
    }

    Ok(partitions)
}

pub fn partition_type_name(partition_type: u8) -> &'static str {
    match partition_type {
        0x01 => "FAT12",
        0x04 | 0x06 | 0x0E => "FAT16",
        0x07 => "NTFS / exFAT / HPFS",
        0x0B | 0x0C => "FAT32",
        0x0F | 0x05 => "Extended",
        0x82 => "Linux Swap",
        0x83 => "Linux Filesystem",
        0xEE => "GPT Protective",
        _ => "Unknown",
    }
}
