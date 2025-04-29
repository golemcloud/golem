use crate::rib_source_span::SourceSpan;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, Eq, PartialOrd, Ord)]
pub enum TypeOrigin {
    Default,
    NoOrigin,
    Declared(SourceSpan),
    Multiple(Vec<TypeOrigin>),
    PatternMatch(SourceSpan),
}

impl TypeOrigin {
    pub fn source_span(&self) -> Option<SourceSpan> {
        let mut stack = vec![self];

        while let Some(origin) = stack.pop() {
            match origin {
                TypeOrigin::PatternMatch(span) => return Some(span.clone()),
                TypeOrigin::Declared(span) => return Some(span.clone()),
                TypeOrigin::Multiple(origins) => {
                    stack.extend(origins.iter());
                }
                TypeOrigin::Default | TypeOrigin::NoOrigin => {
                }
            }
        }

        None
    }

    pub fn is_none(&self) -> bool {
        matches!(self, TypeOrigin::NoOrigin)
    }

    pub fn immediate_critical_origin(&self) -> TypeOrigin {
        let mut queue = vec![self];

        let mut best: Option<TypeOrigin> = None;

        while let Some(current) = queue.pop() {
            match current {
                TypeOrigin::PatternMatch(span) => {
                    return TypeOrigin::PatternMatch(span.clone());
                }
                TypeOrigin::Declared(span) => {
                    if best.is_none() || matches!(best, Some(TypeOrigin::Default) | Some(TypeOrigin::NoOrigin)) {
                        best = Some(TypeOrigin::Declared(span.clone()));
                    }
                }
                TypeOrigin::Default => {
                    if best.is_none() || matches!(best, Some(TypeOrigin::NoOrigin)) {
                        best = Some(TypeOrigin::Default);
                    }
                }
                TypeOrigin::NoOrigin => {
                    if best.is_none() {
                        best = Some(TypeOrigin::NoOrigin);
                    }
                }
                TypeOrigin::Multiple(origins) => {
                    queue.extend(origins.iter());
                }
            }
        }

        best.unwrap_or(TypeOrigin::NoOrigin)
    }

    pub fn is_default(&self) -> bool {
        match self {
            TypeOrigin::Default => true,
            TypeOrigin::NoOrigin => false,
            TypeOrigin::Declared(_) => false,
            TypeOrigin::Multiple(origins) => {
                origins.first().is_some_and(|origin| origin.is_default())
            }

            TypeOrigin::PatternMatch(_) => false,
        }
    }

    pub fn root(&self) -> TypeOrigin {
        match self {
            TypeOrigin::Default => TypeOrigin::Default,
            TypeOrigin::NoOrigin => TypeOrigin::NoOrigin,
            TypeOrigin::Declared(_) => self.clone(),
            TypeOrigin::Multiple(origins) => {
                origins.first().map_or(self.clone(), |origin| origin.root())
            }
            TypeOrigin::PatternMatch(_) => self.clone(),
        }
    }

    pub fn add_origin(&self, new_origin: TypeOrigin) -> TypeOrigin {
        match self {
            TypeOrigin::NoOrigin => new_origin,
            TypeOrigin::Multiple(origins) => {
                let mut new_origins = origins.clone();
                if !origins.contains(&new_origin) {
                    new_origins.push(new_origin);
                }

                TypeOrigin::Multiple(new_origins)
            }
            _ => TypeOrigin::Multiple(vec![self.clone(), new_origin]),
        }
    }
}

// impl Debug for TypeOrigin {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         match self {
//             TypeOrigin::Default => write!(f, "Default"),
//             TypeOrigin::NoOrigin => write!(f, "NoOrigin"),
//             TypeOrigin::Declared(_) => write!(f, "Declared"),
//             TypeOrigin::Multiple(_) => write!(f, "Multiple<Origin>"),
//             TypeOrigin::PatternMatch(_) => write!(f, "PatternMatch"),
//         }
//     }
// }

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
            TypeOrigin::PatternMatch(span) => {
                4.hash(state);
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
