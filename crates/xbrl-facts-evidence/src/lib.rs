//! Evidence receipts: deterministic, machine-verifiable claim-to-source
//! records for XBRL facts.
//!
//! A receipt binds a stated numeric claim to the exact primary-source
//! evidence that supports it: the artifact hash, a precise locator
//! (concept + context + byte range), and the extraction engine. The
//! verifier re-derives everything from the source bytes — no LLM judge,
//! every check is deterministic and reproducible.
//!
//! Design rules (see docs/evidence-receipt-spec-v0):
//! - Core types stay horizontal; XBRL is one locator *profile*.
//! - If a receipt cannot be mechanically re-verified, it must fail loudly.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use xbrl_facts_core::types::{InstanceDocument, NormalizedValue, RawFact};
use xbrl_facts_core::{TaxonomyResolver, normalize_facts, parse_instance};

const ENGINE: &str = concat!("xbrl-facts-evidence@", env!("CARGO_PKG_VERSION"));

// --- Errors ---

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("source parse failed: {0}")]
    Parse(String),
    #[error("receipt serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
}

// --- Horizontal core types ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SourceArtifact {
    pub uri: String,
    /// Hex SHA-256 of the exact source bytes the receipt was built from.
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_at: Option<String>,
    /// Issuing authority identifier (e.g. "jp.fsa.edinet", "us.sec.edgar").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
}

/// XBRL locator profile. Other regulated-document profiles get their own
/// locator structs later; the receipt schema does not assume XBRL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct XbrlLocator {
    /// Concept QName as displayed (prefix:localName).
    pub concept: String,
    pub context_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_ref: Option<String>,
    /// Half-open byte range of the source element, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_range: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Extraction {
    pub engine: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Evidence {
    pub source_artifact: SourceArtifact,
    pub locator: XbrlLocator,
    pub extraction: Extraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    /// The value as stated in the source document.
    Stated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Claim {
    /// Canonical decimal string of the claimed value.
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub kind: ClaimKind,
    /// Document the claim was drawn from.
    pub doc_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Receipt {
    pub receipt_id: String,
    pub claim: Claim,
    pub evidence: Vec<Evidence>,
}

// --- Verification ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckName {
    /// Source bytes hash to the recorded artifact sha256.
    ArtifactHash,
    /// The locator resolves to exactly one fact in the re-parsed source.
    Locate,
    /// The located fact's byte range matches the recorded one.
    ByteRange,
    /// The located fact's normalized value equals the claimed value.
    ValueMatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Check {
    pub name: CheckName,
    pub pass: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Verified,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VerificationReport {
    pub receipt_id: String,
    pub status: ValidationStatus,
    pub checks: Vec<Check>,
}

// --- Helpers ---

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

struct NoLabels;

impl TaxonomyResolver for NoLabels {
    fn label(
        &self,
        _name: &xbrl_facts_core::types::QName,
        _role: Option<&str>,
        _lang: Option<&str>,
    ) -> Option<String> {
        None
    }
}

fn canonical_value(v: &NormalizedValue) -> Option<(String, Option<Decimal>)> {
    match v {
        NormalizedValue::Numeric { raw, decimal, .. } => Some((
            decimal
                .map(|d| d.normalize().to_string())
                .unwrap_or_else(|| raw.clone()),
            *decimal,
        )),
        _ => None,
    }
}

// --- Building ---

/// Build receipts for every numeric fact in a parsed instance.
///
/// Facts that fail normalization (missing context/unit) are skipped —
/// a receipt must never be issued for evidence that cannot be re-derived.
pub fn build_receipts(
    instance: &InstanceDocument,
    doc_id: &str,
    artifact: &SourceArtifact,
) -> Result<Vec<Receipt>, EvidenceError> {
    let normalized = normalize_facts(instance, &NoLabels, doc_id);
    let mut receipts = Vec::new();

    for (raw, norm) in instance.facts.iter().zip(normalized) {
        let Ok(norm) = norm else { continue };
        let Some((value, _)) = canonical_value(&norm.value) else {
            continue;
        };

        let claim = Claim {
            value,
            unit: raw.unit_ref.clone(),
            kind: ClaimKind::Stated,
            doc_id: doc_id.to_string(),
        };
        let evidence = vec![Evidence {
            source_artifact: artifact.clone(),
            locator: XbrlLocator {
                concept: raw.name.to_string(),
                context_ref: raw.context_ref.clone(),
                unit_ref: raw.unit_ref.clone(),
                byte_range: raw.byte_range,
            },
            extraction: Extraction {
                engine: ENGINE.to_string(),
            },
        }];

        // Deterministic id: content hash of the claim+evidence body.
        let body = serde_json::to_string(&(&claim, &evidence))?;
        let receipt_id = format!("er_{}", &sha256_hex(body.as_bytes())[..16]);

        receipts.push(Receipt {
            receipt_id,
            claim,
            evidence,
        });
    }
    Ok(receipts)
}

// --- Verifying ---

/// A parsed-and-indexed source document. Build once, verify many receipts —
/// naive per-receipt re-parsing is O(receipts x document) and unusable on
/// real filings (a 5 MB EDINET instance yields ~1,300 receipts).
pub struct SourceContext {
    sha256: String,
    instance: InstanceDocument,
    /// (concept, context_ref, unit_ref) -> fact indices
    index: std::collections::HashMap<(String, String, Option<String>), Vec<usize>>,
}

impl SourceContext {
    pub fn load(source_bytes: &[u8]) -> Result<Self, EvidenceError> {
        let instance =
            parse_instance(source_bytes).map_err(|e| EvidenceError::Parse(e.to_string()))?;
        let mut index: std::collections::HashMap<_, Vec<usize>> = std::collections::HashMap::new();
        for (i, f) in instance.facts.iter().enumerate() {
            index
                .entry((
                    f.name.to_string(),
                    f.context_ref.clone(),
                    f.unit_ref.clone(),
                ))
                .or_default()
                .push(i);
        }
        Ok(Self {
            sha256: sha256_hex(source_bytes),
            instance,
            index,
        })
    }

    fn locate(&self, locator: &XbrlLocator) -> Vec<usize> {
        let candidates = self
            .index
            .get(&(
                locator.concept.clone(),
                locator.context_ref.clone(),
                locator.unit_ref.clone(),
            ))
            .cloned()
            .unwrap_or_default();
        // Duplicate facts (same concept/context/unit marked up in several
        // places) are normal in XBRL — the byte range disambiguates.
        if candidates.len() > 1 && locator.byte_range.is_some() {
            let exact: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&i| self.instance.facts[i].byte_range == locator.byte_range)
                .collect();
            if !exact.is_empty() {
                return exact;
            }
        }
        candidates
    }
}

#[allow(dead_code)]
fn locate<'a>(instance: &'a InstanceDocument, locator: &XbrlLocator) -> Vec<&'a RawFact> {
    instance
        .facts
        .iter()
        .filter(|f| {
            f.name.to_string() == locator.concept
                && f.context_ref == locator.context_ref
                && f.unit_ref == locator.unit_ref
        })
        .collect()
}

/// Verify one receipt against a prepared [`SourceContext`].
///
/// Every check runs even after an earlier failure, so the report shows
/// the full failure surface (e.g. hash fail + value fail on tampering).
pub fn verify_in(ctx: &SourceContext, receipt: &Receipt) -> VerificationReport {
    let mut checks = Vec::new();
    let evidence = &receipt.evidence[0];

    // 1. artifact hash
    let hash_ok = ctx.sha256 == evidence.source_artifact.sha256;
    checks.push(Check {
        name: CheckName::ArtifactHash,
        pass: hash_ok,
        detail: (!hash_ok).then(|| {
            format!(
                "expected {}, got {}",
                evidence.source_artifact.sha256, ctx.sha256
            )
        }),
    });

    // 2. locate
    let found = ctx.locate(&evidence.locator);
    let locate_ok = found.len() == 1;
    checks.push(Check {
        name: CheckName::Locate,
        pass: locate_ok,
        detail: (!locate_ok).then(|| format!("matched {} facts, expected exactly 1", found.len())),
    });

    if let Some(&idx) = found.first() {
        let fact = &ctx.instance.facts[idx];

        // 3. byte range
        let range_ok = fact.byte_range == evidence.locator.byte_range;
        checks.push(Check {
            name: CheckName::ByteRange,
            pass: range_ok,
            detail: (!range_ok).then(|| {
                format!(
                    "recorded {:?}, re-parsed {:?}",
                    evidence.locator.byte_range, fact.byte_range
                )
            }),
        });

        // 4. value match — re-derive the normalized value from source
        let normalized = normalize_facts(&ctx.instance, &NoLabels, &receipt.claim.doc_id)
            .into_iter()
            .nth(idx);
        let value_check = match normalized {
            Some(Ok(norm)) => match canonical_value(&norm.value) {
                Some((canon, dec)) => {
                    let claim_dec = receipt.claim.value.parse::<Decimal>().ok();
                    let pass = match (dec, claim_dec) {
                        (Some(a), Some(b)) => a == b,
                        _ => canon == receipt.claim.value,
                    };
                    Check {
                        name: CheckName::ValueMatch,
                        pass,
                        detail: (!pass).then(|| {
                            format!("claimed {}, source has {canon}", receipt.claim.value)
                        }),
                    }
                }
                None => Check {
                    name: CheckName::ValueMatch,
                    pass: false,
                    detail: Some("located fact is not numeric".into()),
                },
            },
            _ => Check {
                name: CheckName::ValueMatch,
                pass: false,
                detail: Some("located fact failed normalization".into()),
            },
        };
        checks.push(value_check);
    }

    let status = if checks.iter().all(|c| c.pass) {
        ValidationStatus::Verified
    } else {
        ValidationStatus::Failed
    };
    VerificationReport {
        receipt_id: receipt.receipt_id.clone(),
        status,
        checks,
    }
}

/// Convenience single-receipt verification. For many receipts against the
/// same source, build a [`SourceContext`] once and call [`verify_in`].
pub fn verify(receipt: &Receipt, source_bytes: &[u8]) -> VerificationReport {
    match SourceContext::load(source_bytes) {
        Ok(ctx) => verify_in(&ctx, receipt),
        Err(e) => VerificationReport {
            receipt_id: receipt.receipt_id.clone(),
            status: ValidationStatus::Failed,
            checks: vec![
                Check {
                    name: CheckName::ArtifactHash,
                    pass: sha256_hex(source_bytes) == receipt.evidence[0].source_artifact.sha256,
                    detail: None,
                },
                Check {
                    name: CheckName::Locate,
                    pass: false,
                    detail: Some(format!("source re-parse failed: {e}")),
                },
            ],
        },
    }
}
