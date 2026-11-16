pub mod gpt;
pub mod mbr;

use anyhow::Result;

use crate::disk::reader::DiskImageReader;

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

        for partition in partitions {
            println!("\nPartition #{}", partition.index);
            println!("  Name       : {}", partition.name);
            println!("  Type GUID  : {}", partition.type_guid);
            println!("  Unique GUID: {}", partition.unique_guid);
            println!("  First LBA  : {}", partition.first_lba);
            println!("  Last LBA   : {}", partition.last_lba);
            println!("  Size       : {} bytes", partition.size_bytes());
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

    for partition in partitions {
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
    }

    Ok(())
}

#[cfg(test)]
mod tests;
