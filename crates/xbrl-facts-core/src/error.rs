use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum XbrlError {
    #[error("XML parse error at byte {byte_offset:?}: {message}")]
    Xml {
        message: String,
        byte_offset: Option<u64>,
    },

    #[error("missing context: {context_ref}")]
    MissingContext { context_ref: String },

    #[error("missing unit: {unit_ref}")]
    MissingUnit { unit_ref: String },

    #[error("invalid decimal value: {raw}")]
    InvalidDecimal { raw: String },

    #[error("unsupported inline transform: {format}")]
    UnsupportedInlineTransform { format: String },

    #[error("conflicting {kind} '{id}' across IXDS members")]
    IxdsConflict { kind: &'static str, id: String },

    #[error("not implemented: {feature}")]
    NotImplemented { feature: &'static str },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("XML error: {0}")]
    QuickXml(#[from] quick_xml::Error),

    #[error("XML attribute error: {0}")]
    XmlAttr(#[from] quick_xml::events::attributes::AttrError),
}

impl From<rust_decimal::Error> for XbrlError {
    fn from(e: rust_decimal::Error) -> Self {
        Self::InvalidDecimal { raw: e.to_string() }
    }
}
