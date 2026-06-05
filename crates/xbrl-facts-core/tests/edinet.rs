//! Integration tests against a real EDINET filing.
//!
//! Fixture is a redacted subset of the FY2025 annual report from
//! Nippon Beet Sugar Manufacturing Co., Ltd. (EDINET code E00355) filed
//! on 2025-06-26. EDINET disclosure data is in the public domain.

use std::path::PathBuf;

use xbrl_facts_core::{
    LabelLinkbase, NormalizedValue, QName, SchemaIndex, TaxonomyResolver, normalize_facts,
    parse_instance_set,
};

struct NoLabels;

impl TaxonomyResolver for NoLabels {
    fn label(&self, _name: &QName, _role: Option<&str>, _lang: Option<&str>) -> Option<String> {
        None
    }
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/edinet/nippon-beet-sugar-fy2025")
}

fn read_ixds() -> Vec<Vec<u8>> {
    let dir = fixture_dir();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {}", dir.display(), e))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("htm"))
        .collect();
    entries.sort();
    entries
        .into_iter()
        .map(|p| std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {}", p.display(), e)))
        .collect()
}

#[test]
fn parses_edinet_ixds_without_errors() {
    let inputs = read_ixds();
    assert!(inputs.len() >= 3, "expected header + at least 2 honbun");
    let instance =
        parse_instance_set(inputs.iter().map(|b| b.as_slice())).expect("IXDS parse should succeed");

    assert!(!instance.contexts.is_empty(), "expected contexts");
    assert!(!instance.units.is_empty(), "expected units");
    assert!(!instance.facts.is_empty(), "expected facts");

    let filing_date_ctx = instance
        .contexts
        .get("FilingDateInstant")
        .expect("FilingDateInstant context shared via IXDS header");
    assert_eq!(filing_date_ctx.entity.identifier, "E00355-000");

    let jpy = instance
        .units
        .get("JPY")
        .expect("JPY unit defined in header");
    assert_eq!(jpy.numerator[0].local_name, "JPY");
}

#[test]
fn resolves_filer_specific_labels_from_linkbase() {
    let dir = fixture_dir();
    let xsd = dir.join("jpcrp030000-asr-001_E00355-000_2025-03-31_01_2025-06-26.xsd");
    let lab = dir.join("jpcrp030000-asr-001_E00355-000_2025-03-31_01_2025-06-26_lab.xml");

    let mut schema = SchemaIndex::new();
    schema
        .ingest_schema(
            xsd.file_name().unwrap().to_str().unwrap(),
            &std::fs::read(&xsd).expect("schema fixture"),
        )
        .expect("schema parses");

    let mut linkbase = LabelLinkbase::new();
    linkbase
        .ingest(&std::fs::read(&lab).expect("lab fixture"), &schema)
        .expect("linkbase parses");

    // Filer-specific concept that has a Japanese label in the linkbase.
    let qname = QName {
        namespace_uri: Some(
            "http://disclosure.edinet-fsa.go.jp/jpcrp030000/asr/001/E00355-000/2025-03-31/01/2025-06-26"
                .into(),
        ),
        prefix: Some("jpcrp030000-asr_E00355-000".into()),
        local_name: "IdleAssetExpensesNOE".into(),
    };
    assert_eq!(
        linkbase.label(&qname, None, Some("ja")).as_deref(),
        Some("遊休資産諸費用")
    );
}

#[test]
fn normalizes_known_edinet_concepts() {
    let inputs = read_ixds();
    let instance = parse_instance_set(inputs.iter().map(|b| b.as_slice())).unwrap();
    let normalized: Vec<_> = normalize_facts(&instance, &NoLabels, "E00355-000_2025-03-31")
        .into_iter()
        .collect::<Result<_, _>>()
        .expect("all known concepts normalize cleanly");

    let net_sales: Vec<_> = normalized
        .iter()
        .filter(|f| f.name.local_name == "NetSalesSummaryOfBusinessResults")
        .collect();
    assert!(
        net_sales.len() >= 3,
        "expected 5-year history (got {})",
        net_sales.len()
    );

    for fact in net_sales {
        match &fact.value {
            NormalizedValue::Numeric { decimal, .. } => {
                let d = decimal.as_ref().expect("decimal parsed");
                assert!(
                    *d > rust_decimal::Decimal::ZERO,
                    "net sales should be positive"
                );
            }
            other => panic!("expected numeric, got {other:?}"),
        }
    }
}
