use std::hash::{Hash, Hasher};
use crate::rib_source_span::SourceSpan;

#[derive(Debug, Clone, Eq, PartialOrd, Ord)]
pub enum TypeOrigin {
    Default,
    NoOrigin,
    Declared(SourceSpan),
    Multiple(Vec<TypeOrigin>),
}

impl Hash for TypeOrigin {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            TypeOrigin::Default => 0.hash(state),
            TypeOrigin::NoOrigin => 1.hash(state),
            TypeOrigin::Multiple(origins) => {
                2.hash(state);
                origins.hash(state);
            }
        }
    }
}

// TypeOrigin doesn't matter in any equality logic
impl PartialEq for TypeOrigin {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}
