pub mod gpt;
pub mod mbr;

use anyhow::Result;

use crate::disk::reader::DiskImageReader;
use crate::filesystem::detector;
use crate::filesystem::metadata;

fn display_filesystem_metadata(
    reader: &mut DiskImageReader,
    filesystem: detector::FileSystem,
    start_lba: u64,
    partition_size: u64,
) {
    if filesystem != detector::FileSystem::Fat32 {
        return;
    }

    match metadata::fat32::read(reader, start_lba, partition_size) {
        Ok(info) => {
            println!("  FAT32 Metadata:");
            println!("    Bytes per Sector    : {}", info.bytes_per_sector);
            println!("    Sectors per Cluster : {}", info.sectors_per_cluster);
            println!(
                "    Cluster Size        : {} bytes",
                info.cluster_size_bytes
            );
            println!("    Reserved Sectors    : {}", info.reserved_sectors);
            println!("    Number of FATs      : {}", info.number_of_fats);
            println!("    Total Sectors       : {}", info.total_sectors);
            println!(
                "    FAT Size            : {} sectors",
                info.fat_size_sectors
            );
            println!("    Root Cluster        : {}", info.root_cluster);
            println!("    Data Clusters       : {}", info.data_cluster_count);
            println!(
                "    FAT Capacity        : {} entries",
                info.fat_capacity_entries
            );
            println!("    Volume Size         : {} bytes", info.volume_size_bytes);
        }
        Err(error) => {
            println!("  FAT32 Metadata       : Unable to read metadata");
            println!("  Metadata Error       : {error:#}");
        }
    }
}

pub fn analyze(reader: &mut DiskImageReader) -> Result<()> {
    println!("\n========== PARTITION ANALYSIS ==========");

    if reader.size() < 1024 {
        println!("Image is too small to contain a standard partition table.");
        return Ok(());
    }

    let gpt_header = reader.read_sector(1)?;

    if &gpt_header[0..8] == b"EFI PART" {
        println!("Partition scheme: GPT");

        let partitions = gpt::parse(reader)?;

        if partitions.is_empty() {
            println!("No allocated GPT partitions found.");
        }

        for partition in &partitions {
            println!("\nPartition #{}", partition.index);
            println!("  Name       : {}", partition.name);
            println!("  Type GUID  : {}", partition.type_guid);
            println!("  Unique GUID: {}", partition.unique_guid);
            println!("  First LBA  : {}", partition.first_lba);
            println!("  Last LBA   : {}", partition.last_lba);
            println!("  Size       : {} bytes", partition.size_bytes());

            let filesystem = detector::detect(reader, partition.first_lba, partition.size_bytes())?;

            println!("  Filesystem : {}", filesystem);

            display_filesystem_metadata(
                reader,
                filesystem,
                partition.first_lba,
                partition.size_bytes(),
            );
        }

        return Ok(());
    }

    let first_sector = reader.read_sector(0)?;

    if first_sector[510] != 0x55 || first_sector[511] != 0xAA {
        println!("Partition scheme: UNKNOWN");
        println!("No valid MBR or GPT signature found.");
        return Ok(());
    }

    println!("Partition scheme: MBR");

    let partitions = mbr::parse(reader)?;

    if partitions.is_empty() {
        println!("No allocated MBR partitions found.");
    }

    for partition in &partitions {
        println!("\nPartition #{}", partition.index);
        println!("  Bootable : {}", partition.bootable);
        println!(
            "  Type     : 0x{:02X} ({})",
            partition.partition_type,
            mbr::partition_type_name(partition.partition_type)
        );
        println!("  Start LBA: {}", partition.start_lba);
        println!("  Sectors  : {}", partition.sectors);
        println!("  Size     : {} bytes", partition.size_bytes());

        let filesystem = detector::detect(
            reader,
            u64::from(partition.start_lba),
            partition.size_bytes(),
        )?;

        println!("  Filesystem: {}", filesystem);

        display_filesystem_metadata(
            reader,
            filesystem,
            u64::from(partition.start_lba),
            partition.size_bytes(),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests;
