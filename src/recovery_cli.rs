use anyhow::{Context, Result};
use dialoguer::{Input, Select, theme::ColorfulTheme};
use std::path::{Path, PathBuf};

use crate::disk::reader::DiskImageReader;
use crate::filesystem::{
    detector::{self, FileSystem},
    directory::{entries::DirectoryEntry, fat32::Fat32ClusterReader},
    recovery,
};
use crate::partition::{gpt, mbr};
use crate::recovery::{RecoveryRecord, write_report};

struct PartitionCandidate {
    index: usize,
    start_lba: u64,
    size_bytes: u64,
}

fn discover_fat32_partitions(reader: &mut DiskImageReader) -> Result<Vec<PartitionCandidate>> {
    let mut candidates = Vec::new();

    if reader.sector_count() < 2 {
        anyhow::bail!("Disk image is too small.");
    }

    let sector = reader.read_sector(1)?;

    if &sector[0..8] == b"EFI PART" {
        let partitions = gpt::parse(reader)?;

        for partition in partitions {
            let size = partition.size_bytes();

            if detector::detect(reader, partition.first_lba, size)? == FileSystem::Fat32 {
                candidates.push(PartitionCandidate {
                    index: partition.index,
                    start_lba: partition.first_lba,
                    size_bytes: size,
                });
            }
        }
    } else {
        let partitions = mbr::parse(reader)?;

        for partition in partitions {
            let size = partition.size_bytes();

            if detector::detect(reader, u64::from(partition.start_lba), size)? == FileSystem::Fat32
            {
                candidates.push(PartitionCandidate {
                    index: partition.index,
                    start_lba: u64::from(partition.start_lba),
                    size_bytes: size,
                });
            }
        }
    }

    Ok(candidates)
}

fn output_directory() -> Result<PathBuf> {
    let path: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Recovery output directory")
        .default("recovered".to_string())
        .interact_text()?;

    let path = PathBuf::from(path.trim());

    if path.as_os_str().is_empty() {
        anyhow::bail!("Output directory cannot be empty.");
    }

    Ok(path)
}

/// Write a report without treating report failure as recovery failure.
fn export_recovery_report(output_dir: &Path, records: &[RecoveryRecord]) {
    match write_report(output_dir, records) {
        Ok(path) => {
            println!("Recovery report: {}", path.display());
        }
        Err(error) => {
            eprintln!(
                "Warning: Files were recovered, but the JSON report could not be saved: {error:#}"
            );
        }
    }
}

fn recover_single_file(
    reader: &mut Fat32ClusterReader<'_>,
    entry: &DirectoryEntry,
    source_image: &Path,
) -> Result<()> {
    let output = output_directory()?;

    let record = recovery::recover_file_with_metadata(reader, entry, &output, source_image)?;

    println!("\nRecovered successfully!");
    println!("Output: {}", record.output_path);
    println!("Source cluster: {:?}", record.source_cluster);

    export_recovery_report(&output, &[record]);

    Ok(())
}

fn recover_selected_directory(
    reader: &mut Fat32ClusterReader<'_>,
    entry: &DirectoryEntry,
    source_image: &Path,
) -> Result<()> {
    let output = output_directory()?.join(&entry.name);

    let records = recovery::recover_directory_with_metadata(
        reader,
        entry.first_cluster,
        &output,
        source_image,
    )?;

    println!("\nRecovered {} files.", records.len());
    println!("Output: {}", output.display());

    export_recovery_report(&output, &records);

    Ok(())
}

fn recover_current_directory(
    reader: &mut Fat32ClusterReader<'_>,
    cluster: u32,
    source_image: &Path,
) -> Result<()> {
    let output = output_directory()?;

    let records =
        recovery::recover_directory_with_metadata(reader, cluster, &output, source_image)?;

    println!("\nRecovered {} files.", records.len());
    println!("Output: {}", output.display());

    export_recovery_report(&output, &records);

    Ok(())
}

fn browse_directory(
    reader: &mut Fat32ClusterReader<'_>,
    root_cluster: u32,
    source_image: &Path,
) -> Result<()> {
    let mut stack = vec![(root_cluster, String::from("/"))];

    loop {
        let (current_cluster, current_name) = stack
            .last()
            .context("Directory navigation stack is empty")?;

        println!("\n========== QUICK RECOVERY ==========");
        println!("Current directory: {current_name}");
        println!("Cluster: {current_cluster}");

        let entries = reader.read_directory(*current_cluster)?;

        let mut options = vec!["Recover current directory".to_string()];

        for entry in &entries {
            let label = if entry.is_directory() {
                format!("[DIR]  {}", entry.name)
            } else {
                format!("[FILE] {} ({} bytes)", entry.name, entry.size)
            };

            options.push(label);
        }

        if stack.len() > 1 {
            options.push("Go to parent directory".to_string());
        }

        options.push("Cancel".to_string());

        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Select an item")
            .items(&options)
            .default(0)
            .interact()?;

        if selection == 0 {
            match recover_current_directory(reader, *current_cluster, source_image) {
                Ok(()) => {}
                Err(error) => eprintln!("\nRecovery failed: {error:#}"),
            }

            continue;
        }

        if selection <= entries.len() {
            let entry = entries[selection - 1].clone();

            if entry.is_directory() {
                let actions = ["Open directory", "Recover directory", "Back"];

                let action = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt(&entry.name)
                    .items(actions)
                    .default(0)
                    .interact()?;

                match action {
                    0 => {
                        stack.push((entry.first_cluster, entry.name.clone()));
                    }
                    1 => match recover_selected_directory(reader, &entry, source_image) {
                        Ok(()) => {}
                        Err(error) => {
                            eprintln!("\nRecovery failed: {error:#}");
                        }
                    },
                    _ => {}
                }
            } else {
                match recover_single_file(reader, &entry, source_image) {
                    Ok(()) => {}
                    Err(error) => {
                        eprintln!("\nRecovery failed: {error:#}");
                    }
                }
            }

            continue;
        }

        let parent_index = entries.len() + 1;

        if stack.len() > 1 && selection == parent_index {
            stack.pop();
            continue;
        }

        break;
    }

    Ok(())
}

pub fn run() -> Result<()> {
    println!("\n========== QUICK RECOVERY ==========");

    let image_path: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Enter disk image path")
        .interact_text()?;

    let source_image = PathBuf::from(image_path.trim());

    let mut reader = DiskImageReader::open(&source_image)?;

    println!("\nScanning partitions...");

    let partitions = discover_fat32_partitions(&mut reader)?;

    if partitions.is_empty() {
        println!("No supported FAT32 partitions found.");
        return Ok(());
    }

    let options: Vec<String> = partitions
        .iter()
        .map(|partition| {
            format!(
                "Partition #{} | Start LBA: {} | Size: {} MB",
                partition.index,
                partition.start_lba,
                partition.size_bytes / (1024 * 1024)
            )
        })
        .collect();

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select FAT32 partition")
        .items(&options)
        .default(0)
        .interact()?;

    let partition = &partitions[selection];

    let mut fat_reader =
        Fat32ClusterReader::new(&mut reader, partition.start_lba, partition.size_bytes)?;

    let root_cluster = fat_reader.metadata().root_cluster;

    println!("\nFAT32 partition selected.");
    println!("Root cluster: {root_cluster}");

    browse_directory(&mut fat_reader, root_cluster, &source_image)?;

    Ok(())
}
