mod show;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tabl_engine::io::LoadOptions;

#[derive(Parser)]
#[command(
    name = "tabl",
    about = "A terminal spreadsheet for data files",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Data file to open in the viewer (parquet, csv, json, ndjson).
    file: Option<PathBuf>,

    /// Infer date/datetime columns when reading CSV.
    #[arg(short = 'd', long = "parse-dates")]
    parse_dates: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Convert a data file from one format to another (headless).
    Convert {
        input: PathBuf,
        output: PathBuf,

        /// Infer date/datetime columns when reading CSV.
        #[arg(short = 'd', long = "parse-dates")]
        parse_dates: bool,
    },

    /// Print rows from a file without opening the TUI.
    Show {
        /// File to read.
        file: PathBuf,

        /// Show the first N rows.
        #[arg(short = 'H', long)]
        head: Option<usize>,

        /// Show the last N rows.
        #[arg(short = 'T', long)]
        tail: Option<usize>,

        /// Infer date/datetime columns when reading CSV.
        #[arg(short = 'd', long = "parse-dates")]
        parse_dates: bool,
    },

    /// Print summary statistics for a file (like polars' describe).
    Describe {
        file: PathBuf,

        /// Infer date/datetime columns when reading CSV.
        #[arg(short = 'd', long = "parse-dates")]
        parse_dates: bool,
    },
}

fn main() -> Result<()> {
    let Cli {
        file,
        parse_dates,
        command,
    } = Cli::parse();

    match command {
        Some(Command::Convert {
            input,
            output,
            parse_dates,
        }) => {
            let out_fmt = tabl_engine::Format::from_path(&output)?;
            tabl_engine::convert::convert(&input, &output, out_fmt, LoadOptions { parse_dates })?;
        }
        Some(Command::Show {
            file,
            head,
            tail,
            parse_dates,
        }) => {
            // Fall back to head 10 only when neither flag is given — `--tail`
            // alone must not also pull in a head, so this can't be a plain
            // clap default on `head`.
            let (head, tail) = match (head, tail) {
                (None, None) => (Some(10), None),
                provided => provided,
            };
            show::run(&file, head, tail, LoadOptions { parse_dates })?;
        }
        Some(Command::Describe { file, parse_dates }) => {
            let sheet = tabl_engine::io::load_with(&file, LoadOptions { parse_dates })?;
            show::print_sheet(&sheet.describe()?);
        }
        None => match file {
            Some(file) => {
                let sheet = tabl_engine::io::load_with(&file, LoadOptions { parse_dates })?;
                tabl_tui::run(sheet, file)?;
            }
            None => {
                eprintln!(
                    "usage: tabl <FILE>  |  tabl convert <IN> <OUT>  |  tabl show <FILE>  |  tabl describe <FILE>"
                );
            }
        },
    }

    Ok(())
}
