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
use xbrl_facts_core::types::{InstanceDocument, NormalizedValue};
use xbrl_facts_core::{TaxonomyResolver, normalize_facts, parse_instance};

const ENGINE: &str = concat!("xbrl-facts-evidence@", env!("CARGO_PKG_VERSION"));

// --- Errors ---

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("source parse failed: {0}")]
    Parse(String),
    #[error("receipt serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("derivation failed: {0}")]
    Derivation(String),
    #[error("signing failed: {0}")]
    Signing(String),
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

/// Profile-tagged locator. New regulated-document profiles are added as
/// variants — the receipt schema itself never assumes XBRL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "profile", rename_all = "snake_case")]
pub enum Locator {
    Xbrl(XbrlLocator),
}

impl Locator {
    pub fn xbrl(&self) -> &XbrlLocator {
        match self {
            Locator::Xbrl(l) => l,
        }
    }

    pub fn xbrl_mut(&mut self) -> &mut XbrlLocator {
        match self {
            Locator::Xbrl(l) => l,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Evidence {
    pub source_artifact: SourceArtifact,
    pub locator: Locator,
    pub extraction: Extraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    /// The value as stated in the source document.
    Stated,
    /// A value computed from other receipts via a deterministic operation.
    Derived,
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

/// Deterministic operations over parent receipt values. Rounding uses
/// MidpointAwayFromZero and is part of the recorded operation, so the
/// verifier reproduces the exact same result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    /// inputs[0] / inputs[1]
    Ratio { round_dp: Option<u32> },
    /// inputs[0] - inputs[1]
    Difference,
    /// (inputs[0] - inputs[1]) / |inputs[1]| — abs denominator so that a
    /// shrinking loss reads as positive growth.
    GrowthRate { round_dp: Option<u32> },
}

impl Operation {
    fn round(v: Decimal, dp: Option<u32>) -> Decimal {
        match dp {
            Some(dp) => {
                v.round_dp_with_strategy(dp, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
            }
            None => v,
        }
        .normalize()
    }

    pub fn apply(&self, inputs: &[Decimal]) -> Result<Decimal, EvidenceError> {
        let need = match self {
            Operation::Ratio { .. } | Operation::Difference | Operation::GrowthRate { .. } => 2,
        };
        if inputs.len() != need {
            return Err(EvidenceError::Derivation(format!(
                "operation needs {need} inputs, got {}",
                inputs.len()
            )));
        }
        match self {
            Operation::Ratio { round_dp } => {
                if inputs[1].is_zero() {
                    return Err(EvidenceError::Derivation("division by zero".into()));
                }
                Ok(Self::round(inputs[0] / inputs[1], *round_dp))
            }
            Operation::Difference => Ok((inputs[0] - inputs[1]).normalize()),
            Operation::GrowthRate { round_dp } => {
                if inputs[1].is_zero() {
                    return Err(EvidenceError::Derivation("division by zero".into()));
                }
                Ok(Self::round(
                    (inputs[0] - inputs[1]) / inputs[1].abs(),
                    *round_dp,
                ))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DerivationInput {
    /// Content-hash id of the parent receipt.
    pub receipt_id: String,
    /// The parent's claimed value as consumed (canonical decimal string).
    pub value: String,
    /// Role in the operation (e.g. "numerator", "denominator").
    pub role: String,
}

/// First-class lineage for derived claims — inputs and parameters live
/// here, never overloaded into `evidence`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Derivation {
    pub operation: Operation,
    pub inputs: Vec<DerivationInput>,
}

/// Detached endorsement over the canonical receipt body. Excluded from
/// the receipt_id hash: the id names the content, the attestation vouches
/// for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Attestation {
    pub alg: String,
    pub public_key: String,
    pub sig: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Receipt {
    /// Receipt schema version (canonical encoding is pinned per version).
    pub schema: String,
    pub receipt_id: String,
    pub claim: Claim,
    pub evidence: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derivation: Option<Derivation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<Attestation>,
}

pub const SCHEMA_VERSION: &str = "er/0.2";

/// Content-derived id over the canonical body. Full digest — the id is an
/// audit reference and (in M2) a signing subject, so no truncation.
fn canonical_body(
    claim: &Claim,
    evidence: &[Evidence],
    derivation: &Option<Derivation>,
) -> Result<String, EvidenceError> {
    Ok(serde_json::to_string(&(
        SCHEMA_VERSION,
        claim,
        evidence,
        derivation,
    ))?)
}

fn receipt_id_for(
    claim: &Claim,
    evidence: &[Evidence],
    derivation: &Option<Derivation>,
) -> Result<String, EvidenceError> {
    let body = canonical_body(claim, evidence, derivation)?;
    Ok(format!("er_{}", sha256_hex(body.as_bytes())))
}

// --- Verification ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckName {
    /// The receipt id matches the canonical hash of its own body.
    ReceiptId,
    /// Exactly one evidence item (M1 shape).
    EvidenceShape,
    /// Source bytes hash to the recorded artifact sha256.
    ArtifactHash,
    /// The locator resolves to exactly one fact in the re-parsed source.
    Locate,
    /// The located fact's byte range matches the recorded one.
    ByteRange,
    /// The located fact's normalized value equals the claimed value.
    ValueMatch,
    /// Every derivation input resolves to a known, self-consistent parent.
    InputResolution,
    /// Re-applying the recorded operation reproduces the claimed value.
    Recompute,
    /// The attestation signature is valid over the canonical body.
    Attestation,
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
        // Audit-grade XBRL profile requires an exact byte range; facts
        // assembled outside an XML stream get no receipt.
        if raw.byte_range.is_none() {
            continue;
        }

        let claim = Claim {
            value,
            unit: raw.unit_ref.clone(),
            kind: ClaimKind::Stated,
            doc_id: doc_id.to_string(),
        };
        let evidence = vec![Evidence {
            source_artifact: artifact.clone(),
            locator: Locator::Xbrl(XbrlLocator {
                concept: raw.name.to_string(),
                context_ref: raw.context_ref.clone(),
                unit_ref: raw.unit_ref.clone(),
                byte_range: raw.byte_range,
            }),
            extraction: Extraction {
                engine: ENGINE.to_string(),
            },
        }];

        let receipt_id = receipt_id_for(&claim, &evidence, &None)?;
        receipts.push(Receipt {
            schema: SCHEMA_VERSION.to_string(),
            receipt_id,
            claim,
            evidence,
            derivation: None,
            attestation: None,
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
    /// Fact-index-aligned canonical values, normalized once at load.
    /// (Value derivation does not depend on doc_id.)
    values: Vec<Option<(String, Option<Decimal>)>>,
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
        let values = normalize_facts(&instance, &NoLabels, "")
            .into_iter()
            .map(|n| n.ok().and_then(|norm| canonical_value(&norm.value)))
            .collect();
        Ok(Self {
            sha256: sha256_hex(source_bytes),
            instance,
            index,
            values,
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

/// Verify one receipt against a prepared [`SourceContext`].
///
/// Every check runs even after an earlier failure, so the report shows
/// the full failure surface (e.g. hash fail + value fail on tampering).
pub fn verify_in(ctx: &SourceContext, receipt: &Receipt) -> VerificationReport {
    let mut checks = Vec::new();

    // 0. receipt integrity: the id must be the canonical hash of the body.
    // Without this, a tampered receipt could point at a different (valid)
    // fact and still report Verified.
    let id_ok = matches!(
        receipt_id_for(&receipt.claim, &receipt.evidence, &receipt.derivation),
        Ok(expected) if expected == receipt.receipt_id && receipt.schema == SCHEMA_VERSION
    );
    checks.push(Check {
        name: CheckName::ReceiptId,
        pass: id_ok,
        detail: (!id_ok).then(|| "receipt body does not hash to receipt_id".into()),
    });

    // 0b. M1 shape: exactly one evidence item — trailing items must not be
    // silently ignored, and an empty list must fail loudly, not panic.
    let shape_ok = receipt.evidence.len() == 1;
    checks.push(Check {
        name: CheckName::EvidenceShape,
        pass: shape_ok,
        detail: (!shape_ok).then(|| {
            format!(
                "{} evidence items, expected exactly 1",
                receipt.evidence.len()
            )
        }),
    });

    if let Some(evidence) = receipt.evidence.first() {
        let locator = evidence.locator.xbrl();

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
        let found = ctx.locate(locator);
        let locate_ok = found.len() == 1;
        checks.push(Check {
            name: CheckName::Locate,
            pass: locate_ok,
            detail: (!locate_ok)
                .then(|| format!("matched {} facts, expected exactly 1", found.len())),
        });

        if let Some(&idx) = found.first() {
            let fact = &ctx.instance.facts[idx];

            // 3. byte range — required in the audit-grade XBRL profile
            let range_ok = locator.byte_range.is_some() && fact.byte_range == locator.byte_range;
            checks.push(Check {
                name: CheckName::ByteRange,
                pass: range_ok,
                detail: (!range_ok).then(|| {
                    format!(
                        "recorded {:?}, re-parsed {:?}",
                        locator.byte_range, fact.byte_range
                    )
                }),
            });

            // 4. value match — semantic decimal equality against the value
            // re-derived from source. Byte-exactness of the claim itself is
            // pinned by the ReceiptId check, so "1.0" vs "1" is not a hole.
            let value_check = match &ctx.values[idx] {
                Some((canon, dec)) => {
                    let claim_dec = receipt.claim.value.parse::<Decimal>().ok();
                    let pass = match (dec, claim_dec) {
                        (Some(a), Some(b)) => *a == b,
                        _ => *canon == receipt.claim.value,
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
                    detail: Some("located fact has no re-derivable numeric value".into()),
                },
            };
            checks.push(value_check);
        }
    }

    if let Some(check) = attestation_check(receipt) {
        checks.push(check);
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
            checks: vec![Check {
                name: CheckName::Locate,
                pass: false,
                detail: Some(format!("source re-parse failed: {e}")),
            }],
        },
    }
}

// --- Derived claims (M2) ---

/// Build a derived receipt from parent receipts. The operation is applied
/// to the parents' claimed values in the given order; each input records
/// the parent id, the exact value consumed, and its role.
pub fn build_derived_receipt(
    operation: Operation,
    parents: &[(&Receipt, &str)],
    doc_id: &str,
) -> Result<Receipt, EvidenceError> {
    let values: Vec<Decimal> = parents
        .iter()
        .map(|(p, _)| {
            p.claim
                .value
                .parse::<Decimal>()
                .map_err(|e| EvidenceError::Derivation(format!("parent value not decimal: {e}")))
        })
        .collect::<Result<_, _>>()?;
    let result = operation.apply(&values)?;

    let claim = Claim {
        value: result.to_string(),
        unit: None,
        kind: ClaimKind::Derived,
        doc_id: doc_id.to_string(),
    };
    let derivation = Some(Derivation {
        operation,
        inputs: parents
            .iter()
            .map(|(p, role)| DerivationInput {
                receipt_id: p.receipt_id.clone(),
                value: p.claim.value.clone(),
                role: (*role).to_string(),
            })
            .collect(),
    });
    let receipt_id = receipt_id_for(&claim, &[], &derivation)?;
    Ok(Receipt {
        schema: SCHEMA_VERSION.to_string(),
        receipt_id,
        claim,
        evidence: Vec::new(),
        derivation,
        attestation: None,
    })
}

/// Verify a derived receipt against its parents.
///
/// The chain to primary sources is completed by verifying each stated
/// parent with [`verify_in`]; this function checks the derivation link:
/// parent resolution, parent self-consistency, and recomputation.
pub fn verify_derived(
    receipt: &Receipt,
    parents: &std::collections::HashMap<String, &Receipt>,
) -> VerificationReport {
    let mut checks = Vec::new();

    let id_ok = matches!(
        receipt_id_for(&receipt.claim, &receipt.evidence, &receipt.derivation),
        Ok(expected) if expected == receipt.receipt_id && receipt.schema == SCHEMA_VERSION
    );
    checks.push(Check {
        name: CheckName::ReceiptId,
        pass: id_ok,
        detail: (!id_ok).then(|| "receipt body does not hash to receipt_id".into()),
    });

    let shape_ok = receipt.claim.kind == ClaimKind::Derived
        && receipt.evidence.is_empty()
        && receipt
            .derivation
            .as_ref()
            .is_some_and(|d| !d.inputs.is_empty());
    checks.push(Check {
        name: CheckName::EvidenceShape,
        pass: shape_ok,
        detail: (!shape_ok).then(|| {
            "derived receipt must have kind=derived, empty evidence, non-empty derivation".into()
        }),
    });

    if let Some(derivation) = &receipt.derivation {
        // Inputs resolve to known parents whose bodies are self-consistent
        // and whose claimed values match what this derivation consumed.
        let mut failures = Vec::new();
        let mut values = Vec::new();
        for input in &derivation.inputs {
            match parents.get(&input.receipt_id) {
                None => failures.push(format!("{}: unknown parent", input.receipt_id)),
                Some(parent) => {
                    let parent_ok = matches!(
                        receipt_id_for(&parent.claim, &parent.evidence, &parent.derivation),
                        Ok(expected) if expected == parent.receipt_id
                    );
                    if !parent_ok {
                        failures.push(format!("{}: parent fails integrity", input.receipt_id));
                    } else if parent.claim.value != input.value {
                        failures.push(format!(
                            "{}: consumed {} but parent claims {}",
                            input.receipt_id, input.value, parent.claim.value
                        ));
                    }
                }
            }
            if let Ok(v) = input.value.parse::<Decimal>() {
                values.push(v);
            }
        }
        let res_ok = failures.is_empty();
        checks.push(Check {
            name: CheckName::InputResolution,
            pass: res_ok,
            detail: (!res_ok).then(|| failures.join("; ")),
        });

        // Recompute from the recorded inputs
        let recompute = match derivation.operation.apply(&values) {
            Ok(v) => {
                let claim_dec = receipt.claim.value.parse::<Decimal>().ok();
                let pass = claim_dec.map(|c| c == v).unwrap_or(false);
                Check {
                    name: CheckName::Recompute,
                    pass,
                    detail: (!pass)
                        .then(|| format!("recomputed {v}, claimed {}", receipt.claim.value)),
                }
            }
            Err(e) => Check {
                name: CheckName::Recompute,
                pass: false,
                detail: Some(format!("recompute failed: {e}")),
            },
        };
        checks.push(recompute);
    }

    if let Some(check) = attestation_check(receipt) {
        checks.push(check);
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

// --- Signing (M2) ---

/// Ed25519 signing key for receipt attestations.
pub struct SigningKey(ed25519_dalek::SigningKey);

impl SigningKey {
    pub fn generate() -> Self {
        Self(ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng))
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(ed25519_dalek::SigningKey::from_bytes(bytes))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
}

/// Sign the canonical receipt body. The signature covers exactly the bytes
/// the receipt_id hashes — id names the content, attestation vouches for it.
pub fn sign_receipt(receipt: &mut Receipt, key: &SigningKey) -> Result<(), EvidenceError> {
    use ed25519_dalek::Signer;
    let body = canonical_body(&receipt.claim, &receipt.evidence, &receipt.derivation)?;
    let sig = key.0.sign(body.as_bytes());
    receipt.attestation = Some(Attestation {
        alg: "ed25519".into(),
        public_key: hex::encode(key.0.verifying_key().to_bytes()),
        sig: hex::encode(sig.to_bytes()),
    });
    Ok(())
}

/// Attestation check, or None when the receipt is unsigned.
fn attestation_check(receipt: &Receipt) -> Option<Check> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let att = receipt.attestation.as_ref()?;
    let pass = (|| {
        if att.alg != "ed25519" {
            return Some(false);
        }
        let pk: [u8; 32] = hex::decode(&att.public_key).ok()?.try_into().ok()?;
        let sig: [u8; 64] = hex::decode(&att.sig).ok()?.try_into().ok()?;
        let body = canonical_body(&receipt.claim, &receipt.evidence, &receipt.derivation).ok()?;
        let key = VerifyingKey::from_bytes(&pk).ok()?;
        Some(
            key.verify(body.as_bytes(), &Signature::from_bytes(&sig))
                .is_ok(),
        )
    })()
    .unwrap_or(false);
    Some(Check {
        name: CheckName::Attestation,
        pass,
        detail: (!pass).then(|| "signature invalid over canonical body".into()),
    })
}
