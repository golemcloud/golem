use crate::rib_source_span::SourceSpan;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, Eq, PartialOrd, Ord)]
pub enum TypeOrigin {
    // the first OriginatedAt (if it's TypeOrigin::Multiple) at
    // this level is the source span of the expression to which this
    // type origin is attached
    OriginatedAt(SourceSpan),
    // If an expression has an inferred type that was originated by default
    // this becomes the first origin. So to see if an expression's inferred type
    // was because it was `Default` then this is the first origin
    Default,
    NoOrigin,
    Declared(SourceSpan),
    Multiple(Vec<TypeOrigin>),
    PatternMatch(SourceSpan),
}

impl TypeOrigin {
    // Since OriginatedAt is usually tagged against every expression
    // in the beginning itself, this is actually always Some(span), where
    // the span is the original span of the expression this type origin is attached to;
    pub fn source_span(&self) -> Option<SourceSpan> {
        let mut stack = vec![self];

        while let Some(origin) = stack.pop() {
            match origin {
                TypeOrigin::OriginatedAt(span) => return Some(span.clone()),
                TypeOrigin::PatternMatch(span) => return Some(span.clone()),
                TypeOrigin::Declared(span) => return Some(span.clone()),
                TypeOrigin::Multiple(origins) => {
                    stack.extend(origins.iter());
                }
                TypeOrigin::Default | TypeOrigin::NoOrigin => {}
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
                    if best.is_none()
                        || matches!(
                            best,
                            Some(TypeOrigin::Default)
                                | Some(TypeOrigin::NoOrigin)
                                | Some(TypeOrigin::OriginatedAt(_))
                        )
                    {
                        best = Some(TypeOrigin::Declared(span.clone()));
                    }
                }
                TypeOrigin::Default => {
                    if best.is_none()
                        || matches!(
                            best,
                            Some(TypeOrigin::NoOrigin) | Some(TypeOrigin::OriginatedAt(_))
                        )
                    {
                        best = Some(TypeOrigin::Default);
                    }
                }
                TypeOrigin::OriginatedAt(source_span) => {
                    if best.is_none() {
                        best = Some(TypeOrigin::OriginatedAt(source_span.clone()));
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
            TypeOrigin::OriginatedAt(_) => false,
            TypeOrigin::NoOrigin => false,
            TypeOrigin::Declared(_) => false,
            TypeOrigin::Multiple(origins) => {
                // if the origin was originated as part of "Default", then it will exist in the very beginning
                origins.first().is_some_and(|origin| origin.is_default())
            }

            TypeOrigin::PatternMatch(_) => false,
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
            TypeOrigin::OriginatedAt(span) => {
                5.hash(state);
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
