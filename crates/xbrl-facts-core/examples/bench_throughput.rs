//! Throughput benchmark: parse a real SEC 10-K + a real EDINET IXDS
//! repeatedly to measure pure parser throughput without process-spawn
//! overhead. Run with `cargo run --release --example bench_throughput`.

use std::path::PathBuf;
use std::time::Instant;

use xbrl_facts_core::{parse_instance, parse_instance_set};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_file(rel: &str) -> Vec<u8> {
    let path = workspace_root().join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn read_dir(rel: &str) -> Vec<Vec<u8>> {
    let dir = workspace_root().join(rel);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
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

fn bench<F: FnMut() -> Result<usize, Box<dyn std::error::Error>>>(
    label: &str,
    bytes_per_iter: usize,
    iters: usize,
    mut f: F,
) {
    // Warm-up.
    let _ = f().unwrap();

    let start = Instant::now();
    let mut total_facts = 0usize;
    for _ in 0..iters {
        total_facts += f().unwrap();
    }
    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();
    let mb = (bytes_per_iter as f64 * iters as f64) / 1_048_576.0;
    println!(
        "{label}\n  iters: {iters}, total facts: {total_facts}\n  elapsed: {secs:.3}s, {:.1} MB processed\n  throughput: {:.1} MB/s, {:.0} files/s, {:.0} facts/s\n",
        mb,
        mb / secs,
        iters as f64 / secs,
        total_facts as f64 / secs,
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sec_bytes = read_file("tests/fixtures/sec/aapl-10k-fy25.htm");
    let edinet_inputs = read_dir("tests/fixtures/edinet/nippon-beet-sugar-fy2025");
    let edinet_total_bytes: usize = edinet_inputs.iter().map(|v| v.len()).sum();

    println!(
        "Fixtures: SEC Apple 10-K = {:.0} KB, EDINET IXDS = {} files = {:.0} KB\n",
        sec_bytes.len() as f64 / 1024.0,
        edinet_inputs.len(),
        edinet_total_bytes as f64 / 1024.0,
    );

    bench(
        "SEC 10-K (single file iXBRL, 1.5 MB)",
        sec_bytes.len(),
        100,
        || {
            let doc = parse_instance(&sec_bytes)?;
            Ok(doc.facts.len())
        },
    );

    bench(
        "EDINET 有報 (IXDS, 4 files)",
        edinet_total_bytes,
        500,
        || {
            let doc = parse_instance_set(edinet_inputs.iter().map(|v| v.as_slice()))?;
            Ok(doc.facts.len())
        },
    );

    Ok(())
}
