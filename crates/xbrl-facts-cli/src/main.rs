use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use xbrl_facts_core::{QName, RawFact, TaxonomyResolver, normalize_facts, parse_instance};

#[derive(Parser)]
#[command(
    name = "xbrl-facts",
    version,
    about = "Parse and inspect XBRL financial filings"
)]
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

        /// Fact output mode for JSONL output
        #[arg(long, default_value = "raw")]
        facts: FactOutput,
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

#[derive(Clone, clap::ValueEnum)]
enum FactOutput {
    Raw,
    Normalized,
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
            format,
            facts,
        } => {
            let input = std::fs::read(&path)
                .with_context(|| format!("failed to read input file {}", path.display()))?;
            let instance = parse_instance(&input)?;
            let rendered = match format {
                OutputFormat::Json => serde_json::to_string_pretty(&instance)?,
                OutputFormat::Jsonl => match facts {
                    FactOutput::Raw => instance
                        .facts
                        .iter()
                        .map(serde_json::to_string)
                        .collect::<Result<Vec<_>, _>>()?
                        .join("\n"),
                    FactOutput::Normalized => normalize_facts(&instance, &NoLabels, "stdin")
                        .into_iter()
                        .map(|fact| -> anyhow::Result<String> {
                            Ok(serde_json::to_string(&fact?)?)
                        })
                        .collect::<anyhow::Result<Vec<_>>>()?
                        .join("\n"),
                },
            };

            if let Some(output) = output {
                std::fs::write(&output, rendered)
                    .with_context(|| format!("failed to write output file {}", output.display()))?;
            } else {
                println!("{rendered}");
            }
        }
        Commands::Inspect { path, concept } => {
            let input = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read JSONL file {}", path.display()))?;
            for (line_no, line) in input.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let fact: RawFact = serde_json::from_str(line).with_context(|| {
                    format!("invalid JSONL at {}:{}", path.display(), line_no + 1)
                })?;
                if concept
                    .as_ref()
                    .is_some_and(|name| fact.name.local_name != *name)
                {
                    continue;
                }
                println!("{}", serde_json::to_string(&fact)?);
            }
        }
    }

    Ok(())
}

struct NoLabels;

impl TaxonomyResolver for NoLabels {
    fn label(&self, _name: &QName, _role: Option<&str>, _lang: Option<&str>) -> Option<String> {
        None
    }
}
