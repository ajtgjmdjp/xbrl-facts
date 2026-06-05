//! XBRL linkbase support.
//!
//! Currently implements the label linkbase only (XBRL 2.1 §5.2.2.1) and the
//! minimum slice of XML Schema needed to resolve `xlink:href` fragments to
//! `(namespace, local_name)` pairs.

use std::collections::HashMap;
use std::str;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::XbrlError;
use crate::parser::TaxonomyResolver;
use crate::types::QName;

/// Index from `(schema_href, element_id)` to the concept's qualified name.
#[derive(Debug, Default, Clone)]
pub struct SchemaIndex {
    by_id: HashMap<(String, String), QName>,
}

impl SchemaIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse an XBRL taxonomy schema document and record its element
    /// declarations under the given `schema_href` (typically the relative
    /// filename used in `xlink:href`).
    pub fn ingest_schema(&mut self, schema_href: &str, input: &[u8]) -> Result<(), XbrlError> {
        let input = str::from_utf8(input).map_err(|e| XbrlError::Xml {
            message: e.to_string(),
            byte_offset: Some(e.valid_up_to() as u64),
        })?;
        let mut reader = Reader::from_str(input);
        reader.config_mut().trim_text(true);

        let mut target_namespace: Option<String> = None;
        let mut prefix_namespaces: HashMap<String, String> = HashMap::new();

        loop {
            match reader.read_event()? {
                Event::Start(element) | Event::Empty(element) => {
                    let name = element.name();
                    let local = local_name(name.as_ref());

                    for attr in element.attributes() {
                        let attr = attr?;
                        let key = attr.key.as_ref();
                        let value = attr.unescape_value()?.into_owned();
                        if key == b"targetNamespace" {
                            target_namespace = Some(value);
                        } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
                            let prefix = str::from_utf8(prefix)
                                .map_err(|e| XbrlError::Xml {
                                    message: e.to_string(),
                                    byte_offset: None,
                                })?
                                .to_owned();
                            prefix_namespaces.insert(prefix, value);
                        }
                    }

                    if local == b"element" {
                        let mut id = None;
                        let mut elem_name = None;
                        for attr in element.attributes() {
                            let attr = attr?;
                            match attr.key.as_ref() {
                                b"id" => id = Some(attr.unescape_value()?.into_owned()),
                                b"name" => elem_name = Some(attr.unescape_value()?.into_owned()),
                                _ => {}
                            }
                        }
                        if let (Some(id), Some(name)) = (id, elem_name) {
                            let prefix = target_namespace.as_ref().and_then(|ns| {
                                prefix_namespaces
                                    .iter()
                                    .find(|(_, v)| *v == ns)
                                    .map(|(k, _)| k.clone())
                            });
                            self.by_id.insert(
                                (schema_href.to_owned(), id),
                                QName {
                                    namespace_uri: target_namespace.clone(),
                                    prefix,
                                    local_name: name,
                                },
                            );
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(())
    }

    /// Look up the qname for `(schema_href, fragment)`.
    pub fn resolve(&self, schema_href: &str, fragment: &str) -> Option<&QName> {
        self.by_id
            .get(&(schema_href.to_owned(), fragment.to_owned()))
    }

    /// Look up by fragment alone — used when the label linkbase references
    /// a schema not in the index. Returns the first match.
    pub fn resolve_by_fragment(&self, fragment: &str) -> Option<&QName> {
        self.by_id
            .iter()
            .find(|((_, id), _)| id == fragment)
            .map(|(_, q)| q)
    }
}

/// Standard XBRL label role.
pub mod role {
    pub const LABEL: &str = "http://www.xbrl.org/2003/role/label";
    pub const VERBOSE_LABEL: &str = "http://www.xbrl.org/2003/role/verboseLabel";
    pub const TERSE_LABEL: &str = "http://www.xbrl.org/2003/role/terseLabel";
    pub const DOCUMENTATION: &str = "http://www.xbrl.org/2003/role/documentation";
}

/// A parsed label linkbase, indexed by concept `QName`.
#[derive(Debug, Default, Clone)]
pub struct LabelLinkbase {
    /// (QName, role, lang) → label text
    by_qname: HashMap<(QName, String, String), String>,
}

impl LabelLinkbase {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a label linkbase XML document, resolving `xlink:href` fragments
    /// via `schema_index`. Loc entries whose fragment is unknown are skipped.
    pub fn ingest(&mut self, input: &[u8], schema_index: &SchemaIndex) -> Result<(), XbrlError> {
        let input = str::from_utf8(input).map_err(|e| XbrlError::Xml {
            message: e.to_string(),
            byte_offset: Some(e.valid_up_to() as u64),
        })?;
        let mut reader = Reader::from_str(input);
        reader.config_mut().trim_text(true);

        // loc xlink:label → resolved QName
        let mut loc_to_qname: HashMap<String, QName> = HashMap::new();
        // label xlink:label → (role, lang, text)
        let mut label_resources: HashMap<String, (String, String, String)> = HashMap::new();
        // arcs: (from_loc_label, to_label_label)
        let mut arcs: Vec<(String, String)> = Vec::new();

        // We need to grab text for label resources, so track current label.
        let mut current_label: Option<(String, String, String)> = None;
        let mut text_buf = String::new();

        loop {
            match reader.read_event()? {
                Event::Start(element) | Event::Empty(element) => {
                    let local = local_name(element.name().as_ref()).to_owned();
                    let local = String::from_utf8(local).unwrap_or_default();
                    let mut attrs: HashMap<String, String> = HashMap::new();
                    for attr in element.attributes() {
                        let attr = attr?;
                        let key = str::from_utf8(attr.key.as_ref())
                            .map_err(|e| XbrlError::Xml {
                                message: e.to_string(),
                                byte_offset: None,
                            })?
                            .to_owned();
                        attrs.insert(key, attr.unescape_value()?.into_owned());
                    }

                    match local.as_str() {
                        "loc" => {
                            let href = attrs.get("xlink:href");
                            let label = attrs.get("xlink:label");
                            if let (Some(href), Some(label)) = (href, label)
                                && let Some((schema, fragment)) = href.split_once('#')
                                && let Some(q) = schema_index
                                    .resolve(schema, fragment)
                                    .or_else(|| schema_index.resolve_by_fragment(fragment))
                            {
                                loc_to_qname.insert(label.clone(), q.clone());
                            }
                        }
                        "labelArc" => {
                            if let (Some(from), Some(to)) =
                                (attrs.get("xlink:from"), attrs.get("xlink:to"))
                            {
                                arcs.push((from.clone(), to.clone()));
                            }
                        }
                        "label" => {
                            if let Some(label_id) = attrs.get("xlink:label").cloned() {
                                let role = attrs
                                    .get("xlink:role")
                                    .cloned()
                                    .unwrap_or_else(|| role::LABEL.to_owned());
                                let lang = attrs.get("xml:lang").cloned().unwrap_or_default();
                                current_label = Some((label_id, role, lang));
                                text_buf.clear();
                            }
                        }
                        _ => {}
                    }
                }
                Event::Text(text) if current_label.is_some() => {
                    text_buf.push_str(text.unescape()?.as_ref());
                }
                Event::CData(text) if current_label.is_some() => {
                    text_buf.push_str(str::from_utf8(&text).unwrap_or_default());
                }
                Event::End(end) => {
                    if local_name(end.name().as_ref()) == b"label"
                        && let Some((label_id, role, lang)) = current_label.take()
                    {
                        label_resources.insert(label_id, (role, lang, text_buf.clone()));
                        text_buf.clear();
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }

        for (from, to) in arcs {
            let Some(qname) = loc_to_qname.get(&from) else {
                continue;
            };
            let Some((role, lang, text)) = label_resources.get(&to) else {
                continue;
            };
            self.by_qname
                .insert((qname.clone(), role.clone(), lang.clone()), text.clone());
        }
        Ok(())
    }

    pub fn lookup(&self, name: &QName, role: &str, lang: Option<&str>) -> Option<&str> {
        let key = (name.clone(), role.to_owned(), lang.unwrap_or("").to_owned());
        if let Some(label) = self.by_qname.get(&key) {
            return Some(label.as_str());
        }
        // Fallback: any lang for the requested role.
        self.by_qname
            .iter()
            .find(|((q, r, _), _)| q == name && r == role)
            .map(|(_, label)| label.as_str())
    }
}

impl TaxonomyResolver for LabelLinkbase {
    fn label(&self, name: &QName, role: Option<&str>, lang: Option<&str>) -> Option<String> {
        let role = role.unwrap_or(role::LABEL);
        self.lookup(name, role, lang).map(str::to_owned)
    }
}

fn local_name(qualified: &[u8]) -> &[u8] {
    qualified
        .iter()
        .rposition(|b| *b == b':')
        .map(|idx| &qualified[idx + 1..])
        .unwrap_or(qualified)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = r#"<?xml version="1.0"?>
<schema xmlns="http://www.w3.org/2001/XMLSchema"
        xmlns:ex="http://example.com/taxonomy"
        targetNamespace="http://example.com/taxonomy">
  <element id="ex_NetSales" name="NetSales"/>
  <element id="ex_TotalAssets" name="TotalAssets"/>
</schema>"#;

    const LAB: &str = r#"<?xml version="1.0"?>
<link:linkbase xmlns:xlink="http://www.w3.org/1999/xlink"
               xmlns:link="http://www.xbrl.org/2003/linkbase">
  <link:labelLink xlink:type="extended" xlink:role="http://www.xbrl.org/2003/role/link">
    <link:label xml:lang="ja" xlink:type="resource"
                xlink:label="net_sales_ja_label"
                xlink:role="http://www.xbrl.org/2003/role/label">売上高</link:label>
    <link:label xml:lang="en" xlink:type="resource"
                xlink:label="net_sales_en_label"
                xlink:role="http://www.xbrl.org/2003/role/label">Net Sales</link:label>
    <link:loc xlink:type="locator"
              xlink:href="example.xsd#ex_NetSales"
              xlink:label="net_sales_loc"/>
    <link:labelArc xlink:type="arc"
                   xlink:from="net_sales_loc"
                   xlink:to="net_sales_ja_label"
                   xlink:arcrole="http://www.xbrl.org/2003/arcrole/concept-label"/>
    <link:labelArc xlink:type="arc"
                   xlink:from="net_sales_loc"
                   xlink:to="net_sales_en_label"
                   xlink:arcrole="http://www.xbrl.org/2003/arcrole/concept-label"/>
  </link:labelLink>
</link:linkbase>"#;

    fn loaded() -> LabelLinkbase {
        let mut schema = SchemaIndex::new();
        schema
            .ingest_schema("example.xsd", SCHEMA.as_bytes())
            .unwrap();
        let mut lab = LabelLinkbase::new();
        lab.ingest(LAB.as_bytes(), &schema).unwrap();
        lab
    }

    #[test]
    fn resolves_ja_label_for_concept() {
        let lab = loaded();
        let qname = QName {
            namespace_uri: Some("http://example.com/taxonomy".into()),
            prefix: Some("ex".into()),
            local_name: "NetSales".into(),
        };
        assert_eq!(
            lab.label(&qname, Some(role::LABEL), Some("ja")).as_deref(),
            Some("売上高")
        );
        assert_eq!(
            lab.label(&qname, Some(role::LABEL), Some("en")).as_deref(),
            Some("Net Sales")
        );
    }

    #[test]
    fn returns_none_for_unknown_concept() {
        let lab = loaded();
        let qname = QName {
            namespace_uri: Some("http://example.com/taxonomy".into()),
            prefix: Some("ex".into()),
            local_name: "Unknown".into(),
        };
        assert!(lab.label(&qname, None, Some("ja")).is_none());
    }

    #[test]
    fn ignores_prefix_in_lookup() {
        let lab = loaded();
        let qname = QName {
            namespace_uri: Some("http://example.com/taxonomy".into()),
            prefix: Some("different".into()),
            local_name: "NetSales".into(),
        };
        assert_eq!(
            lab.label(&qname, None, Some("ja")).as_deref(),
            Some("売上高")
        );
    }
}
