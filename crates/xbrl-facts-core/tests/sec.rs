//! Integration tests against a real SEC EDGAR Inline XBRL filing.
//!
//! Fixture is Apple Inc.'s 10-K filing for fiscal year 2025, accession
//! 0000320193-25-000079, filed 2025-10-31. SEC EDGAR filings are in the
//! public domain.

use std::path::PathBuf;

use xbrl_facts_core::{NormalizedValue, QName, TaxonomyResolver, normalize_facts, parse_instance};

struct NoLabels;

impl TaxonomyResolver for NoLabels {
    fn label(&self, _name: &QName, _role: Option<&str>, _lang: Option<&str>) -> Option<String> {
        None
    }
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sec/aapl-10k-fy25.htm")
}

#[test]
fn parses_full_apple_10k_without_dropping_facts() {
    let bytes = std::fs::read(fixture_path()).expect("fixture");
    let instance = parse_instance(&bytes).expect("parse");

    // Source contains 1131 ix:nonFraction + ix:nonNumeric elements. The
    // ix:continuation refactor (2026-06-06) was specifically motivated by
    // this filing, so guard against regressing back into reading
    // continuation contents as opaque text.
    assert!(
        instance.facts.len() >= 1100,
        "expected ~1131 facts, got {} (continuation handling may have regressed)",
        instance.facts.len()
    );

    // FY25 total revenue ($416.161B) is reported inside an ix:nonNumeric
    // text block via ix:continuation — historically dropped before the fix.
    let total_revenue: Vec<_> = instance
        .facts
        .iter()
        .filter(|f| f.name.local_name == "RevenueFromContractWithCustomerExcludingAssessedTax")
        .collect();
    assert!(
        total_revenue.len() >= 9,
        "expected at least 9 revenue facts (3 years × products/services/total), got {}",
        total_revenue.len()
    );
}

#[test]
fn normalizes_apple_revenue_to_correct_decimal() {
    let bytes = std::fs::read(fixture_path()).unwrap();
    let instance = parse_instance(&bytes).unwrap();

    let normalized: Vec<_> = normalize_facts(&instance, &NoLabels, "aapl-10k-fy25")
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("all normalize");

    // Apple FY25 total revenue: $416.161B (raw "416,161" with scale=6 and decimals=-6).
    let fy25_total = normalized.iter().find(|f| {
        f.name.local_name == "RevenueFromContractWithCustomerExcludingAssessedTax"
            && matches!(&f.value, NormalizedValue::Numeric { raw, .. } if raw == "416,161")
    });
    let fact = fy25_total.expect("FY25 total revenue present");
    if let NormalizedValue::Numeric { decimal, .. } = &fact.value {
        let d = decimal.as_ref().expect("decimal parsed");
        assert_eq!(
            d.to_string(),
            "416161000000",
            "scale=6 not applied correctly"
        );
    } else {
        panic!("expected Numeric");
    }
}

#[test]
fn handles_sec_numwordsen_transform() {
    let bytes = std::fs::read(fixture_path()).unwrap();
    let instance = parse_instance(&bytes).unwrap();
    let normalized: Vec<_> = normalize_facts(&instance, &NoLabels, "aapl-10k-fy25")
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("numwordsen-formatted facts normalize cleanly");

    // The 10-K reports "NumberOfCustomersWithSignificantAccountsReceivableBalance"
    // as "one" with format="ixt-sec:numwordsen". Before the transform was
    // implemented, the run-wide normalize would bail with InvalidDecimal.
    let word_fact = normalized
        .iter()
        .find(|f| f.name.local_name == "NumberOfCustomersWithSignificantAccountsReceivableBalance")
        .expect("word-numbered fact present");
    if let NormalizedValue::Numeric { decimal, .. } = &word_fact.value {
        assert_eq!(decimal.as_ref().unwrap().to_string(), "1");
    } else {
        panic!("expected Numeric");
    }
}
