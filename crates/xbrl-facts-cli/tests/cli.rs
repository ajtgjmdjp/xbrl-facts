use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn parse_jsonl_and_inspect_concept() {
    let output = Command::new(env!("CARGO_BIN_EXE_xbrl-facts"))
        .args([
            "parse",
            fixture("minimal.xbrl").to_str().unwrap(),
            "--format",
            "jsonl",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let jsonl = String::from_utf8(output.stdout).unwrap();
    assert!(jsonl.contains("NetSales"));
    assert!(jsonl.contains("CompanyName"));

    let path = std::env::temp_dir().join("xbrl-facts-cli-test.jsonl");
    std::fs::write(&path, jsonl).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_xbrl-facts"))
        .args(["inspect", path.to_str().unwrap(), "--concept", "NetSales"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("NetSales"));
    assert!(!stdout.contains("CompanyName"));
}

#[test]
fn parse_normalized_jsonl() {
    let output = Command::new(env!("CARGO_BIN_EXE_xbrl-facts"))
        .args([
            "parse",
            fixture("advanced.xbrl").to_str().unwrap(),
            "--format",
            "jsonl",
            "--facts",
            "normalized",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("EarningsPerShare"));
    assert!(stdout.contains("StoreAxis"));
    assert!(stdout.contains("JPYPerShare"));
    assert!(stdout.contains("\"type\":\"nil\""));
}

#[test]
fn parse_inline_xhtml_normalized_jsonl() {
    let output = Command::new(env!("CARGO_BIN_EXE_xbrl-facts"))
        .args([
            "parse",
            fixture("inline.xhtml").to_str().unwrap(),
            "--format",
            "jsonl",
            "--facts",
            "normalized",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("HiddenLoss"));
    assert!(stdout.contains("\"decimal\":\"-42\""));
    assert!(stdout.contains("Revenue"));
    assert!(stdout.contains("\"decimal\":\"1234000\""));
    assert!(stdout.contains("EuAmount"));
    assert!(stdout.contains("\"decimal\":\"1234.56\""));
    assert!(stdout.contains("DashAmount"));
    assert!(stdout.contains("\"decimal\":\"0\""));
    assert!(stdout.contains("ParenthesizedLoss"));
    assert!(stdout.contains("\"decimal\":\"-1234\""));
    assert!(stdout.contains("CompanyName"));
}

#[test]
fn parse_json_with_footnotes() {
    let output = Command::new(env!("CARGO_BIN_EXE_xbrl-facts"))
        .args([
            "parse",
            fixture("footnote.xbrl").to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Includes overseas sales."));
    assert!(stdout.contains("\"fact_refs\": ["));
    assert!(stdout.contains("\"f1\""));
}
