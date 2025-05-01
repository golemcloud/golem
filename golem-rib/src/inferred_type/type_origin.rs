// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::rib_source_span::SourceSpan;
use std::collections::VecDeque;
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
    // To not interfere with equality logic of types
    // yet if we need to specifically compare the type origins
    pub fn eq(&self, other: &TypeOrigin) -> bool {
        match (self, other) {
            (TypeOrigin::NoOrigin, TypeOrigin::NoOrigin) => true,
            (TypeOrigin::Default, TypeOrigin::Default) => true,
            (TypeOrigin::Declared(source_span1), TypeOrigin::Declared(source_span2)) => {
                source_span1.is_equal(source_span2)
            }
            (TypeOrigin::PatternMatch(source_span1), TypeOrigin::PatternMatch(source_span2)) => {
                source_span1.is_equal(source_span2)
            }
            (TypeOrigin::Multiple(origins1), TypeOrigin::Multiple(origins2)) => {
                if origins1.len() != origins2.len() {
                    false
                } else {
                    for (origin1, origin2) in origins1.iter().zip(origins2.iter()) {
                        if !origin1.eq(origin2) {
                            return false;
                        }
                    }
                    true
                }
            }
            (TypeOrigin::OriginatedAt(source_span1), TypeOrigin::OriginatedAt(source_span2)) => {
                source_span1.is_equal(source_span2)
            }

            _ => false,
        }
    }

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

    // It simply picks up the origin based on a priority
    pub fn critical_origin(&self) -> TypeOrigin {
        let mut queue = VecDeque::new();
        queue.push_back(self);

        let mut best: Option<TypeOrigin> = None;

        while let Some(current) = queue.pop_front() {
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
            TypeOrigin::NoOrigin => match new_origin {
                TypeOrigin::Multiple(origins) => TypeOrigin::Multiple(origins),
                _ => new_origin,
            },

            TypeOrigin::Multiple(existing_origins) => {
                let mut updated = existing_origins.clone();

                match new_origin {
                    TypeOrigin::Multiple(new_origins) => {
                        for origin in new_origins {
                            if !updated.iter().any(|o| o.eq(&origin)) {
                                updated.push(origin);
                            }
                        }
                    }
                    origin => {
                        if !updated.iter().any(|o| o.eq(&origin)) {
                            updated.push(origin);
                        }
                    }
                }

                TypeOrigin::Multiple(updated)
            }

            other => match new_origin {
                TypeOrigin::Multiple(origins) => {
                    let mut updated = vec![other.clone()];

                    for origin in origins {
                        if !updated.iter().any(|o| o.eq(&origin)) {
                            updated.push(origin);
                        }
                    }

                    if updated.len() == 1 {
                        updated.pop().unwrap()
                    } else {
                        TypeOrigin::Multiple(updated)
                    }
                }

                origin if origin.eq(other) => other.clone(),

                origin => TypeOrigin::Multiple(vec![other.clone(), origin]),
            },
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

#[cfg(test)]
mod tests {
    use crate::inferred_type::TypeOrigin;
    use crate::rib_source_span::{SourcePosition, SourceSpan};
    use test_r::test;

    #[test]
    fn test_origin_add_1() {
        let existing_origin = TypeOrigin::NoOrigin;
        let result = existing_origin.add_origin(TypeOrigin::Default);

        assert_eq!(result, TypeOrigin::Default);
    }

    #[test]
    fn test_origin_add_2() {
        let existing_origin = TypeOrigin::Default;

        let result = existing_origin.add_origin(TypeOrigin::Default);

        assert!(result.eq(&TypeOrigin::Default));
    }

    #[test]
    fn test_origin_add_3() {
        let existing_origin = TypeOrigin::Default;

        let result = existing_origin.add_origin(TypeOrigin::OriginatedAt(SourceSpan::default()));

        let expected = TypeOrigin::Multiple(vec![
            TypeOrigin::Default,
            TypeOrigin::OriginatedAt(SourceSpan::default()),
        ]);

        assert!(result.eq(&expected));
    }

    #[test]
    fn test_origin_add_4() {
        let type_origin1 = TypeOrigin::Multiple(vec![
            TypeOrigin::Default,
            TypeOrigin::OriginatedAt(SourceSpan::new(
                SourcePosition::new(2, 37),
                SourcePosition::new(2, 38),
            )),
            TypeOrigin::OriginatedAt(SourceSpan::new(
                SourcePosition::new(2, 37),
                SourcePosition::new(2, 38),
            )),
        ]);

        let type_origin2 = TypeOrigin::Declared(SourceSpan::new(
            SourcePosition::new(2, 11),
            SourcePosition::new(2, 39),
        ));

        let result = type_origin1.add_origin(type_origin2.clone());

        let expected = TypeOrigin::Multiple(vec![
            TypeOrigin::Default,
            TypeOrigin::OriginatedAt(SourceSpan::new(
                SourcePosition::new(2, 37),
                SourcePosition::new(2, 38),
            )),
            TypeOrigin::OriginatedAt(SourceSpan::new(
                SourcePosition::new(2, 37),
                SourcePosition::new(2, 38),
            )),
            type_origin2,
        ]);

        assert!(result.eq(&expected));
    }
}
