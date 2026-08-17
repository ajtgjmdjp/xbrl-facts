//! M2: derived claims with first-class lineage, and signing.

use std::collections::HashMap;

use xbrl_facts_core::parse_instance;
use xbrl_facts_evidence::{
    CheckName, ClaimKind, Operation, Receipt, SigningKey, SourceArtifact, ValidationStatus,
    build_derived_receipt, build_receipts, sha256_hex, sign_receipt, verify, verify_derived,
};

const INSTANCE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbrli:xbrl xmlns:xbrli="http://www.xbrl.org/2003/instance"
            xmlns:jppfs_cor="http://disclosure.edinet-fsa.go.jp/taxonomy/jppfs/2023-11-01/jppfs_cor">
  <xbrli:context id="Cur">
    <xbrli:entity><xbrli:identifier scheme="http://disclosure.edinet-fsa.go.jp">E00001-000</xbrli:identifier></xbrli:entity>
    <xbrli:period><xbrli:startDate>2025-04-01</xbrli:startDate><xbrli:endDate>2026-03-31</xbrli:endDate></xbrli:period>
  </xbrli:context>
  <xbrli:unit id="JPY"><xbrli:measure>iso4217:JPY</xbrli:measure></xbrli:unit>
  <jppfs_cor:NetSales contextRef="Cur" unitRef="JPY" decimals="-6">200</jppfs_cor:NetSales>
  <jppfs_cor:OperatingIncome contextRef="Cur" unitRef="JPY" decimals="-6">50</jppfs_cor:OperatingIncome>
</xbrli:xbrl>
"#;

fn stated(bytes: &[u8]) -> Vec<Receipt> {
    let doc = parse_instance(bytes).unwrap();
    let artifact = SourceArtifact {
        uri: "test://doc".into(),
        sha256: sha256_hex(bytes),
        retrieved_at: None,
        authority: None,
    };
    build_receipts(&doc, "DOC1", &artifact).unwrap()
}

fn parents_map(rs: &[Receipt]) -> HashMap<String, &Receipt> {
    rs.iter().map(|r| (r.receipt_id.clone(), r)).collect()
}

#[test]
fn derived_ratio_builds_and_verifies() {
    let bytes = INSTANCE.as_bytes();
    let rs = stated(bytes);
    assert_eq!(rs.len(), 2);
    let (sales, oi) = (&rs[0], &rs[1]);
    // 営業利益率 = 50/200 = 0.25
    let derived = build_derived_receipt(
        Operation::Ratio { round_dp: Some(4) },
        &[(oi, "numerator"), (sales, "denominator")],
        "DOC1",
    )
    .unwrap();
    assert_eq!(derived.claim.kind, ClaimKind::Derived);
    assert_eq!(derived.claim.value, "0.25");
    assert!(
        derived.evidence.is_empty(),
        "lineage is first-class, not evidence"
    );
    let report = verify_derived(&derived, &parents_map(&rs));
    assert_eq!(report.status, ValidationStatus::Verified, "{report:?}");
}

#[test]
fn derived_fails_when_claim_tampered() {
    let bytes = INSTANCE.as_bytes();
    let rs = stated(bytes);
    let mut derived = build_derived_receipt(
        Operation::Ratio { round_dp: Some(4) },
        &[(&rs[1], "numerator"), (&rs[0], "denominator")],
        "DOC1",
    )
    .unwrap();
    derived.claim.value = "0.99".into();
    let report = verify_derived(&derived, &parents_map(&rs));
    assert_eq!(report.status, ValidationStatus::Failed);
    // ReceiptId と Recompute の両方が落ちる
    assert!(
        !report
            .checks
            .iter()
            .find(|c| c.name == CheckName::ReceiptId)
            .unwrap()
            .pass
    );
}

#[test]
fn derived_fails_when_parent_missing() {
    let bytes = INSTANCE.as_bytes();
    let rs = stated(bytes);
    let derived = build_derived_receipt(
        Operation::Ratio { round_dp: Some(4) },
        &[(&rs[1], "numerator"), (&rs[0], "denominator")],
        "DOC1",
    )
    .unwrap();
    let only_one = parents_map(&rs[..1]);
    let report = verify_derived(&derived, &only_one);
    assert_eq!(report.status, ValidationStatus::Failed);
    let res = report
        .checks
        .iter()
        .find(|c| c.name == CheckName::InputResolution)
        .unwrap();
    assert!(!res.pass);
}

#[test]
fn derived_fails_when_parent_value_mutated() {
    // 親 receipt の claim を改竄 → 親自身の ReceiptId が壊れるが、
    // derived 側の入力照合でも検出できること
    let bytes = INSTANCE.as_bytes();
    let mut rs = stated(bytes);
    let derived = build_derived_receipt(
        Operation::Ratio { round_dp: Some(4) },
        &[(&rs[1], "numerator"), (&rs[0], "denominator")],
        "DOC1",
    )
    .unwrap();
    rs[0].claim.value = "999".into();
    let report = verify_derived(&derived, &parents_map(&rs));
    assert_eq!(report.status, ValidationStatus::Failed);
}

#[test]
fn growth_rate_uses_abs_denominator() {
    // -100 → -50 の改善は +50% (edinet-mcp で踏んだ罠の再発防止を仕様に固定)
    let bytes = INSTANCE.as_bytes();
    let rs = stated(bytes);
    let mut cur = rs[0].clone();
    let mut prior = rs[0].clone();
    // 値だけ差し替えた合成 receipt を作るため build 経由でなく直接検証はしない。
    // ここでは演算単体の決定性を Operation::apply で確認する。
    let _ = (&mut cur, &mut prior);
    let v = Operation::GrowthRate { round_dp: Some(4) }
        .apply(&["-50".parse().unwrap(), "-100".parse().unwrap()])
        .unwrap();
    assert_eq!(v.to_string(), "0.5");
}

#[test]
fn signed_receipt_verifies_and_tamper_fails() {
    let bytes = INSTANCE.as_bytes();
    let mut rs = stated(bytes);
    let key = SigningKey::generate();
    sign_receipt(&mut rs[0], &key).unwrap();
    assert!(rs[0].attestation.is_some());
    // 署名込みで通常検証 PASS
    let report = verify(&rs[0], bytes);
    assert_eq!(report.status, ValidationStatus::Verified, "{report:?}");
    let att = report
        .checks
        .iter()
        .find(|c| c.name == CheckName::Attestation)
        .unwrap();
    assert!(att.pass);
    // 署名後に本文を改竄 → Attestation も ReceiptId も FAIL
    rs[0].claim.value = "1".into();
    let report = verify(&rs[0], bytes);
    assert!(
        !report
            .checks
            .iter()
            .find(|c| c.name == CheckName::Attestation)
            .unwrap()
            .pass
    );
}

#[test]
fn unsigned_receipt_has_no_attestation_check() {
    let bytes = INSTANCE.as_bytes();
    let rs = stated(bytes);
    let report = verify(&rs[0], bytes);
    assert!(
        report
            .checks
            .iter()
            .all(|c| c.name != CheckName::Attestation)
    );
}
