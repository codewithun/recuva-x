use anyhow::{Context, Result, bail};
use dialoguer::{Input, theme::ColorfulTheme};
use std::path::PathBuf;

use crate::disk::reader::DiskImageReader;
use crate::recovery::{RecoveryMethod, RecoveryRecord, write_report};

use super::scanner;

fn image_path() -> Result<PathBuf> {
    let path: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Enter disk image path")
        .interact_text()?;

    let path = PathBuf::from(path.trim());

    if path.as_os_str().is_empty() {
        bail!("Disk image path cannot be empty.");
    }

    Ok(path)
}

fn output_directory() -> Result<PathBuf> {
    let path: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Recovery output directory")
        .default("deep-recovered".to_string())
        .interact_text()?;

    let path = PathBuf::from(path.trim());

    if path.as_os_str().is_empty() {
        bail!("Recovery output directory cannot be empty.");
    }

    Ok(path)
}

pub fn run() -> Result<()> {
    println!("\n========== DEEP RECOVERY ==========");
    println!("Supported formats: JPEG, PNG, PDF");
    println!("Disk image will be opened in read-only mode.");

    let image_path = image_path()?;
    let output_dir = output_directory()?;

    let mut reader = DiskImageReader::open(&image_path).context("Failed to open disk image")?;

    println!("\nDisk image: {}", reader.path().display());
    println!("Image size: {} MB", reader.size() / (1024 * 1024));
    println!("Output directory: {}", output_dir.display());

    println!("\nStarting deep recovery scan...");

    let recovered = scanner::scan_image(&mut reader, &output_dir)?;

    println!("\n========== RECOVERY SUMMARY ==========");

    let mut records = Vec::with_capacity(recovered.len());

    if recovered.is_empty() {
        println!("No recoverable files found.");
    } else {
        let total_size = recovered
            .iter()
            .map(|file| file.size)
            .fold(0u64, u64::saturating_add);

        for (index, file) in recovered.iter().enumerate() {
            println!(
                "{}. [{}] {} ({} bytes)",
                index + 1,
                file.format.name(),
                file.path.display(),
                file.size
            );

            records.push(RecoveryRecord::new(
                &file.path,
                &image_path,
                file.format.name(),
                file.size,
                Some(file.source_offset),
                None,
                RecoveryMethod::DeepRecovery,
            ));
        }

        println!("\nTotal recovered files: {}", recovered.len());
        println!("Total recovered size: {} bytes", total_size);
    }

    // Report failure must not invalidate successfully recovered files.
    match write_report(&output_dir, &records) {
        Ok(path) => {
            println!("\nRecovery report: {}", path.display());
        }
        Err(error) => {
            eprintln!(
                "\nWarning: Recovery completed, but the JSON report could not be saved: {error:#}"
            );
        }
    }

    println!("\nRecovery output: {}", output_dir.display());

    Ok(())
}
