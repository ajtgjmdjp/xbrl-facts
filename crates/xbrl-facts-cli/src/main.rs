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
    /// Emit evidence receipts (one JSON line per numeric fact)
    Receipt {
        /// Path to an XBRL instance file
        path: PathBuf,

        /// Document id recorded in each receipt (e.g. EDINET docID)
        #[arg(long)]
        doc_id: String,

        /// Source URI recorded in the receipt
        #[arg(long, default_value = "")]
        uri: String,

        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Generate an ed25519 signing key (32-byte hex file)
    Keygen {
        /// Output key file
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Sign receipts with an ed25519 key (attestation over canonical body)
    Sign {
        /// Path to a receipts JSONL file
        receipts: PathBuf,

        /// Path to a 32-byte hex key file (see keygen)
        #[arg(long)]
        key: PathBuf,

        /// Output file (default: in-place)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Deterministically verify evidence receipts against source bytes
    Verify {
        /// Path to a receipts JSONL file (one receipt object per line)
        receipts: PathBuf,

        /// Path to the source XBRL instance the receipts cite
        #[arg(long)]
        source: PathBuf,
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
        Commands::Receipt {
            path,
            doc_id,
            uri,
            output,
        } => {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("failed to read input file {}", path.display()))?;
            let instance = parse_instance(&bytes)?;
            let artifact = xbrl_facts_evidence::SourceArtifact {
                uri: if uri.is_empty() {
                    path.display().to_string()
                } else {
                    uri
                },
                sha256: xbrl_facts_evidence::sha256_hex(&bytes),
                retrieved_at: None,
                authority: None,
            };
            let receipts = xbrl_facts_evidence::build_receipts(&instance, &doc_id, &artifact)?;
            let mut out = String::new();
            for r in &receipts {
                out.push_str(&serde_json::to_string(r)?);
                out.push('\n');
            }
            match output {
                Some(p) => std::fs::write(&p, out)
                    .with_context(|| format!("failed to write {}", p.display()))?,
                None => print!("{out}"),
            }
            eprintln!("{} receipts emitted", receipts.len());
        }
        Commands::Verify { receipts, source } => {
            let source_bytes = std::fs::read(&source)
                .with_context(|| format!("failed to read source {}", source.display()))?;
            let text = std::fs::read_to_string(&receipts)
                .with_context(|| format!("failed to read receipts {}", receipts.display()))?;
            let ctx = xbrl_facts_evidence::SourceContext::load(&source_bytes)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let all: Vec<xbrl_facts_evidence::Receipt> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(serde_json::from_str)
                .collect::<Result<_, _>>()?;
            let parents: std::collections::HashMap<String, &xbrl_facts_evidence::Receipt> =
                all.iter().map(|r| (r.receipt_id.clone(), r)).collect();

            let mut pass = 0usize;
            let mut fail = 0usize;
            let mut verified_ids = std::collections::HashSet::new();
            // Stated first, so derived receipts can require verified parents
            for receipt in all.iter().filter(|r| r.derivation.is_none()) {
                let report = xbrl_facts_evidence::verify_in(&ctx, receipt);
                if report.status == xbrl_facts_evidence::ValidationStatus::Verified {
                    pass += 1;
                    verified_ids.insert(receipt.receipt_id.clone());
                } else {
                    fail += 1;
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
            }
            for receipt in all.iter().filter(|r| r.derivation.is_some()) {
                let report = xbrl_facts_evidence::verify_derived(receipt, &parents);
                // Chain rule: a derived claim is only as good as its parents'
                // verification against the primary source.
                let chain_ok = receipt.derivation.as_ref().is_some_and(|d| {
                    d.inputs
                        .iter()
                        .all(|i| verified_ids.contains(&i.receipt_id))
                });
                if report.status == xbrl_facts_evidence::ValidationStatus::Verified && chain_ok {
                    pass += 1;
                    verified_ids.insert(receipt.receipt_id.clone());
                } else {
                    fail += 1;
                    if !chain_ok {
                        eprintln!("{}: parent(s) not source-verified", receipt.receipt_id);
                    }
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
            }
            eprintln!("verified: {pass} PASS, {fail} FAIL");
            if fail > 0 {
                std::process::exit(1);
            }
        }
        Commands::Keygen { output } => {
            let key = xbrl_facts_evidence::SigningKey::generate();
            std::fs::write(&output, hex::encode(key.to_bytes()))
                .with_context(|| format!("failed to write {}", output.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o600))?;
            }
            eprintln!("key written to {}", output.display());
        }
        Commands::Sign {
            receipts,
            key,
            output,
        } => {
            let hex_key = std::fs::read_to_string(&key)
                .with_context(|| format!("failed to read key {}", key.display()))?;
            let bytes: [u8; 32] = hex::decode(hex_key.trim())
                .map_err(|e| anyhow::anyhow!("bad key hex: {e}"))?
                .try_into()
                .map_err(|_| anyhow::anyhow!("key must be 32 bytes"))?;
            let signer = xbrl_facts_evidence::SigningKey::from_bytes(&bytes);
            let text = std::fs::read_to_string(&receipts)
                .with_context(|| format!("failed to read receipts {}", receipts.display()))?;
            let mut out = String::new();
            let mut n = 0usize;
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                let mut receipt: xbrl_facts_evidence::Receipt = serde_json::from_str(line)?;
                xbrl_facts_evidence::sign_receipt(&mut receipt, &signer)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                out.push_str(&serde_json::to_string(&receipt)?);
                out.push('\n');
                n += 1;
            }
            let dest = output.unwrap_or(receipts);
            std::fs::write(&dest, out)
                .with_context(|| format!("failed to write {}", dest.display()))?;
            eprintln!("{n} receipts signed -> {}", dest.display());
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
