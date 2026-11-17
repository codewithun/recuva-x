mod devices;
mod disk;
mod filesystem;
mod partition;

use anyhow::Result;
use dialoguer::{Input, Select, theme::ColorfulTheme};
use owo_colors::OwoColorize;
use std::io;

fn print_banner() {
    println!(
        "{}",
        r#"
██████╗ ███████╗ ██████╗██╗   ██╗██╗   ██╗ █████╗
██╔══██╗██╔════╝██╔════╝██║   ██║██║   ██║██╔══██╗
██████╔╝█████╗  ██║     ██║   ██║██║   ██║███████║
██╔══██╗██╔══╝  ██║     ██║   ██║██║   ██║██╔══██║
██║  ██║███████╗╚██████╗╚██████╔╝ ╚████╔╝ ██║  ██║
╚═╝  ╚═╝╚══════╝ ╚═════╝ ╚═════╝   ╚═══╝  ╚═╝  ╚═╝
"#
        .bright_cyan()
    );

    println!(
        "{}",
        "       ADVANCED CLI DATA RECOVERY TOOLKIT"
            .bright_white()
            .bold()
    );

    println!("{}", "       Version 0.1.0 | Rust Edition".dimmed());
    println!();
}

fn pause() -> Result<()> {
    println!("\nPress Enter to continue...");

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(())
}

fn handle_selection(selection: usize) -> Result<bool> {
    match selection {
        0 => {
            println!("{}", "\nDevice Information".bright_cyan());

            match devices::discovery::list_devices() {
                Ok(()) => {
                    println!("{}", "\nDevice discovery completed.".bright_green());
                }
                Err(error) => {
                    eprintln!("{} {}", "[ERROR]".bright_red().bold(), error);
                }
            }

            pause()?;
        }

        1 => {
            println!("{}", "\nDisk Analysis".bright_cyan());
            println!("Disk analysis is not implemented yet.");
            pause()?;
        }

        2 => {
            println!("{}", "\nQuick Recovery".bright_cyan());
            println!("Recovery engine is not implemented yet.");
            pause()?;
        }

        3 => {
            println!("{}", "\nDeep Recovery".bright_cyan());
            println!("File carving is not implemented yet.");
            pause()?;
        }

        4 => {
            println!("{}", "\nPartition Recovery".bright_cyan());
            println!("Partition recovery is not implemented yet.");
            pause()?;
        }

        5 => {
            println!("{}", "\nDisk Image Analysis".bright_cyan());

            let path: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Enter disk image path")
                .interact_text()?;

            match disk::image::analyze(&path) {
                Ok(()) => {
                    println!("{}", "\nAnalysis completed.".bright_green());
                }
                Err(error) => {
                    eprintln!("{} {}", "[ERROR]".bright_red().bold(), error);
                }
            }

            pause()?;
        }

        6 => {
            println!("{}", "\nRecovery History".bright_cyan());
            println!("Recovery history is not implemented yet.");
            pause()?;
        }

        7 => {
            println!("{}", "\nSettings".bright_cyan());
            println!("Settings are not implemented yet.");
            pause()?;
        }

        8 => {
            println!("{}", "\nExiting RecuvaX...".yellow());
            return Ok(false);
        }

        _ => {}
    }

    Ok(true)
}

fn main() -> Result<()> {
    loop {
        print_banner();

        let options = [
            "Device Information",
            "Disk Analysis",
            "Quick Recovery",
            "Deep Recovery",
            "Partition Recovery",
            "Disk Image Analysis",
            "Recovery History",
            "Settings",
            "Exit",
        ];

        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Select an operation")
            .items(&options)
            .default(0)
            .interact()?;

        if !handle_selection(selection)? {
            break;
        }
    }

    Ok(())
}
