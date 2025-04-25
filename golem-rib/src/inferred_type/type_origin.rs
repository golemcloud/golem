use crate::rib_source_span::SourceSpan;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Eq, PartialOrd, Ord)]
pub enum TypeOrigin {
    Default,
    NoOrigin,
    Declared(SourceSpan),
    Multiple(Vec<TypeOrigin>),
}

impl TypeOrigin {
    pub fn is_none(&self) -> bool {
        matches!(self, TypeOrigin::NoOrigin)
    }

    pub fn is_default(&self) -> bool {
        matches!(self, TypeOrigin::Default)
    }

    pub fn add_origin(&mut self, new_origin: TypeOrigin) {
        match self {
            TypeOrigin::NoOrigin => *self = new_origin,
            TypeOrigin::Multiple(origins) => {
                if !origins.contains(&new_origin) {
                    origins.push(new_origin);
                }
            }
            _ => {
                *self = TypeOrigin::Multiple(vec![self.clone(), new_origin]);
            }
        }
    }
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
            TypeOrigin::Declared(span) => {
                3.hash(state);
                span.hash(state);
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
