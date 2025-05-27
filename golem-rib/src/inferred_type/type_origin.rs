// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::rib_source_span::SourceSpan;
use bigdecimal::BigDecimal;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};

#[derive(Clone, Eq, PartialOrd, Ord)]
pub enum TypeOrigin {
    OriginatedAt(SourceSpan),
    Default(DefaultType),
    NoOrigin,
    Declared(SourceSpan),
    Multiple(Vec<TypeOrigin>),
}

impl Debug for TypeOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<TypeOrigin>")
    }
}

#[derive(Clone, Debug, Eq, PartialOrd, Ord)]
pub enum DefaultType {
    String,
    F64,
    S32,
}

impl From<&BigDecimal> for DefaultType {
    fn from(value: &BigDecimal) -> Self {
        if value.fractional_digit_count() <= 0 {
            // Rust inspired
            // https://github.com/rust-lang/rfcs/blob/master/text/0212-restore-int-fallback.md#rationale-for-the-choice-of-defaulting-to-i32
            DefaultType::S32
        } else {
            // more precision, almost same perf as f32
            DefaultType::F64
        }
    }
}

impl DefaultType {
    pub fn eq(&self, other: &DefaultType) -> bool {
        matches!(
            (self, other),
            (DefaultType::String, DefaultType::String)
                | (DefaultType::F64, DefaultType::F64)
                | (DefaultType::S32, DefaultType::S32)
        )
    }
}

impl PartialEq for DefaultType {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl TypeOrigin {
    // TypeOrigin need separate `eq` since Origin is always part of
    // a InferredType and their equality shouldn't be affected even their origins are different.
    pub fn eq(&self, other: &TypeOrigin) -> bool {
        match (self, other) {
            (TypeOrigin::NoOrigin, TypeOrigin::NoOrigin) => true,
            (TypeOrigin::Default(df1), TypeOrigin::Default(df2)) => df1.eq(df2),
            (TypeOrigin::Declared(source_span1), TypeOrigin::Declared(source_span2)) => {
                source_span1.eq(source_span2)
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
                source_span1.eq(source_span2)
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
                TypeOrigin::Declared(span) => return Some(span.clone()),
                TypeOrigin::Multiple(origins) => {
                    stack.extend(origins.iter());
                }
                TypeOrigin::Default(_) | TypeOrigin::NoOrigin => {}
            }
        }

        None
    }

    pub fn is_none(&self) -> bool {
        matches!(self, TypeOrigin::NoOrigin)
    }

    pub fn is_default(&self) -> bool {
        match self {
            TypeOrigin::Default(_) => true,
            TypeOrigin::OriginatedAt(_) => false,
            TypeOrigin::NoOrigin => false,
            TypeOrigin::Declared(_) => false,
            TypeOrigin::Multiple(origins) => origins.iter().any(|origin| origin.is_default()),
        }
    }

    pub fn is_declared(&self) -> Option<&SourceSpan> {
        match self {
            TypeOrigin::Declared(span) => Some(span),
            TypeOrigin::OriginatedAt(_) => None,
            TypeOrigin::NoOrigin => None,
            TypeOrigin::Default(_) => None,
            TypeOrigin::Multiple(origins) => {
                for origin in origins {
                    if let TypeOrigin::Declared(span) = origin {
                        return Some(span);
                    }
                }
                None
            }
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
            TypeOrigin::Default(_) => 0.hash(state),
            TypeOrigin::NoOrigin => 1.hash(state),
            TypeOrigin::Multiple(origins) => {
                2.hash(state);
                origins.hash(state);
            }
            TypeOrigin::Declared(span) => {
                3.hash(state);
                span.hash(state);
            }
            TypeOrigin::OriginatedAt(span) => {
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

#[cfg(test)]
mod tests {
    use crate::inferred_type::type_origin::DefaultType;
    use crate::inferred_type::TypeOrigin;
    use crate::rib_source_span::{SourcePosition, SourceSpan};
    use test_r::test;

    #[test]
    fn test_origin_add_1() {
        let existing_origin = TypeOrigin::NoOrigin;
        let result = existing_origin.add_origin(TypeOrigin::Default(DefaultType::S32));

        assert_eq!(result, TypeOrigin::Default(DefaultType::S32));
    }

    #[test]
    fn test_origin_add_2() {
        let existing_origin = TypeOrigin::Default(DefaultType::S32);

        let result = existing_origin.add_origin(TypeOrigin::Default(DefaultType::S32));

        assert!(result.eq(&TypeOrigin::Default(DefaultType::S32)));

        let result = existing_origin.add_origin(TypeOrigin::Default(DefaultType::F64));

        let expected = TypeOrigin::Multiple(vec![
            TypeOrigin::Default(DefaultType::S32),
            TypeOrigin::Default(DefaultType::F64),
        ]);

        assert!(result.eq(&expected));
    }

    #[test]
    fn test_origin_add_3() {
        let existing_origin = TypeOrigin::Default(DefaultType::S32);

        let result = existing_origin.add_origin(TypeOrigin::OriginatedAt(SourceSpan::default()));

        let expected = TypeOrigin::Multiple(vec![
            TypeOrigin::Default(DefaultType::S32),
            TypeOrigin::OriginatedAt(SourceSpan::default()),
        ]);

        assert!(result.eq(&expected));
    }

    #[test]
    fn test_origin_add_4() {
        let type_origin1 = TypeOrigin::Multiple(vec![
            TypeOrigin::Default(DefaultType::S32),
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
            TypeOrigin::Default(DefaultType::S32),
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
