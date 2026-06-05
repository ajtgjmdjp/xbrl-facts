use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use xbrl_facts_core::{
    LabelLinkbase, QName, RawFact, SchemaIndex, TaxonomyResolver, normalize_facts, parse_instance,
    parse_instance_set,
};

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
    /// Parse an XBRL/iXBRL file or directory and output facts.
    ///
    /// If `path` is a directory, every `.htm`/`.xhtml`/`.xbrl` file inside is
    /// merged as one Inline XBRL Document Set (IXDS).
    Parse {
        /// Path to XBRL/iXBRL file, or directory containing an IXDS
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

        /// Taxonomy schema file (.xsd). May be repeated. Required for label
        /// resolution; without it `--labels` cannot map fragments to QNames.
        #[arg(long = "schema", value_name = "FILE", action = clap::ArgAction::Append)]
        schemas: Vec<PathBuf>,

        /// Label linkbase file (.xml). May be repeated.
        #[arg(long = "labels", value_name = "FILE", action = clap::ArgAction::Append)]
        labels: Vec<PathBuf>,

        /// Preferred language for label lookup (e.g. "ja", "en")
        #[arg(long = "lang", default_value = "ja")]
        lang: String,
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
            schemas,
            labels,
            lang,
        } => {
            let instance = if path.is_dir() {
                let inputs = collect_ixds_inputs(&path)?;
                if inputs.is_empty() {
                    anyhow::bail!("no XBRL/iXBRL files found in {}", path.display());
                }
                parse_instance_set(inputs.iter().map(|b| b.as_slice()))?
            } else {
                let input = std::fs::read(&path)
                    .with_context(|| format!("failed to read input file {}", path.display()))?;
                parse_instance(&input)?
            };

            let taxonomy: Box<dyn TaxonomyResolver> = if labels.is_empty() {
                Box::new(NoLabels)
            } else {
                let mut schema = SchemaIndex::new();
                for path in &schemas {
                    let bytes = std::fs::read(path)
                        .with_context(|| format!("failed to read schema {}", path.display()))?;
                    let href = path
                        .file_name()
                        .and_then(|f| f.to_str())
                        .unwrap_or_default();
                    schema.ingest_schema(href, &bytes)?;
                }
                let mut linkbase = LabelLinkbase::new();
                for path in &labels {
                    let bytes = std::fs::read(path).with_context(|| {
                        format!("failed to read label linkbase {}", path.display())
                    })?;
                    linkbase.ingest(&bytes, &schema)?;
                }
                Box::new(LangPreferringResolver {
                    linkbase,
                    lang: lang.clone(),
                })
            };

            let rendered = match format {
                OutputFormat::Json => serde_json::to_string_pretty(&instance)?,
                OutputFormat::Jsonl => match facts {
                    FactOutput::Raw => instance
                        .facts
                        .iter()
                        .map(serde_json::to_string)
                        .collect::<Result<Vec<_>, _>>()?
                        .join("\n"),
                    FactOutput::Normalized => {
                        normalize_facts(&instance, taxonomy.as_ref(), "stdin")
                            .into_iter()
                            .map(|fact| -> anyhow::Result<String> {
                                Ok(serde_json::to_string(&fact?)?)
                            })
                            .collect::<anyhow::Result<Vec<_>>>()?
                            .join("\n")
                    }
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

fn collect_ixds_inputs(dir: &std::path::Path) -> anyhow::Result<Vec<Vec<u8>>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| matches!(e.to_ascii_lowercase().as_str(), "htm" | "xhtml" | "xbrl"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| std::fs::read(&p).with_context(|| format!("failed to read {}", p.display())))
        .collect()
}

struct LangPreferringResolver {
    linkbase: LabelLinkbase,
    lang: String,
}

impl TaxonomyResolver for LangPreferringResolver {
    fn label(&self, name: &QName, role: Option<&str>, lang: Option<&str>) -> Option<String> {
        let preferred = lang.unwrap_or(&self.lang);
        self.linkbase.label(name, role, Some(preferred))
    }
}

struct NoLabels;

impl TaxonomyResolver for NoLabels {
    fn label(&self, _name: &QName, _role: Option<&str>, _lang: Option<&str>) -> Option<String> {
        None
    }
}
