mod agent;
mod db;
mod export;
mod inventory;
mod sysmon;
mod util;
#[cfg(windows)]
mod win_activity;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "habits", version, about = "Local-first Windows activity monitor")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Display heartbeat and collection counters
    Status,
    /// Run foreground collector (normally use windowless habagent.exe)
    Run,
    /// Take a software inventory snapshot immediately
    Inventory,
    /// Export a self-contained, date-limited ZIP report
    Export {
        #[arg(long = "from", value_parser = parse_date)]
        from: NaiveDate,
        #[arg(long = "to", value_parser = parse_date)]
        to: NaiveDate,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

fn parse_date(s: &str) -> std::result::Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| format!("expected YYYY-MM-DD: {e}"))
}

pub fn cli_main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Status => db::show_status(&util::database_path()?),
        Commands::Run => run_agent(),
        Commands::Inventory => {
            #[cfg(windows)]
            {
                let mut db = db::open(&util::database_path()?)?;
                let result = inventory::scan_and_update(&mut db)?;
                println!("Inventory complete: {result} changes");
                Ok(())
            }
            #[cfg(not(windows))]
            anyhow::bail!("Inventory collection requires Windows")
        }
        Commands::Export { from, to, output } => {
            let input = util::database_path()?;
            let dest = output.unwrap_or_else(|| {
                util::data_dir().unwrap_or_else(|_| PathBuf::from("."))
                    .join("exports")
                    .join(format!("habits-{from}_to_{to}.zip"))
            });
            export::export_zip(&input, &dest, from, to)
                .with_context(|| format!("exporting {}", dest.display()))?;
            println!("{}", dest.display());
            Ok(())
        }
    }
}

pub fn run_agent() -> Result<()> {
    #[cfg(windows)]
    { agent::run() }
    #[cfg(not(windows))]
    { anyhow::bail!("The Windows agent requires Windows") }
}

pub fn write_fatal_log(message: &str) {
    let line = format!("{} FATAL {}\n", chrono::Utc::now().to_rfc3339(), message);
    if let Ok(dir) = util::data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("agent-errors.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(log) {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
        }
    }
}
