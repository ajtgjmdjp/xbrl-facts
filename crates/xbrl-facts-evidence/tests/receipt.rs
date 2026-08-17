//! Evidence receipt M1: build from a parsed fact, verify deterministically.

use xbrl_facts_core::parse_instance;
use xbrl_facts_evidence::{
    CheckName, ClaimKind, SourceArtifact, ValidationStatus, build_receipts, sha256_hex, verify,
};

const INSTANCE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbrli:xbrl xmlns:xbrli="http://www.xbrl.org/2003/instance"
            xmlns:jppfs_cor="http://disclosure.edinet-fsa.go.jp/taxonomy/jppfs/2023-11-01/jppfs_cor">
  <xbrli:context id="CurrentYearDuration">
    <xbrli:entity><xbrli:identifier scheme="http://disclosure.edinet-fsa.go.jp">E02144-000</xbrli:identifier></xbrli:entity>
    <xbrli:period><xbrli:startDate>2025-04-01</xbrli:startDate><xbrli:endDate>2026-03-31</xbrli:endDate></xbrli:period>
  </xbrli:context>
  <xbrli:unit id="JPY"><xbrli:measure>iso4217:JPY</xbrli:measure></xbrli:unit>
  <jppfs_cor:NetSales contextRef="CurrentYearDuration" unitRef="JPY" decimals="-6">50684952000000</jppfs_cor:NetSales>
</xbrli:xbrl>
"#;

fn artifact(bytes: &[u8]) -> SourceArtifact {
    SourceArtifact {
        uri: "edinet://S100Y8NY/test.xbrl".into(),
        sha256: sha256_hex(bytes),
        retrieved_at: Some("2026-08-18T00:00:00Z".into()),
        authority: Some("jp.fsa.edinet".into()),
    }
}

#[test]
fn builds_receipt_for_numeric_fact() {
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    assert_eq!(receipts.len(), 1);
    let r = &receipts[0];
    assert!(r.receipt_id.starts_with("er_"));
    assert_eq!(r.claim.kind, ClaimKind::Stated);
    assert_eq!(r.claim.value, "50684952000000");
    assert_eq!(r.evidence[0].locator.xbrl().concept, "jppfs_cor:NetSales");
    assert_eq!(
        r.evidence[0].locator.xbrl().context_ref,
        "CurrentYearDuration"
    );
    assert!(r.evidence[0].locator.xbrl().byte_range.is_some());
}

#[test]
fn receipt_id_is_deterministic() {
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let a = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    let b = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    assert_eq!(a[0].receipt_id, b[0].receipt_id);
}

#[test]
fn verify_passes_on_untampered_source() {
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    let report = verify(&receipts[0], bytes);
    assert_eq!(report.status, ValidationStatus::Verified, "{report:?}");
    assert!(report.checks.iter().all(|c| c.pass));
}

#[test]
fn verify_fails_when_source_tampered() {
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    // 原本の数字を1桁改竄
    let tampered = INSTANCE.replace("50684952000000", "50684952000001");
    let report = verify(&receipts[0], tampered.as_bytes());
    assert_eq!(report.status, ValidationStatus::Failed);
    let hash = report
        .checks
        .iter()
        .find(|c| c.name == CheckName::ArtifactHash)
        .unwrap();
    assert!(!hash.pass);
    let val = report
        .checks
        .iter()
        .find(|c| c.name == CheckName::ValueMatch)
        .unwrap();
    assert!(!val.pass, "改竄後の値は claim と一致してはならない");
}

#[test]
fn verify_fails_when_claim_tampered() {
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let mut receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    receipts[0].claim.value = "99999999999999".into();
    let report = verify(&receipts[0], bytes);
    assert_eq!(report.status, ValidationStatus::Failed);
    let hash = report
        .checks
        .iter()
        .find(|c| c.name == CheckName::ArtifactHash)
        .unwrap();
    assert!(hash.pass, "原本は無傷なのでハッシュは通る");
    let val = report
        .checks
        .iter()
        .find(|c| c.name == CheckName::ValueMatch)
        .unwrap();
    assert!(!val.pass);
}

#[test]
fn verify_fails_when_fact_not_locatable() {
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let mut receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    receipts[0].evidence[0].locator.xbrl_mut().context_ref = "NoSuchContext".into();
    let report = verify(&receipts[0], bytes);
    assert_eq!(report.status, ValidationStatus::Failed);
    let loc = report
        .checks
        .iter()
        .find(|c| c.name == CheckName::Locate)
        .unwrap();
    assert!(!loc.pass);
}

#[test]
fn receipt_serializes_roundtrip() {
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    let json = serde_json::to_string_pretty(&receipts[0]).unwrap();
    let back: xbrl_facts_evidence::Receipt = serde_json::from_str(&json).unwrap();
    assert_eq!(receipts[0], back);
}

#[test]
fn duplicate_facts_disambiguate_by_byte_range() {
    // 同一 (concept, context, unit) の fact が2回出現するのは XBRL では普通
    // (iXBRL の本文と表で同じ値を2箇所マークアップ等)。byte_range で一意化する。
    let dup = INSTANCE.replace(
        "</xbrli:xbrl>",
        r#"<jppfs_cor:NetSales contextRef="CurrentYearDuration" unitRef="JPY" decimals="-6">50684952000000</jppfs_cor:NetSales>
</xbrli:xbrl>"#,
    );
    let bytes = dup.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    assert_eq!(receipts.len(), 2);
    for r in &receipts {
        let report = verify(r, bytes);
        assert_eq!(report.status, ValidationStatus::Verified, "{report:?}");
    }
}

#[test]
fn tampered_receipt_id_binding_fails() {
    // claim を別の正当な値に書き換えても receipt_id が旧いままなら FAIL
    // (原本上は成立する claim へのすり替え攻撃を receipt_id 検証で塞ぐ)
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let mut receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    receipts[0].claim.doc_id = "S999FORGED".into();
    let report = verify(&receipts[0], bytes);
    assert_eq!(report.status, ValidationStatus::Failed);
    let id = report
        .checks
        .iter()
        .find(|c| c.name == CheckName::ReceiptId)
        .unwrap();
    assert!(!id.pass);
}

#[test]
fn empty_evidence_fails_loudly_not_panics() {
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let mut receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    receipts[0].evidence.clear();
    let report = verify(&receipts[0], bytes);
    assert_eq!(report.status, ValidationStatus::Failed);
    let shape = report
        .checks
        .iter()
        .find(|c| c.name == CheckName::EvidenceShape)
        .unwrap();
    assert!(!shape.pass);
}

#[test]
fn trailing_forged_evidence_fails() {
    // M1 は evidence ちょうど1件のみ許可 — 2件目の紛れ込みは形状違反
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let mut receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    let dup = receipts[0].evidence[0].clone();
    receipts[0].evidence.push(dup);
    let report = verify(&receipts[0], bytes);
    assert_eq!(report.status, ValidationStatus::Failed);
}

#[test]
fn locator_serializes_with_profile_tag() {
    let bytes = INSTANCE.as_bytes();
    let doc = parse_instance(bytes).unwrap();
    let receipts = build_receipts(&doc, "S100Y8NY", &artifact(bytes)).unwrap();
    let json = serde_json::to_value(&receipts[0]).unwrap();
    assert_eq!(json["schema"], "er/0.1");
    assert_eq!(json["evidence"][0]["locator"]["profile"], "xbrl");
    assert!(
        receipts[0].receipt_id.len() > 20,
        "full digest, not truncated"
    );
}
