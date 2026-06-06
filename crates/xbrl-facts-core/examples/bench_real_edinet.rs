//! Sequentially parse every EDINET IXDS in a directory.
//!
//! Invoked from `/tmp/edinet-extracted/<docid>/XBRL/PublicDoc` style trees:
//!
//!   cargo run --release --example bench_real_edinet -- /tmp/edinet-extracted
//!
//! Reports per-filing facts/timing and an aggregate throughput line.

use std::path::{Path, PathBuf};
use std::time::Instant;

use xbrl_facts_core::parse_instance_set;

fn collect_ixds_dir(filing_root: &Path) -> Option<PathBuf> {
    // Standard EDINET zip layout: <docid>/XBRL/PublicDoc/*.htm
    let candidate = filing_root.join("XBRL").join("PublicDoc");
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

fn read_htm_files(dir: &Path) -> Vec<Vec<u8>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {}", dir.display(), e))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("htm"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| std::fs::read(p).unwrap())
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/edinet-extracted".to_owned());
    let root = PathBuf::from(root);

    let mut filings: Vec<PathBuf> = std::fs::read_dir(&root)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    filings.sort();

    println!(
        "Scanning {} filings under {}\n",
        filings.len(),
        root.display()
    );

    let mut total_bytes = 0usize;
    let mut total_facts = 0usize;
    let mut succeeded = 0usize;
    let mut failed = 0usize;

    let global_start = Instant::now();

    for filing in &filings {
        let Some(ixds_dir) = collect_ixds_dir(filing) else {
            println!("skip {}: no XBRL/PublicDoc dir", filing.display());
            continue;
        };
        let inputs = read_htm_files(&ixds_dir);
        let bytes: usize = inputs.iter().map(|v| v.len()).sum();
        if inputs.is_empty() {
            println!("skip {}: no .htm files", filing.display());
            continue;
        }

        let start = Instant::now();
        match parse_instance_set(inputs.iter().map(|v| v.as_slice())) {
            Ok(doc) => {
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                println!(
                    "OK  {:>10}  {:>4} files {:>5} KB  {:>6} facts  {:>6.1} ms",
                    filing.file_name().unwrap().to_string_lossy(),
                    inputs.len(),
                    bytes / 1024,
                    doc.facts.len(),
                    elapsed_ms,
                );
                total_bytes += bytes;
                total_facts += doc.facts.len();
                succeeded += 1;
            }
            Err(e) => {
                println!("FAIL {}: {e}", filing.display());
                failed += 1;
            }
        }
    }

    let elapsed = global_start.elapsed().as_secs_f64();
    println!(
        "\nSummary: {succeeded} OK, {failed} failed, {total_facts} facts in {elapsed:.3}s\n  {:.1} MB processed, {:.1} MB/s, {:.0} filings/s",
        total_bytes as f64 / 1_048_576.0,
        (total_bytes as f64 / 1_048_576.0) / elapsed,
        succeeded as f64 / elapsed,
    );

    Ok(())
}
