use crate::error::XbrlError;
use crate::types::{InstanceDocument, NormalizedFact};

pub fn parse_instance(_input: &[u8]) -> Result<InstanceDocument, XbrlError> {
    todo!("XBRL instance parser")
}

pub trait TaxonomyResolver {
    fn label(&self, name: &crate::types::QName, role: Option<&str>, lang: Option<&str>)
        -> Option<String>;
}

pub fn normalize_facts(
    _instance: &InstanceDocument,
    _taxonomy: &dyn TaxonomyResolver,
    _doc_id: &str,
) -> Vec<Result<NormalizedFact, XbrlError>> {
    todo!("fact normalization pipeline")
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {
        assert!(true);
    }
}
