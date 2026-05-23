use std::collections::BTreeMap;
use std::fmt;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// --- Qualified Name ---

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct QName {
    pub namespace_uri: Option<String>,
    pub prefix: Option<String>,
    pub local_name: String,
}

impl fmt::Display for QName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.prefix, &self.namespace_uri) {
            (Some(p), _) => write!(f, "{}:{}", p, self.local_name),
            (None, Some(ns)) => write!(f, "{{{}}}{}", ns, self.local_name),
            _ => write!(f, "{}", self.local_name),
        }
    }
}

// --- Lossless Instance Model (XBRL 2.1 §4) ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstanceDocument {
    pub schema_refs: Vec<String>,
    pub contexts: BTreeMap<String, Context>,
    pub units: BTreeMap<String, Unit>,
    pub facts: Vec<RawFact>,
    pub footnotes: Vec<Footnote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RawFact {
    pub id: Option<String>,
    pub name: QName,
    pub value: RawFactValue,
    pub context_ref: String,
    pub unit_ref: Option<String>,
    pub decimals: Option<Decimals>,
    pub precision: Option<Precision>,
    pub lang: Option<String>,
    pub inline_meta: Option<InlineMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
#[non_exhaustive]
pub enum RawFactValue {
    Numeric { raw: String },
    Text { value: String },
    Nil,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InlineMeta {
    pub format: Option<String>,
    pub scale: Option<i32>,
    pub sign: Option<String>,
    pub target: Option<String>,
    pub continued_from: Option<String>,
    pub is_hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
#[non_exhaustive]
pub enum Decimals {
    Infinite,
    Value { n: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
#[non_exhaustive]
pub enum Precision {
    Infinite,
    Value { n: u32 },
}

// --- Context (XBRL 2.1 §4.7) ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Context {
    pub id: String,
    pub entity: Entity,
    pub period: Period,
    pub segment: Vec<ContextElement>,
    pub scenario: Vec<ContextElement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Entity {
    pub scheme: String,
    pub identifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
#[non_exhaustive]
pub enum Period {
    Instant { date: String },
    Duration { start: String, end: String },
    Forever,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
#[non_exhaustive]
pub enum ContextElement {
    ExplicitDimension { dimension: QName, member: QName },
    TypedDimension { dimension: QName, value: String },
    Other { raw_xml: String },
}

// --- Unit (XBRL 2.1 §4.8) ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Unit {
    pub id: String,
    pub numerator: Vec<QName>,
    pub denominator: Vec<QName>,
}

// --- Footnote (XBRL 2.1 §4.11) ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Footnote {
    pub id: Option<String>,
    pub role: Option<String>,
    pub lang: Option<String>,
    pub content: String,
    pub fact_refs: Vec<String>,
}

// --- Normalized Fact (provenance-rich, engine output) ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NormalizedFact {
    pub name: QName,
    pub label: Option<String>,
    pub value: NormalizedValue,
    pub period: Period,
    pub entity: Entity,
    pub unit: Option<Unit>,
    pub dimensions: BTreeMap<QName, DimensionMember>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
#[non_exhaustive]
pub enum NormalizedValue {
    Numeric { raw: String, decimal: Option<Decimal>, decimals: Option<i32> },
    Text { value: String, lang: Option<String> },
    Nil,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
#[non_exhaustive]
pub enum DimensionMember {
    Explicit { member: QName },
    Typed { value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Provenance {
    pub doc_id: String,
    pub accession: Option<String>,
    pub source_url: Option<String>,
    pub element_id: Option<String>,
    pub fact_id: Option<String>,
    pub context_ref: String,
    pub byte_range: Option<(u64, u64)>,
}

// --- Filing (top-level container) ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FilingMeta {
    pub doc_id: String,
    pub entity: Option<Entity>,
    pub filing_date: Option<String>,
    pub doc_type: Option<String>,
    pub taxonomy_version: Option<String>,
    pub source_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qname_display_with_prefix() {
        let q = QName {
            namespace_uri: Some("http://example.com".into()),
            prefix: Some("ex".into()),
            local_name: "NetSales".into(),
        };
        assert_eq!(q.to_string(), "ex:NetSales");
    }

    #[test]
    fn qname_display_namespace_only() {
        let q = QName {
            namespace_uri: Some("http://example.com".into()),
            prefix: None,
            local_name: "NetSales".into(),
        };
        assert_eq!(q.to_string(), "{http://example.com}NetSales");
    }

    #[test]
    fn qname_display_local_only() {
        let q = QName {
            namespace_uri: None,
            prefix: None,
            local_name: "NetSales".into(),
        };
        assert_eq!(q.to_string(), "NetSales");
    }

    #[test]
    fn raw_fact_roundtrip_json() {
        let fact = RawFact {
            id: Some("fact1".into()),
            name: QName {
                namespace_uri: None,
                prefix: Some("jpcrp".into()),
                local_name: "NetSales".into(),
            },
            value: RawFactValue::Numeric {
                raw: "1000000".into(),
            },
            context_ref: "ctx1".into(),
            unit_ref: Some("jpy".into()),
            decimals: Some(Decimals::Value { n: 0 }),
            precision: None,
            lang: None,
            inline_meta: None,
        };
        let json = serde_json::to_string(&fact).unwrap();
        let back: RawFact = serde_json::from_str(&json).unwrap();
        assert_eq!(fact, back);
    }

    #[test]
    fn context_segment_scenario_distinction() {
        let ctx = Context {
            id: "ctx1".into(),
            entity: Entity {
                scheme: "http://disclosure.edinet-fsa.go.jp".into(),
                identifier: "E02144".into(),
            },
            period: Period::Duration {
                start: "2024-04-01".into(),
                end: "2025-03-31".into(),
            },
            segment: vec![ContextElement::ExplicitDimension {
                dimension: QName {
                    namespace_uri: None,
                    prefix: Some("jppfs".into()),
                    local_name: "ConsolidatedOrNonConsolidatedAxis".into(),
                },
                member: QName {
                    namespace_uri: None,
                    prefix: Some("jppfs".into()),
                    local_name: "ConsolidatedMember".into(),
                },
            }],
            scenario: vec![],
        };
        assert_eq!(ctx.segment.len(), 1);
        assert!(ctx.scenario.is_empty());
    }
}
