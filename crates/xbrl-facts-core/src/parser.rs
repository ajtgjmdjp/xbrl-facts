use std::collections::{BTreeMap, HashSet};
use std::str;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use rust_decimal::Decimal;

use crate::error::XbrlError;
use crate::types::{
    Context, ContextElement, Decimals, Dimension, DimensionMember, Entity, Footnote, InlineMeta,
    InstanceDocument, NormalizedFact, NormalizedValue, Period, Precision, Provenance, QName,
    RawFact, RawFactValue, Unit,
};

pub fn parse_instance(input: &[u8]) -> Result<InstanceDocument, XbrlError> {
    let input = str::from_utf8(input).map_err(|e| XbrlError::Xml {
        message: e.to_string(),
        byte_offset: Some(e.valid_up_to() as u64),
    })?;
    let mut parser = InstanceParser::new(input);
    parser.parse()
}

/// Parse an Inline XBRL Document Set (IXDS): multiple iXBRL files that share
/// a single virtual instance. Contexts and units must be unique across the
/// set; conflicting definitions return [`XbrlError::IxdsConflict`].
///
/// Per Inline XBRL 1.1 §5, EDINET filings split content across multiple
/// `*_ixbrl.htm` files where the `0000000_header_*.htm` typically defines all
/// contexts/units in `ix:resources` while sibling files carry facts.
pub fn parse_instance_set<I>(inputs: I) -> Result<InstanceDocument, XbrlError>
where
    I: IntoIterator,
    I::Item: AsRef<[u8]>,
{
    let mut merged = InstanceDocument {
        schema_refs: Vec::new(),
        contexts: BTreeMap::new(),
        units: BTreeMap::new(),
        facts: Vec::new(),
        footnotes: Vec::new(),
    };

    for input in inputs {
        let doc = parse_instance(input.as_ref())?;
        for schema_ref in doc.schema_refs {
            if !merged.schema_refs.contains(&schema_ref) {
                merged.schema_refs.push(schema_ref);
            }
        }
        for (id, ctx) in doc.contexts {
            match merged.contexts.get(&id) {
                None => {
                    merged.contexts.insert(id, ctx);
                }
                Some(existing) if existing == &ctx => {}
                Some(_) => {
                    return Err(XbrlError::IxdsConflict {
                        kind: "context",
                        id,
                    });
                }
            }
        }
        for (id, unit) in doc.units {
            match merged.units.get(&id) {
                None => {
                    merged.units.insert(id, unit);
                }
                Some(existing) if existing == &unit => {}
                Some(_) => {
                    return Err(XbrlError::IxdsConflict { kind: "unit", id });
                }
            }
        }
        merged.facts.extend(doc.facts);
        merged.footnotes.extend(doc.footnotes);
    }

    Ok(merged)
}

pub trait TaxonomyResolver {
    fn label(
        &self,
        name: &crate::types::QName,
        role: Option<&str>,
        lang: Option<&str>,
    ) -> Option<String>;
}

pub fn normalize_facts(
    instance: &InstanceDocument,
    taxonomy: &dyn TaxonomyResolver,
    doc_id: &str,
) -> Vec<Result<NormalizedFact, XbrlError>> {
    instance
        .facts
        .iter()
        .map(|fact| normalize_fact(instance, taxonomy, doc_id, fact))
        .collect()
}

fn normalize_fact(
    instance: &InstanceDocument,
    taxonomy: &dyn TaxonomyResolver,
    doc_id: &str,
    fact: &RawFact,
) -> Result<NormalizedFact, XbrlError> {
    let context =
        instance
            .contexts
            .get(&fact.context_ref)
            .ok_or_else(|| XbrlError::MissingContext {
                context_ref: fact.context_ref.clone(),
            })?;
    let unit = fact
        .unit_ref
        .as_ref()
        .map(|unit_ref| {
            instance
                .units
                .get(unit_ref)
                .cloned()
                .ok_or_else(|| XbrlError::MissingUnit {
                    unit_ref: unit_ref.clone(),
                })
        })
        .transpose()?;

    let value = match &fact.value {
        RawFactValue::Numeric { raw } => NormalizedValue::Numeric {
            raw: raw.clone(),
            decimal: Some(parse_normalized_decimal(raw, fact.inline_meta.as_ref())?),
            decimals: fact.decimals.clone(),
        },
        RawFactValue::Text { value } => NormalizedValue::Text {
            value: apply_text_transform(value, fact.inline_meta.as_ref()),
            lang: fact.lang.clone(),
        },
        RawFactValue::Nil => NormalizedValue::Nil,
    };

    Ok(NormalizedFact {
        name: fact.name.clone(),
        label: taxonomy.label(&fact.name, None, fact.lang.as_deref()),
        value,
        period: context.period.clone(),
        entity: context.entity.clone(),
        unit,
        dimensions: context_dimensions(context),
        provenance: Provenance {
            doc_id: doc_id.to_owned(),
            accession: None,
            source_url: None,
            element_id: None,
            fact_id: fact.id.clone(),
            context_ref: fact.context_ref.clone(),
            byte_range: fact.byte_range,
        },
    })
}

fn context_dimensions(context: &Context) -> Vec<Dimension> {
    context
        .segment
        .iter()
        .chain(context.scenario.iter())
        .filter_map(|element| match element {
            ContextElement::ExplicitDimension { dimension, member } => Some(Dimension {
                dimension: dimension.clone(),
                member: DimensionMember::Explicit {
                    member: member.clone(),
                },
            }),
            ContextElement::TypedDimension { dimension, raw_xml } => Some(Dimension {
                dimension: dimension.clone(),
                member: DimensionMember::Typed {
                    raw_xml: raw_xml.clone(),
                },
            }),
            ContextElement::Other { .. } => None,
        })
        .collect()
}

#[derive(Debug, Clone)]
struct Frame {
    qname: QName,
    namespaces: NamespaceMap,
}

type NamespaceMap = BTreeMap<Option<String>, String>;

struct InstanceParser<'a> {
    reader: Reader<&'a [u8]>,
    stack: Vec<Frame>,
    doc: InstanceDocument,
    context: Option<ContextBuilder>,
    unit: Option<UnitBuilder>,
    fact_stack: Vec<FactBuilder>,
    continuation_stack: Vec<ContinuationBuilder>,
    text_target: Option<TextTarget>,
    hidden_depth: usize,
    continuations: BTreeMap<String, Continuation>,
    locators: BTreeMap<String, String>,
    footnote_labels: BTreeMap<String, usize>,
    footnote_arcs: Vec<(String, String)>,
}

impl<'a> InstanceParser<'a> {
    fn new(input: &'a str) -> Self {
        let mut reader = Reader::from_str(input);
        reader.config_mut().trim_text(true);
        Self {
            reader,
            stack: Vec::new(),
            doc: InstanceDocument {
                schema_refs: Vec::new(),
                contexts: BTreeMap::new(),
                units: BTreeMap::new(),
                facts: Vec::new(),
                footnotes: Vec::new(),
            },
            context: None,
            unit: None,
            fact_stack: Vec::new(),
            continuation_stack: Vec::new(),
            text_target: None,
            hidden_depth: 0,
            continuations: BTreeMap::new(),
            locators: BTreeMap::new(),
            footnote_labels: BTreeMap::new(),
            footnote_arcs: Vec::new(),
        }
    }

    fn parse(&mut self) -> Result<InstanceDocument, XbrlError> {
        loop {
            match self.reader.read_event()? {
                Event::Start(element) => {
                    self.start_element(&element)?;
                }
                Event::Empty(element) => {
                    let pushed = self.start_element(&element)?;
                    if pushed {
                        self.end_element(element.name().as_ref())?;
                    }
                }
                Event::Text(text) => self.text(text.unescape()?.as_ref())?,
                Event::CData(text) => self.text(str::from_utf8(&text).unwrap_or_default())?,
                Event::End(end) => self.end_element(end.name().as_ref())?,
                Event::Eof => break,
                _ => {}
            }
        }
        resolve_continuations(&mut self.doc.facts, &self.continuations);
        resolve_footnote_refs(
            &mut self.doc.footnotes,
            &self.locators,
            &self.footnote_labels,
            &self.footnote_arcs,
        );
        Ok(std::mem::replace(
            &mut self.doc,
            InstanceDocument {
                schema_refs: Vec::new(),
                contexts: BTreeMap::new(),
                units: BTreeMap::new(),
                facts: Vec::new(),
                footnotes: Vec::<Footnote>::new(),
            },
        ))
    }

    fn start_element(&mut self, element: &BytesStart<'_>) -> Result<bool, XbrlError> {
        let parent_namespaces = self
            .stack
            .last()
            .map(|frame| frame.namespaces.clone())
            .unwrap_or_default();
        let (attrs, namespaces) = collect_attrs(element, &self.reader, parent_namespaces)?;
        let qname = parse_qname_bytes(element.name().as_ref(), &namespaces, true)?;

        if self.context.is_some() && qname.local_name == "typedMember" {
            self.typed_member(element, &attrs, &namespaces)?;
            return Ok(false);
        }

        if qname.local_name == "continuation" {
            let frame = Frame {
                qname: qname.clone(),
                namespaces: namespaces.clone(),
            };
            self.stack.push(frame);
            self.continuation_stack.push(ContinuationBuilder {
                id: attrs.get("id").cloned(),
                continued_at: attrs.get("continuedAt").cloned(),
                text: String::new(),
            });
            return Ok(true);
        }

        match qname.local_name.as_str() {
            "loc" => {
                self.locator(&attrs);
                return Ok(false);
            }
            "footnoteArc" => {
                self.footnote_arc(&attrs);
                return Ok(false);
            }
            "footnote" => {
                self.footnote(element, &attrs)?;
                return Ok(false);
            }
            _ => {}
        }

        let frame = Frame {
            qname: qname.clone(),
            namespaces: namespaces.clone(),
        };
        self.stack.push(frame);

        if qname.local_name == "hidden" {
            self.hidden_depth += 1;
            return Ok(true);
        }

        if qname.local_name == "schemaRef" {
            if let Some(href) = attrs.get("href").or_else(|| attrs.get("xlink:href")) {
                self.doc.schema_refs.push(href.clone());
            }
            return Ok(true);
        }

        // Inline facts (ix:nonFraction/ix:nonNumeric) can nest inside other
        // inline facts, so check this before bumping the parent's depth.
        let byte_start = self.reader.buffer_position();
        if let Some(fact) = FactBuilder::inline(
            &qname,
            &attrs,
            &namespaces,
            self.hidden_depth > 0,
            byte_start,
        )? {
            self.fact_stack.push(fact);
            self.text_target = Some(TextTarget::Fact);
            return Ok(true);
        }

        if !self.fact_stack.is_empty() {
            if let Some(fact) = self.fact_stack.last_mut() {
                fact.depth += 1;
            }
            return Ok(true);
        }

        if qname.local_name == "context" {
            if let Some(id) = attrs.get("id") {
                self.context = Some(ContextBuilder::new(id.clone()));
            }
            return Ok(true);
        }

        if qname.local_name == "unit" {
            if let Some(id) = attrs.get("id") {
                self.unit = Some(UnitBuilder::new(id.clone()));
            }
            return Ok(true);
        }

        if self.context.is_some() {
            self.context_start(&qname, &attrs, &namespaces)?;
            return Ok(true);
        }

        if self.unit.is_some() {
            self.unit_start(&qname);
            return Ok(true);
        }

        if let Some(context_ref) = attrs.get("contextRef").cloned() {
            self.fact_stack
                .push(FactBuilder::new(qname, context_ref, &attrs, byte_start)?);
            self.text_target = Some(TextTarget::Fact);
        }

        Ok(true)
    }

    fn end_element(&mut self, raw_name: &[u8]) -> Result<(), XbrlError> {
        let ended = self.stack.pop().map(|frame| frame.qname);
        let Some(qname) = ended else {
            return Err(XbrlError::Xml {
                message: format!(
                    "unexpected closing tag {}",
                    str::from_utf8(raw_name).unwrap_or("<non-utf8>")
                ),
                byte_offset: None,
            });
        };

        if qname.local_name == "continuation"
            && let Some(c) = self.continuation_stack.pop()
        {
            if let Some(id) = c.id {
                self.continuations.insert(
                    id,
                    Continuation {
                        text: c.text,
                        continued_at: c.continued_at,
                    },
                );
            }
            return Ok(());
        }

        if let Some(top) = self.fact_stack.last_mut() {
            if top.depth > 0 {
                top.depth -= 1;
            } else {
                let byte_end = self.reader.buffer_position();
                let finished = self.fact_stack.pop().expect("fact exists").finish(byte_end);
                self.doc.facts.push(finished);
                if self.fact_stack.is_empty() {
                    self.text_target = None;
                }
            }
            return Ok(());
        }

        if qname.local_name == "hidden" {
            self.hidden_depth = self.hidden_depth.saturating_sub(1);
            return Ok(());
        }

        if qname.local_name == "context" {
            if let Some(context) = self.context.take().and_then(ContextBuilder::finish) {
                self.doc.contexts.insert(context.id.clone(), context);
            }
            return Ok(());
        }

        if qname.local_name == "unit" {
            if let Some(unit) = self.unit.take() {
                let unit = unit.finish();
                self.doc.units.insert(unit.id.clone(), unit);
            }
            return Ok(());
        }

        if let Some(context) = &mut self.context {
            match qname.local_name.as_str() {
                "segment" | "scenario" => context.container = ContextContainer::None,
                _ => {}
            }
        }

        if let Some(unit) = &mut self.unit {
            match qname.local_name.as_str() {
                "unitDenominator" => unit.in_denominator = false,
                "unitNumerator" => unit.in_denominator = false,
                _ => {}
            }
        }

        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), XbrlError> {
        if text.trim().is_empty() {
            return Ok(());
        }

        if !self.fact_stack.is_empty() || !self.continuation_stack.is_empty() {
            let trimmed = text.trim();
            for fact in self.fact_stack.iter_mut() {
                fact.text.push_str(trimmed);
            }
            for c in self.continuation_stack.iter_mut() {
                c.text.push_str(trimmed);
            }
            return Ok(());
        }

        let Some(target) = self.text_target.take() else {
            return Ok(());
        };

        match target {
            TextTarget::Identifier => {
                if let Some(context) = &mut self.context {
                    context.entity_identifier = Some(text.trim().to_owned());
                }
            }
            TextTarget::Instant => {
                if let Some(context) = &mut self.context {
                    context.period = Some(Period::Instant {
                        date: text.trim().to_owned(),
                    });
                }
            }
            TextTarget::StartDate => {
                if let Some(context) = &mut self.context {
                    context.start_date = Some(text.trim().to_owned());
                }
            }
            TextTarget::EndDate => {
                if let Some(context) = &mut self.context {
                    context.end_date = Some(text.trim().to_owned());
                }
            }
            TextTarget::ExplicitMember {
                dimension,
                container,
            } => {
                let namespaces = self
                    .stack
                    .last()
                    .map(|frame| &frame.namespaces)
                    .ok_or_else(|| XbrlError::Xml {
                        message: "missing namespace scope".to_owned(),
                        byte_offset: None,
                    })?;
                let member = parse_qname_str(text.trim(), namespaces, false)?;
                if let Some(context) = &mut self.context {
                    context.push_element(
                        container,
                        ContextElement::ExplicitDimension { dimension, member },
                    );
                }
            }
            TextTarget::Measure { denominator } => {
                let namespaces = self
                    .stack
                    .last()
                    .map(|frame| &frame.namespaces)
                    .ok_or_else(|| XbrlError::Xml {
                        message: "missing namespace scope".to_owned(),
                        byte_offset: None,
                    })?;
                let measure = parse_qname_str(text.trim(), namespaces, false)?;
                if let Some(unit) = &mut self.unit {
                    if denominator {
                        unit.denominator.push(measure);
                    } else {
                        unit.numerator.push(measure);
                    }
                }
            }
            TextTarget::Fact => {}
        }

        Ok(())
    }

    fn context_start(
        &mut self,
        qname: &QName,
        attrs: &BTreeMap<String, String>,
        namespaces: &NamespaceMap,
    ) -> Result<(), XbrlError> {
        let Some(context) = &mut self.context else {
            return Ok(());
        };
        match qname.local_name.as_str() {
            "identifier" => {
                context.entity_scheme = attrs.get("scheme").cloned();
                self.text_target = Some(TextTarget::Identifier);
            }
            "instant" => self.text_target = Some(TextTarget::Instant),
            "startDate" => self.text_target = Some(TextTarget::StartDate),
            "endDate" => self.text_target = Some(TextTarget::EndDate),
            "forever" => context.period = Some(Period::Forever),
            "segment" => context.container = ContextContainer::Segment,
            "scenario" => context.container = ContextContainer::Scenario,
            "explicitMember" => {
                if let Some(dimension) = attrs.get("dimension") {
                    self.text_target = Some(TextTarget::ExplicitMember {
                        dimension: parse_qname_str(dimension, namespaces, false)?,
                        container: context.container,
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn typed_member(
        &mut self,
        element: &BytesStart<'_>,
        attrs: &BTreeMap<String, String>,
        namespaces: &NamespaceMap,
    ) -> Result<(), XbrlError> {
        let Some(context) = &mut self.context else {
            return Ok(());
        };
        let Some(dimension) = attrs.get("dimension") else {
            return Ok(());
        };
        let container = context.container;
        let dimension = parse_qname_str(dimension, namespaces, false)?;
        let raw_xml = self.reader.read_text(element.to_end().name())?.into_owned();
        context.push_element(
            container,
            ContextElement::TypedDimension { dimension, raw_xml },
        );
        Ok(())
    }

    fn locator(&mut self, attrs: &BTreeMap<String, String>) {
        let Some(label) = attrs.get("label") else {
            return;
        };
        let Some(href) = attrs.get("href") else {
            return;
        };
        let Some((_, fragment)) = href.rsplit_once('#') else {
            return;
        };
        self.locators.insert(label.clone(), fragment.to_owned());
    }

    fn footnote_arc(&mut self, attrs: &BTreeMap<String, String>) {
        let (Some(from), Some(to)) = (attrs.get("from"), attrs.get("to")) else {
            return;
        };
        self.footnote_arcs.push((from.clone(), to.clone()));
    }

    fn footnote(
        &mut self,
        element: &BytesStart<'_>,
        attrs: &BTreeMap<String, String>,
    ) -> Result<(), XbrlError> {
        let label = attrs.get("label").cloned();
        let content = self.reader.read_text(element.to_end().name())?.into_owned();
        let index = self.doc.footnotes.len();
        if let Some(label) = label {
            self.footnote_labels.insert(label, index);
        }
        self.doc.footnotes.push(Footnote {
            id: attrs.get("id").cloned(),
            role: attrs.get("role").cloned(),
            lang: attrs.get("lang").or_else(|| attrs.get("xml:lang")).cloned(),
            content,
            fact_refs: Vec::new(),
        });
        Ok(())
    }

    fn unit_start(&mut self, qname: &QName) {
        let Some(unit) = &mut self.unit else {
            return;
        };
        match qname.local_name.as_str() {
            "unitDenominator" => unit.in_denominator = true,
            "unitNumerator" => unit.in_denominator = false,
            "measure" => {
                self.text_target = Some(TextTarget::Measure {
                    denominator: unit.in_denominator,
                });
            }
            _ => {}
        }
    }
}

fn collect_attrs(
    element: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
    mut namespaces: NamespaceMap,
) -> Result<(BTreeMap<String, String>, NamespaceMap), XbrlError> {
    let mut attrs = BTreeMap::new();
    for attr in element.attributes().with_checks(false) {
        let attr = attr?;
        let key = str::from_utf8(attr.key.as_ref())
            .map_err(|e| XbrlError::Xml {
                message: e.to_string(),
                byte_offset: None,
            })?
            .to_owned();
        let value = attr
            .decode_and_unescape_value(reader.decoder())?
            .into_owned();

        if key == "xmlns" {
            namespaces.insert(None, value);
            continue;
        }
        if let Some(prefix) = key.strip_prefix("xmlns:") {
            namespaces.insert(Some(prefix.to_owned()), value);
            continue;
        }

        attrs.insert(key.clone(), value.clone());
        let local_key = key
            .rsplit_once(':')
            .map_or(key.as_str(), |(_, local)| local);
        attrs.entry(local_key.to_owned()).or_insert(value);
    }
    Ok((attrs, namespaces))
}

fn parse_qname_bytes(
    raw: &[u8],
    namespaces: &NamespaceMap,
    default_namespace: bool,
) -> Result<QName, XbrlError> {
    let raw = str::from_utf8(raw).map_err(|e| XbrlError::Xml {
        message: e.to_string(),
        byte_offset: None,
    })?;
    parse_qname_str(raw, namespaces, default_namespace)
}

fn parse_qname_str(
    raw: &str,
    namespaces: &NamespaceMap,
    default_namespace: bool,
) -> Result<QName, XbrlError> {
    let (prefix, local_name) = raw
        .split_once(':')
        .map_or((None, raw), |(prefix, local)| (Some(prefix), local));
    let namespace_uri = match prefix {
        Some(prefix) => namespaces.get(&Some(prefix.to_owned())).cloned(),
        None if default_namespace => namespaces.get(&None).cloned(),
        None => None,
    };

    Ok(QName {
        namespace_uri,
        prefix: prefix.map(str::to_owned),
        local_name: local_name.to_owned(),
    })
}

#[derive(Debug, Clone)]
struct ContextBuilder {
    id: String,
    entity_scheme: Option<String>,
    entity_identifier: Option<String>,
    period: Option<Period>,
    start_date: Option<String>,
    end_date: Option<String>,
    segment: Vec<ContextElement>,
    scenario: Vec<ContextElement>,
    container: ContextContainer,
}

impl ContextBuilder {
    fn new(id: String) -> Self {
        Self {
            id,
            entity_scheme: None,
            entity_identifier: None,
            period: None,
            start_date: None,
            end_date: None,
            segment: Vec::new(),
            scenario: Vec::new(),
            container: ContextContainer::None,
        }
    }

    fn push_element(&mut self, container: ContextContainer, element: ContextElement) {
        match container {
            ContextContainer::Segment => self.segment.push(element),
            ContextContainer::Scenario => self.scenario.push(element),
            ContextContainer::None => {}
        }
    }

    fn finish(self) -> Option<Context> {
        let period = self.period.or_else(|| {
            Some(Period::Duration {
                start: self.start_date?,
                end: self.end_date?,
            })
        })?;
        Some(Context {
            id: self.id,
            entity: Entity {
                scheme: self.entity_scheme?,
                identifier: self.entity_identifier?,
            },
            period,
            segment: self.segment,
            scenario: self.scenario,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum ContextContainer {
    None,
    Segment,
    Scenario,
}

#[derive(Debug, Clone)]
struct UnitBuilder {
    id: String,
    numerator: Vec<QName>,
    denominator: Vec<QName>,
    in_denominator: bool,
}

impl UnitBuilder {
    fn new(id: String) -> Self {
        Self {
            id,
            numerator: Vec::new(),
            denominator: Vec::new(),
            in_denominator: false,
        }
    }

    fn finish(self) -> Unit {
        Unit {
            id: self.id,
            numerator: self.numerator,
            denominator: self.denominator,
        }
    }
}

#[derive(Debug, Clone)]
struct FactBuilder {
    id: Option<String>,
    name: QName,
    context_ref: String,
    unit_ref: Option<String>,
    decimals: Option<Decimals>,
    precision: Option<Precision>,
    lang: Option<String>,
    nil: bool,
    text: String,
    depth: usize,
    kind: Option<FactKind>,
    inline_meta: Option<InlineMeta>,
    byte_start: u64,
}

impl FactBuilder {
    fn new(
        name: QName,
        context_ref: String,
        attrs: &BTreeMap<String, String>,
        byte_start: u64,
    ) -> Result<Self, XbrlError> {
        Ok(Self {
            id: attrs.get("id").cloned(),
            name,
            context_ref,
            unit_ref: attrs.get("unitRef").cloned(),
            decimals: attrs
                .get("decimals")
                .map(|value| parse_decimals(value))
                .transpose()?,
            precision: attrs
                .get("precision")
                .map(|value| parse_precision(value))
                .transpose()?,
            lang: attrs.get("lang").or_else(|| attrs.get("xml:lang")).cloned(),
            nil: attrs
                .get("nil")
                .is_some_and(|value| value == "true" || value == "1"),
            text: String::new(),
            depth: 0,
            kind: None,
            inline_meta: None,
            byte_start,
        })
    }

    fn inline(
        element_name: &QName,
        attrs: &BTreeMap<String, String>,
        namespaces: &NamespaceMap,
        is_hidden: bool,
        byte_start: u64,
    ) -> Result<Option<Self>, XbrlError> {
        let kind = match element_name.local_name.as_str() {
            "nonFraction" => FactKind::Numeric,
            "nonNumeric" => FactKind::Text,
            _ => return Ok(None),
        };
        let Some(context_ref) = attrs.get("contextRef").cloned() else {
            return Ok(None);
        };
        let Some(name) = attrs.get("name") else {
            return Ok(None);
        };
        let name = parse_qname_str(name, namespaces, false)?;

        Ok(Some(Self {
            id: attrs.get("id").cloned(),
            name,
            context_ref,
            unit_ref: attrs.get("unitRef").cloned(),
            decimals: attrs
                .get("decimals")
                .map(|value| parse_decimals(value))
                .transpose()?,
            precision: attrs
                .get("precision")
                .map(|value| parse_precision(value))
                .transpose()?,
            lang: attrs.get("lang").or_else(|| attrs.get("xml:lang")).cloned(),
            nil: attrs
                .get("nil")
                .is_some_and(|value| value == "true" || value == "1"),
            text: String::new(),
            depth: 0,
            kind: Some(kind),
            inline_meta: Some(InlineMeta {
                format: attrs.get("format").cloned(),
                scale: attrs.get("scale").and_then(|scale| scale.parse().ok()),
                sign: attrs.get("sign").cloned(),
                target: attrs.get("target").cloned(),
                continued_from: attrs.get("continuedAt").cloned(),
                is_hidden,
            }),
            byte_start,
        }))
    }

    fn finish(self, byte_end: u64) -> RawFact {
        let value = if self.nil {
            RawFactValue::Nil
        } else if self.kind == Some(FactKind::Numeric)
            || self.unit_ref.is_some()
            || self.decimals.is_some()
            || self.precision.is_some()
        {
            RawFactValue::Numeric { raw: self.text }
        } else {
            RawFactValue::Text { value: self.text }
        };

        RawFact {
            id: self.id,
            name: self.name,
            value,
            context_ref: self.context_ref,
            unit_ref: self.unit_ref,
            decimals: self.decimals,
            precision: self.precision,
            lang: self.lang,
            inline_meta: self.inline_meta,
            byte_range: Some((self.byte_start, byte_end)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FactKind {
    Numeric,
    Text,
}

#[derive(Debug, Clone)]
struct ContinuationBuilder {
    id: Option<String>,
    continued_at: Option<String>,
    text: String,
}

#[derive(Debug, Clone)]
struct Continuation {
    text: String,
    continued_at: Option<String>,
}

fn resolve_continuations(facts: &mut [RawFact], continuations: &BTreeMap<String, Continuation>) {
    for fact in facts {
        let Some(start) = fact
            .inline_meta
            .as_ref()
            .and_then(|meta| meta.continued_from.clone())
        else {
            continue;
        };
        let suffix = continuation_text(start, continuations);
        if suffix.is_empty() {
            continue;
        }
        match &mut fact.value {
            RawFactValue::Numeric { raw } => raw.push_str(&suffix),
            RawFactValue::Text { value } => value.push_str(&suffix),
            RawFactValue::Nil => {}
        }
    }
}

fn continuation_text(start: String, continuations: &BTreeMap<String, Continuation>) -> String {
    let mut text = String::new();
    let mut current = Some(start);
    let mut seen = HashSet::new();

    while let Some(id) = current {
        if !seen.insert(id.clone()) {
            break;
        }
        let Some(continuation) = continuations.get(&id) else {
            break;
        };
        text.push_str(continuation.text.trim());
        current = continuation.continued_at.clone();
    }

    text
}

fn resolve_footnote_refs(
    footnotes: &mut [Footnote],
    locators: &BTreeMap<String, String>,
    footnote_labels: &BTreeMap<String, usize>,
    arcs: &[(String, String)],
) {
    for (from, to) in arcs {
        let (Some(fact_id), Some(footnote_index)) =
            (locators.get(from), footnote_labels.get(to).copied())
        else {
            continue;
        };
        if let Some(footnote) = footnotes.get_mut(footnote_index) {
            footnote.fact_refs.push(fact_id.clone());
        }
    }
}

fn parse_decimals(value: &str) -> Result<Decimals, XbrlError> {
    if value == "INF" {
        Ok(Decimals::Infinite)
    } else {
        value
            .parse::<i32>()
            .map(|n| Decimals::Value { n })
            .map_err(|_| XbrlError::Xml {
                message: format!("invalid decimals value: {value}"),
                byte_offset: None,
            })
    }
}

fn parse_precision(value: &str) -> Result<Precision, XbrlError> {
    if value == "INF" {
        Ok(Precision::Infinite)
    } else {
        value
            .parse::<u32>()
            .map(|n| Precision::Value { n })
            .map_err(|_| XbrlError::Xml {
                message: format!("invalid precision value: {value}"),
                byte_offset: None,
            })
    }
}

fn parse_normalized_decimal(
    raw: &str,
    inline_meta: Option<&InlineMeta>,
) -> Result<Decimal, XbrlError> {
    let mut normalized = normalize_numeric_text(raw, inline_meta);
    if normalized.is_empty() {
        return Err(XbrlError::InvalidDecimal {
            raw: raw.to_owned(),
        });
    }

    if inline_meta
        .and_then(|meta| meta.sign.as_deref())
        .is_some_and(|sign| sign == "-")
        && !normalized.starts_with('-')
    {
        normalized.insert(0, '-');
    }

    let mut decimal = normalized
        .parse::<Decimal>()
        .map_err(|_| XbrlError::InvalidDecimal {
            raw: raw.to_owned(),
        })?;

    if let Some(scale) = inline_meta.and_then(|meta| meta.scale) {
        decimal = apply_scale(decimal, scale);
    }

    Ok(decimal)
}

fn normalize_numeric_text(raw: &str, inline_meta: Option<&InlineMeta>) -> String {
    let raw_trim = raw.trim();
    let format = inline_meta.and_then(|meta| meta.format.as_deref());
    let format_local = format.and_then(format_local_name);

    // SEC's numwordsen transform: spelled-out English numbers ("one" → "1").
    if format_local.is_some_and(|f| f.eq_ignore_ascii_case("numwordsen"))
        && let Some(num) = english_words_to_number(raw_trim)
    {
        return num;
    }

    let mut text = raw_trim.replace([' ', '\u{a0}'], "");
    let is_parenthesized_negative = text.starts_with('(') && text.ends_with(')');
    if is_parenthesized_negative {
        text = text
            .trim_start_matches('(')
            .trim_end_matches(')')
            .to_owned();
    }

    let mut normalized = if is_zero_dash(&text, format) {
        "0".to_owned()
    } else if format_local.is_some_and(|local| local.eq_ignore_ascii_case("numcommadecimal")) {
        text.replace('.', "").replace(',', ".")
    } else {
        text.replace(',', "")
    };

    if is_parenthesized_negative && !normalized.starts_with('-') {
        normalized.insert(0, '-');
    }

    normalized
}

/// Convert SEC `ixt-sec:numwordsen` spelled-out numbers ("one", "twenty-three",
/// "one hundred million") into ASCII digit form. Returns `None` if the input
/// contains a word outside the supported vocabulary so the caller can decide
/// how to recover.
fn english_words_to_number(input: &str) -> Option<String> {
    let normalized = input.to_ascii_lowercase().replace('-', " ");
    let tokens: Vec<&str> = normalized
        .split_whitespace()
        .filter(|t| *t != "and")
        .collect();
    if tokens.is_empty() {
        return None;
    }

    let unit_value = |w: &str| -> Option<u64> {
        Some(match w {
            "zero" => 0,
            "one" => 1,
            "two" => 2,
            "three" => 3,
            "four" => 4,
            "five" => 5,
            "six" => 6,
            "seven" => 7,
            "eight" => 8,
            "nine" => 9,
            "ten" => 10,
            "eleven" => 11,
            "twelve" => 12,
            "thirteen" => 13,
            "fourteen" => 14,
            "fifteen" => 15,
            "sixteen" => 16,
            "seventeen" => 17,
            "eighteen" => 18,
            "nineteen" => 19,
            "twenty" => 20,
            "thirty" => 30,
            "forty" => 40,
            "fifty" => 50,
            "sixty" => 60,
            "seventy" => 70,
            "eighty" => 80,
            "ninety" => 90,
            _ => return None,
        })
    };

    let mut total: u64 = 0;
    let mut current: u64 = 0;
    for tok in &tokens {
        match *tok {
            "hundred" => {
                if current == 0 {
                    current = 1;
                }
                current *= 100;
            }
            "thousand" => {
                if current == 0 {
                    current = 1;
                }
                total += current * 1_000;
                current = 0;
            }
            "million" => {
                if current == 0 {
                    current = 1;
                }
                total += current * 1_000_000;
                current = 0;
            }
            "billion" => {
                if current == 0 {
                    current = 1;
                }
                total += current * 1_000_000_000;
                current = 0;
            }
            "trillion" => {
                if current == 0 {
                    current = 1;
                }
                total += current * 1_000_000_000_000;
                current = 0;
            }
            other => current += unit_value(other)?,
        }
    }
    Some((total + current).to_string())
}

fn is_zero_dash(text: &str, format: Option<&str>) -> bool {
    let is_dash = matches!(text, "-" | "−" | "－" | "—");
    is_dash
        && format.and_then(format_local_name).is_some_and(|local| {
            local.eq_ignore_ascii_case("zerodash")
                || local.eq_ignore_ascii_case("numdash")
                || local.eq_ignore_ascii_case("fixed-zero")
        })
}

fn format_local_name(format: &str) -> Option<&str> {
    format
        .rsplit_once(':')
        .map_or(Some(format), |(_, local)| Some(local))
}

/// Apply an iXBRL Transformations Registry text transform.
///
/// Implements a subset of the TR4 registry that EDINET filings use in
/// practice. Numeric transforms (numdotdecimal, numcommadecimal, zerodash,
/// numdash, fixed-zero) are handled inside [`normalize_numeric_text`]; this
/// function covers the text/date/boolean transforms applied to
/// `ix:nonNumeric` facts.
fn apply_text_transform(raw: &str, inline_meta: Option<&InlineMeta>) -> String {
    let Some(format) = inline_meta.and_then(|meta| meta.format.as_deref()) else {
        return raw.to_owned();
    };
    let Some(local) = format_local_name(format) else {
        return raw.to_owned();
    };
    match local.to_ascii_lowercase().as_str() {
        "dateyearmonthdaycjk" => transform_date_ymd_cjk(raw).unwrap_or_else(|| raw.to_owned()),
        "dateyearmonthcjk" => transform_date_ym_cjk(raw).unwrap_or_else(|| raw.to_owned()),
        "datemonthdaycjk" => transform_date_md_cjk(raw).unwrap_or_else(|| raw.to_owned()),
        "dateerayearmonthdayjp" => transform_date_era_ymd_jp(raw).unwrap_or_else(|| raw.to_owned()),
        "booleanfalse" | "fixed-false" => "false".to_owned(),
        "booleantrue" | "fixed-true" => "true".to_owned(),
        "fixed-empty" | "nocontent" => String::new(),
        _ => raw.to_owned(),
    }
}

/// Map a CJK or fullwidth digit string to ASCII digits, returning `None`
/// if a non-digit character (other than known CJK 0-9 numerals) is found.
fn to_ascii_digits(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        let digit = match ch {
            '0'..='9' => ch,
            '０'..='９' => char::from(b'0' + (ch as u32 - '０' as u32) as u8),
            '〇' | '零' => '0',
            '一' | '壱' => '1',
            '二' | '弐' => '2',
            '三' | '参' => '3',
            '四' => '4',
            '五' => '5',
            '六' => '6',
            '七' => '7',
            '八' => '8',
            '九' => '9',
            _ => return None,
        };
        out.push(digit);
    }
    Some(out)
}

fn transform_date_ymd_cjk(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (year, rest) = trimmed.split_once('年')?;
    let (month, rest) = rest.split_once('月')?;
    let day = rest.strip_suffix('日')?;
    let year = to_ascii_digits(year.trim())?;
    let month = to_ascii_digits(month.trim())?;
    let day = to_ascii_digits(day.trim())?;
    Some(format!("{:0>4}-{:0>2}-{:0>2}", year, month, day,))
}

fn transform_date_ym_cjk(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (year, rest) = trimmed.split_once('年')?;
    let month = rest.strip_suffix('月')?;
    let year = to_ascii_digits(year.trim())?;
    let month = to_ascii_digits(month.trim())?;
    Some(format!("{:0>4}-{:0>2}", year, month))
}

fn transform_date_md_cjk(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (month, rest) = trimmed.split_once('月')?;
    let day = rest.strip_suffix('日')?;
    let month = to_ascii_digits(month.trim())?;
    let day = to_ascii_digits(day.trim())?;
    Some(format!("--{:0>2}-{:0>2}", month, day))
}

/// Japanese era date: e.g. `令和7年6月26日` → `2025-06-26`.
fn transform_date_era_ymd_jp(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (era, rest) = era_prefix(trimmed)?;
    let (year, rest) = rest.split_once('年')?;
    let (month, rest) = rest.split_once('月')?;
    let day = rest.strip_suffix('日')?;
    let year_n: u32 = to_ascii_digits(year.trim())?.parse().ok()?;
    let month: u32 = to_ascii_digits(month.trim())?.parse().ok()?;
    let day: u32 = to_ascii_digits(day.trim())?.parse().ok()?;
    let base = match era {
        "令和" => 2018,
        "平成" => 1988,
        "昭和" => 1925,
        "大正" => 1911,
        "明治" => 1867,
        _ => return None,
    };
    let year = base + year_n;
    Some(format!("{year}-{:0>2}-{:0>2}", month, day))
}

fn era_prefix(s: &str) -> Option<(&str, &str)> {
    for era in ["令和", "平成", "昭和", "大正", "明治"] {
        if let Some(rest) = s.strip_prefix(era) {
            return Some((era, rest));
        }
    }
    None
}

fn apply_scale(mut value: Decimal, scale: i32) -> Decimal {
    let ten = Decimal::new(10, 0);
    if scale >= 0 {
        for _ in 0..scale {
            value *= ten;
        }
    } else {
        for _ in scale..0 {
            value /= ten;
        }
    }
    value
}

#[derive(Debug, Clone)]
enum TextTarget {
    Identifier,
    Instant,
    StartDate,
    EndDate,
    ExplicitMember {
        dimension: QName,
        container: ContextContainer,
    },
    Measure {
        denominator: bool,
    },
    Fact,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_XBRL: &str = r#"
<xbrli:xbrl
    xmlns:xbrli="http://www.xbrl.org/2003/instance"
    xmlns:link="http://www.xbrl.org/2003/linkbase"
    xmlns:xlink="http://www.w3.org/1999/xlink"
    xmlns:xbrldi="http://xbrl.org/2006/xbrldi"
    xmlns:iso4217="http://www.xbrl.org/2003/iso4217"
    xmlns:ex="http://example.com/taxonomy">
  <link:schemaRef xlink:href="example.xsd" xlink:type="simple"/>
  <xbrli:context id="ctx1">
    <xbrli:entity>
      <xbrli:identifier scheme="http://example.com/entity">E00001</xbrli:identifier>
      <xbrli:segment>
        <xbrldi:explicitMember dimension="ex:ConsolidatedAxis">ex:ConsolidatedMember</xbrldi:explicitMember>
      </xbrli:segment>
    </xbrli:entity>
    <xbrli:period>
      <xbrli:instant>2025-03-31</xbrli:instant>
    </xbrli:period>
  </xbrli:context>
  <xbrli:unit id="JPY">
    <xbrli:measure>iso4217:JPY</xbrli:measure>
  </xbrli:unit>
  <ex:NetSales id="f1" contextRef="ctx1" unitRef="JPY" decimals="0">1000000</ex:NetSales>
  <ex:CompanyName contextRef="ctx1" xml:lang="ja">Example株式会社</ex:CompanyName>
</xbrli:xbrl>
"#;

    const ADVANCED_XBRL: &str = r#"
<xbrli:xbrl
    xmlns:xbrli="http://www.xbrl.org/2003/instance"
    xmlns:xbrldi="http://xbrl.org/2006/xbrldi"
    xmlns:iso4217="http://www.xbrl.org/2003/iso4217"
    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
    xmlns:ex="http://example.com/taxonomy">
  <xbrli:context id="ctx_typed">
    <xbrli:entity>
      <xbrli:identifier scheme="http://example.com/entity">E00001</xbrli:identifier>
      <xbrli:scenario>
        <xbrldi:typedMember dimension="ex:StoreAxis"><ex:StoreCode>001</ex:StoreCode></xbrldi:typedMember>
      </xbrli:scenario>
    </xbrli:entity>
    <xbrli:period>
      <xbrli:startDate>2024-04-01</xbrli:startDate>
      <xbrli:endDate>2025-03-31</xbrli:endDate>
    </xbrli:period>
  </xbrli:context>
  <xbrli:unit id="JPYPerShare">
    <xbrli:divide>
      <xbrli:unitNumerator>
        <xbrli:measure>iso4217:JPY</xbrli:measure>
      </xbrli:unitNumerator>
      <xbrli:unitDenominator>
        <xbrli:measure>xbrli:shares</xbrli:measure>
      </xbrli:unitDenominator>
    </xbrli:divide>
  </xbrli:unit>
  <ex:EarningsPerShare contextRef="ctx_typed" unitRef="JPYPerShare" decimals="INF">123.45</ex:EarningsPerShare>
  <ex:OptionalDisclosure contextRef="ctx_typed" xsi:nil="true"/>
</xbrli:xbrl>
"#;

    const INLINE_XBRL: &str = r#"
<html
    xmlns="http://www.w3.org/1999/xhtml"
    xmlns:ix="http://www.xbrl.org/2013/inlineXBRL"
    xmlns:xbrli="http://www.xbrl.org/2003/instance"
    xmlns:iso4217="http://www.xbrl.org/2003/iso4217"
    xmlns:ex="http://example.com/taxonomy">
  <head>
    <ix:hidden>
      <ix:resources>
        <xbrli:context id="c1">
          <xbrli:entity>
            <xbrli:identifier scheme="http://example.com/entity">E00001</xbrli:identifier>
          </xbrli:entity>
          <xbrli:period>
            <xbrli:instant>2025-03-31</xbrli:instant>
          </xbrli:period>
        </xbrli:context>
        <xbrli:unit id="JPY">
          <xbrli:measure>iso4217:JPY</xbrli:measure>
        </xbrli:unit>
        <ix:nonFraction name="ex:HiddenLoss" contextRef="c1" unitRef="JPY" decimals="0" sign="-">42</ix:nonFraction>
      </ix:resources>
    </ix:hidden>
  </head>
  <body>
    <p>Revenue: <ix:nonFraction name="ex:Revenue" contextRef="c1" unitRef="JPY" decimals="-3" scale="3">1,234</ix:nonFraction></p>
    <p>EU amount: <ix:nonFraction name="ex:EuAmount" contextRef="c1" unitRef="JPY" decimals="2" format="ixt:numcommadecimal">1.234,56</ix:nonFraction></p>
    <p>Dash amount: <ix:nonFraction name="ex:DashAmount" contextRef="c1" unitRef="JPY" decimals="0" format="ixt:zerodash">-</ix:nonFraction></p>
    <p>Parenthesized loss: <ix:nonFraction name="ex:ParenthesizedLoss" contextRef="c1" unitRef="JPY" decimals="0">(1,234)</ix:nonFraction></p>
    <p>Name: <ix:nonNumeric name="ex:CompanyName" contextRef="c1" xml:lang="ja" continuedAt="name-cont-1">Example</ix:nonNumeric></p>
    <ix:continuation id="name-cont-1">株式会社</ix:continuation>
  </body>
</html>
"#;

    const FOOTNOTE_XBRL: &str = r##"
<xbrli:xbrl
    xmlns:xbrli="http://www.xbrl.org/2003/instance"
    xmlns:link="http://www.xbrl.org/2003/linkbase"
    xmlns:xlink="http://www.w3.org/1999/xlink"
    xmlns:iso4217="http://www.xbrl.org/2003/iso4217"
    xmlns:ex="http://example.com/taxonomy">
  <xbrli:context id="ctx1">
    <xbrli:entity>
      <xbrli:identifier scheme="http://example.com/entity">E00001</xbrli:identifier>
    </xbrli:entity>
    <xbrli:period>
      <xbrli:instant>2025-03-31</xbrli:instant>
    </xbrli:period>
  </xbrli:context>
  <xbrli:unit id="JPY">
    <xbrli:measure>iso4217:JPY</xbrli:measure>
  </xbrli:unit>
  <ex:NetSales id="f1" contextRef="ctx1" unitRef="JPY" decimals="0">1000</ex:NetSales>
  <link:footnoteLink xlink:type="extended">
    <link:loc xlink:type="locator" xlink:href="#f1" xlink:label="fact_f1"/>
    <link:footnote xlink:type="resource" xlink:label="fn1" xlink:role="http://www.xbrl.org/2003/role/footnote" xml:lang="en">Includes overseas sales.</link:footnote>
    <link:footnoteArc xlink:type="arc" xlink:arcrole="http://www.xbrl.org/2003/arcrole/fact-footnote" xlink:from="fact_f1" xlink:to="fn1"/>
  </link:footnoteLink>
</xbrli:xbrl>
"##;

    struct Labels;

    impl TaxonomyResolver for Labels {
        fn label(&self, name: &QName, _role: Option<&str>, _lang: Option<&str>) -> Option<String> {
            (name.local_name == "NetSales").then(|| "Net sales".to_owned())
        }
    }

    #[test]
    fn parse_minimal_xbrl_instance() {
        let instance = parse_instance(MINIMAL_XBRL.as_bytes()).unwrap();

        assert_eq!(instance.schema_refs, vec!["example.xsd"]);
        assert_eq!(instance.contexts.len(), 1);
        assert_eq!(instance.units.len(), 1);
        assert_eq!(instance.facts.len(), 2);

        let context = instance.contexts.get("ctx1").unwrap();
        assert_eq!(context.entity.identifier, "E00001");
        assert_eq!(
            context.period,
            Period::Instant {
                date: "2025-03-31".into()
            }
        );
        assert_eq!(context.segment.len(), 1);

        let unit = instance.units.get("JPY").unwrap();
        assert_eq!(unit.numerator[0].local_name, "JPY");

        let fact = &instance.facts[0];
        assert_eq!(fact.name.local_name, "NetSales");
        assert_eq!(fact.context_ref, "ctx1");
        assert_eq!(fact.unit_ref.as_deref(), Some("JPY"));
        assert_eq!(fact.decimals, Some(Decimals::Value { n: 0 }));
        assert_eq!(
            fact.value,
            RawFactValue::Numeric {
                raw: "1000000".into()
            }
        );
    }

    #[test]
    fn normalize_minimal_fact() {
        let instance = parse_instance(MINIMAL_XBRL.as_bytes()).unwrap();
        let normalized = normalize_facts(&instance, &Labels, "doc1");

        assert_eq!(normalized.len(), 2);
        let fact = normalized[0].as_ref().unwrap();
        assert_eq!(fact.label.as_deref(), Some("Net sales"));
        assert_eq!(
            fact.value,
            NormalizedValue::Numeric {
                raw: "1000000".into(),
                decimal: Some(Decimal::new(1_000_000, 0)),
                decimals: Some(Decimals::Value { n: 0 }),
            }
        );
        assert_eq!(fact.dimensions.len(), 1);
        assert_eq!(fact.dimensions[0].dimension.local_name, "ConsolidatedAxis");
        assert_eq!(fact.provenance.doc_id, "doc1");
    }

    #[test]
    fn normalize_reports_missing_context() {
        let instance = InstanceDocument {
            schema_refs: vec![],
            contexts: BTreeMap::new(),
            units: BTreeMap::new(),
            facts: vec![RawFact {
                id: None,
                name: QName {
                    namespace_uri: None,
                    prefix: None,
                    local_name: "NetSales".into(),
                },
                value: RawFactValue::Numeric { raw: "1".into() },
                context_ref: "missing".into(),
                unit_ref: None,
                decimals: None,
                precision: None,
                lang: None,
                inline_meta: None,
                byte_range: None,
            }],
            footnotes: vec![],
        };

        let result = normalize_facts(&instance, &Labels, "doc1").remove(0);
        assert!(matches!(
            result,
            Err(XbrlError::MissingContext { context_ref }) if context_ref == "missing"
        ));
    }

    #[test]
    fn parse_typed_dimension_divide_unit_and_nil_fact() {
        let instance = parse_instance(ADVANCED_XBRL.as_bytes()).unwrap();

        let context = instance.contexts.get("ctx_typed").unwrap();
        assert_eq!(
            context.period,
            Period::Duration {
                start: "2024-04-01".into(),
                end: "2025-03-31".into()
            }
        );
        assert_eq!(context.scenario.len(), 1);
        match &context.scenario[0] {
            ContextElement::TypedDimension { dimension, raw_xml } => {
                assert_eq!(dimension.local_name, "StoreAxis");
                assert!(raw_xml.contains("<ex:StoreCode>001</ex:StoreCode>"));
            }
            other => panic!("expected typed dimension, got {other:?}"),
        }

        let unit = instance.units.get("JPYPerShare").unwrap();
        assert_eq!(unit.numerator[0].local_name, "JPY");
        assert_eq!(unit.denominator[0].local_name, "shares");

        assert_eq!(instance.facts.len(), 2);
        assert_eq!(instance.facts[0].decimals, Some(Decimals::Infinite));
        assert_eq!(instance.facts[1].value, RawFactValue::Nil);
    }

    #[test]
    fn normalize_preserves_typed_dimension_and_inf_decimals() {
        let instance = parse_instance(ADVANCED_XBRL.as_bytes()).unwrap();
        let normalized = normalize_facts(&instance, &Labels, "doc2");
        let fact = normalized[0].as_ref().unwrap();

        assert_eq!(
            fact.value,
            NormalizedValue::Numeric {
                raw: "123.45".into(),
                decimal: Some(Decimal::new(12345, 2)),
                decimals: Some(Decimals::Infinite),
            }
        );
        assert_eq!(fact.dimensions.len(), 1);
        assert!(matches!(
            &fact.dimensions[0].member,
            DimensionMember::Typed { raw_xml } if raw_xml.contains("StoreCode")
        ));
    }

    #[test]
    fn parse_inline_xbrl_non_fraction_and_non_numeric() {
        let instance = parse_instance(INLINE_XBRL.as_bytes()).unwrap();

        assert_eq!(instance.contexts.len(), 1);
        assert_eq!(instance.units.len(), 1);
        assert_eq!(instance.facts.len(), 6);

        let hidden = &instance.facts[0];
        assert_eq!(hidden.name.local_name, "HiddenLoss");
        assert!(hidden.inline_meta.as_ref().unwrap().is_hidden);
        assert_eq!(
            hidden.inline_meta.as_ref().unwrap().sign.as_deref(),
            Some("-")
        );

        let revenue = &instance.facts[1];
        assert_eq!(revenue.name.local_name, "Revenue");
        assert_eq!(
            revenue.value,
            RawFactValue::Numeric {
                raw: "1,234".into()
            }
        );
        assert_eq!(revenue.inline_meta.as_ref().unwrap().scale, Some(3));

        let name = &instance.facts[5];
        assert_eq!(name.name.local_name, "CompanyName");
        assert_eq!(
            name.value,
            RawFactValue::Text {
                value: "Example株式会社".into()
            }
        );
        assert_eq!(
            name.inline_meta
                .as_ref()
                .and_then(|meta| meta.continued_from.as_deref()),
            Some("name-cont-1")
        );
    }

    #[test]
    fn normalize_inline_xbrl_applies_sign_and_scale() {
        let instance = parse_instance(INLINE_XBRL.as_bytes()).unwrap();
        let normalized = normalize_facts(&instance, &Labels, "inline-doc");

        assert_eq!(
            normalized[0].as_ref().unwrap().value,
            NormalizedValue::Numeric {
                raw: "42".into(),
                decimal: Some(Decimal::new(-42, 0)),
                decimals: Some(Decimals::Value { n: 0 }),
            }
        );
        assert_eq!(
            normalized[1].as_ref().unwrap().value,
            NormalizedValue::Numeric {
                raw: "1,234".into(),
                decimal: Some(Decimal::new(1_234_000, 0)),
                decimals: Some(Decimals::Value { n: -3 }),
            }
        );
        assert_eq!(
            normalized[2].as_ref().unwrap().value,
            NormalizedValue::Numeric {
                raw: "1.234,56".into(),
                decimal: Some(Decimal::new(123_456, 2)),
                decimals: Some(Decimals::Value { n: 2 }),
            }
        );
        assert_eq!(
            normalized[3].as_ref().unwrap().value,
            NormalizedValue::Numeric {
                raw: "-".into(),
                decimal: Some(Decimal::new(0, 0)),
                decimals: Some(Decimals::Value { n: 0 }),
            }
        );
        assert_eq!(
            normalized[4].as_ref().unwrap().value,
            NormalizedValue::Numeric {
                raw: "(1,234)".into(),
                decimal: Some(Decimal::new(-1234, 0)),
                decimals: Some(Decimals::Value { n: 0 }),
            }
        );
    }

    #[test]
    fn transform_dates_to_iso() {
        let meta = |fmt: &str| {
            Some(InlineMeta {
                format: Some(fmt.to_owned()),
                scale: None,
                sign: None,
                target: None,
                continued_from: None,
                is_hidden: false,
            })
        };

        assert_eq!(
            apply_text_transform("2025年6月26日", meta("ixt:dateyearmonthdaycjk").as_ref()),
            "2025-06-26"
        );
        assert_eq!(
            apply_text_transform(
                "２０２５年６月２６日",
                meta("ixt:dateyearmonthdaycjk").as_ref()
            ),
            "2025-06-26"
        );
        assert_eq!(
            apply_text_transform("2025年3月", meta("ixt:dateyearmonthcjk").as_ref()),
            "2025-03"
        );
        assert_eq!(
            apply_text_transform("令和7年6月26日", meta("ixt:dateerayearmonthdayjp").as_ref()),
            "2025-06-26"
        );
        assert_eq!(
            apply_text_transform("anything", meta("ixt:fixed-false").as_ref()),
            "false"
        );
        assert_eq!(
            apply_text_transform("anything", meta("ixt:nocontent").as_ref()),
            ""
        );

        // Unknown format → passthrough
        assert_eq!(
            apply_text_transform("raw", meta("ixt:made-up").as_ref()),
            "raw"
        );
        // Malformed input → passthrough
        assert_eq!(
            apply_text_transform("not a date", meta("ixt:dateyearmonthdaycjk").as_ref()),
            "not a date"
        );
    }

    #[test]
    fn parse_basic_fact_footnote_link() {
        let instance = parse_instance(FOOTNOTE_XBRL.as_bytes()).unwrap();

        assert_eq!(instance.facts.len(), 1);
        assert_eq!(instance.footnotes.len(), 1);
        let footnote = &instance.footnotes[0];
        assert_eq!(footnote.content, "Includes overseas sales.");
        assert_eq!(footnote.lang.as_deref(), Some("en"));
        assert_eq!(footnote.fact_refs, vec!["f1"]);
    }
}
