use anyhow::{Context, Result, bail};
use std::process::{Command, Output};

pub fn list_devices() -> Result<()> {
    println!("\n========== DEVICE DISCOVERY ==========\n");

    println!("Platform: {}", std::env::consts::OS);
    println!("Architecture: {}", std::env::consts::ARCH);

    let output = get_device_list()?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);

        bail!("Device discovery failed: {}", error.trim());
    }

    let devices = String::from_utf8_lossy(&output.stdout);

    if devices.trim().is_empty() {
        println!("No storage devices found.");
    } else {
        println!("{}", devices);
    }

    println!("======================================");

    Ok(())
}

#[cfg(target_os = "macos")]
fn get_device_list() -> Result<Output> {
    Command::new("diskutil")
        .arg("list")
        .output()
        .context("Failed to execute diskutil. Is it available?")
}

#[cfg(target_os = "linux")]
fn get_device_list() -> Result<Output> {
    Command::new("lsblk")
        .args([
            "-b",
            "-o",
            "NAME,PATH,SIZE,TYPE,MODEL,TRAN,RO,RM,MOUNTPOINTS",
        ])
        .output()
        .context("Failed to execute lsblk. Is it installed?")
}

#[cfg(target_os = "windows")]
fn get_device_list() -> Result<Output> {
    let script = "Get-CimInstance Win32_DiskDrive | Select-Object Index,Model,Size,MediaType,InterfaceType,DeviceID | Format-Table -AutoSize | Out-String -Width 240";

    Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .context("Failed to execute PowerShell.")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn get_device_list() -> Result<Output> {
    bail!("This operating system is not supported yet.")
}
