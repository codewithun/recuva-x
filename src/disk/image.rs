use anyhow::Result;

use super::reader::{DiskImageReader, SECTOR_SIZE};

pub fn analyze(path: &str) -> Result<()> {
    let mut reader = DiskImageReader::open(path)?;

    println!("\n========== DISK IMAGE ANALYSIS ==========");

    println!("File       : {}", reader.path().display());
    println!("Size       : {} bytes", reader.size());
    println!("Sectors    : {}", reader.sector_count());
    println!("Sector Size: {} bytes", SECTOR_SIZE);

    if reader.size() < SECTOR_SIZE {
        println!("\nWarning: Image is smaller than one sector.");
        return Ok(());
    }

    let first_sector = reader.read_sector(0)?;

    let has_mbr_signature = first_sector[510] == 0x55 && first_sector[511] == 0xAA;

    println!("\nPartition Table:");

    if has_mbr_signature {
        println!("  MBR signature: FOUND");
    } else {
        println!("  MBR signature: NOT FOUND");
    }

    if reader.size() >= SECTOR_SIZE * 2 {
        let second_sector = reader.read_sector(1)?;

        let has_gpt_signature = &second_sector[0..8] == b"EFI PART";

        if has_gpt_signature {
            println!("  GPT signature: FOUND");
        } else {
            println!("  GPT signature: NOT FOUND");
        }
    } else {
        println!("  GPT signature: NOT CHECKED (image too small)");
    }

    println!("\nFirst 16 bytes:");

    for byte in &first_sector[..16] {
        print!("{:02X} ", byte);
    }

    crate::partition::analyze(&mut reader)?;
    println!("\n\n=========================================");

    Ok(())
}
