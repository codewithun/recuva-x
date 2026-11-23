use anyhow::{Context, Result, bail};
use dialoguer::{Input, theme::ColorfulTheme};
use std::path::PathBuf;

use crate::disk::reader::DiskImageReader;

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
        bail!("Output directory cannot be empty.");
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

    if recovered.is_empty() {
        println!("No recoverable files found.");
    } else {
        let total_size: u64 = recovered.iter().map(|file| file.size).sum();

        for (index, file) in recovered.iter().enumerate() {
            println!(
                "{}. [{}] {} ({} bytes)",
                index + 1,
                file.format.name(),
                file.path.display(),
                file.size
            );
        }

        println!("\nTotal recovered files: {}", recovered.len());
        println!("Total recovered size: {} bytes", total_size);
    }

    println!("\nRecovery output: {}", output_dir.display());

    Ok(())
}
