use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xbrl-facts", version, about = "Parse and inspect XBRL financial filings")]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse an XBRL/iXBRL file and output facts
    Parse {
        /// Path to XBRL or iXBRL file
        path: PathBuf,

        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Output format
        #[arg(short, long, default_value = "jsonl")]
        format: OutputFormat,
    },
    /// Inspect parsed JSONL facts
    Inspect {
        /// Path to JSONL file
        path: PathBuf,

        /// Filter by concept local name
        #[arg(long)]
        concept: Option<String>,
    },
}

#[derive(Clone, clap::ValueEnum)]
enum OutputFormat {
    Jsonl,
    Json,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.verbose {
        eprintln!("verbose mode enabled");
    }

    match cli.command {
        Commands::Parse {
            path,
            output,
            format: _,
        } => {
            let dest = output
                .as_ref()
                .map_or("stdout".to_string(), |p| p.display().to_string());
            eprintln!("TODO: parse {} -> {dest}", path.display());
        }
        Commands::Inspect { path, concept } => {
            eprintln!("TODO: inspect {} concept={concept:?}", path.display());
        }
    }

    Ok(())
}
