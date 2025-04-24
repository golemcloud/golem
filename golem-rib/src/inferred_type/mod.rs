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

pub(crate) use flatten::*;
mod flatten;
mod unification;
use crate::instance_type::InstanceType;
use crate::rib_source_span::SourceSpan;
use crate::type_inference::GetTypeHint;
use crate::TypeName;
use bigdecimal::num_bigint::Sign;
use bigdecimal::BigDecimal;
use golem_wasm_ast::analysis::analysed_type::*;
use golem_wasm_ast::analysis::*;
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Deref;

#[derive(Debug, Hash, Clone, Eq, PartialOrd, Ord)]
pub struct InferredType {
    pub inner: Box<TypeInternal>,
    pub origin: TypeOrigin,
}

#[derive(Debug, Clone, Ord, PartialOrd)]
pub enum TypeInternal {
    Bool,
    S8,
    U8,
    S16,
    U16,
    S32,
    U32,
    S64,
    U64,
    F32,
    F64,
    Chr,
    Str,
    List(InferredType),
    Tuple(Vec<InferredType>),
    Record(Vec<(String, InferredType)>),
    Flags(Vec<String>),
    Enum(Vec<String>),
    Option(InferredType),
    Result {
        ok: Option<InferredType>,
        error: Option<InferredType>,
    },
    Variant(Vec<(String, Option<InferredType>)>),
    Resource {
        resource_id: u64,
        resource_mode: u8,
    },
    Range {
        from: InferredType,
        to: Option<InferredType>,
    },
    Instance {
        instance_type: Box<InstanceType>,
    },
    OneOf(Vec<InferredType>),
    AllOf(Vec<InferredType>),
    Unknown,
    // Because function result can be a vector of types
    Sequence(Vec<InferredType>),
}

impl TypeInternal {
    pub fn to_inferred_type(&self) -> InferredType {
        InferredType::new(self.clone(), TypeOrigin::NoOrigin)
    }
}

#[derive(Debug, Hash, Clone, Eq, PartialOrd, Ord)]
pub enum TypeOrigin {
    Default(SourceSpan),
    NoOrigin,
    Multiple(Vec<TypeOrigin>),
}

// TypeOrigin doesn't matter in any equality logic
impl PartialEq for TypeOrigin {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl InferredType {
    pub fn s8() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S8),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u8() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U8),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s16() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S16),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u16() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U16),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s32() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S32),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u32() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U32),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s64() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S64),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u64() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U64),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn f32() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::F32),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn f64() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::F64),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn bool() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Bool),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn char() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Chr),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn string() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Str),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn flags(flags: Vec<String>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Flags(flags)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn enum_(cases: Vec<String>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Enum(cases)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn resource(resource_id: u64, resource_mode: u8) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Resource {
                resource_id,
                resource_mode,
            }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn range(from: InferredType, to: Option<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Range { from, to }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn instance(instance_type: InstanceType) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Instance {
                instance_type: Box::new(instance_type),
            }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn list(inner: InferredType) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::List(inner)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn unit() -> InferredType {
        InferredType::tuple(vec![])
    }

    pub fn tuple(inner: Vec<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Tuple(inner)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn record(fields: Vec<(String, InferredType)>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Record(fields)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn option(inner: InferredType) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Option(inner)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn result(ok: Option<InferredType>, error: Option<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Result { ok, error }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn variant(fields: Vec<(String, Option<InferredType>)>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Variant(fields)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn option_type(inner: InferredType) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Option(inner)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn range_type(from: InferredType, to: Option<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Range { from, to }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn new(inferred_type: TypeInternal, origin: TypeOrigin) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
            origin,
        }
    }

    // resolved implies, we no longer care the origin
    pub fn resolved(inferred_type: TypeInternal) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn without_origin(inferred_type: TypeInternal) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn unknown() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Unknown),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn default(inferred_type: TypeInternal, source_span: &SourceSpan) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
            origin: TypeOrigin::Default(source_span.clone()),
        }
    }
}

impl PartialEq for InferredType {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for TypeInternal {}

impl Hash for TypeInternal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            TypeInternal::Bool => 0.hash(state),
            TypeInternal::S8 => 1.hash(state),
            TypeInternal::U8 => 2.hash(state),
            TypeInternal::S16 => 3.hash(state),
            TypeInternal::U16 => 4.hash(state),
            TypeInternal::S32 => 5.hash(state),
            TypeInternal::U32 => 6.hash(state),
            TypeInternal::S64 => 7.hash(state),
            TypeInternal::U64 => 8.hash(state),
            TypeInternal::F32 => 9.hash(state),
            TypeInternal::F64 => 10.hash(state),
            TypeInternal::Chr => 11.hash(state),
            TypeInternal::Str => 12.hash(state),
            TypeInternal::List(inner) => {
                13.hash(state);
                inner.hash(state);
            }
            TypeInternal::Tuple(inner) => {
                14.hash(state);
                inner.hash(state);
            }
            TypeInternal::Record(fields) => {
                15.hash(state);
                let mut sorted_fields = fields.clone();
                sorted_fields.sort_by(|a, b| a.0.cmp(&b.0));
                sorted_fields.hash(state);
            }
            TypeInternal::Flags(flags) => {
                16.hash(state);
                let mut sorted_flags = flags.clone();
                sorted_flags.sort();
                sorted_flags.hash(state);
            }
            TypeInternal::Enum(variants) => {
                17.hash(state);
                let mut sorted_variants = variants.clone();
                sorted_variants.sort();
                sorted_variants.hash(state);
            }
            TypeInternal::Option(inner) => {
                18.hash(state);
                inner.hash(state);
            }
            TypeInternal::Result { ok, error } => {
                19.hash(state);
                ok.hash(state);
                error.hash(state);
            }
            TypeInternal::Variant(fields) => {
                20.hash(state);
                let mut sorted_fields = fields.clone();
                sorted_fields.sort_by(|a, b| a.0.cmp(&b.0));
                sorted_fields.hash(state);
            }
            TypeInternal::Resource {
                resource_id,
                resource_mode,
            } => {
                21.hash(state);
                resource_id.hash(state);
                resource_mode.hash(state);
            }
            TypeInternal::Range { from, to } => {
                22.hash(state);
                from.hash(state);
                to.hash(state);
            }
            TypeInternal::Instance { instance_type } => {
                23.hash(state);
                instance_type.hash(state);
            }
            TypeInternal::OneOf(types)
            | TypeInternal::AllOf(types)
            | TypeInternal::Sequence(types) => {
                24.hash(state);
                let mut sorted_types = types.clone();
                sorted_types.sort();
                sorted_types.hash(state);
            }
            TypeInternal::Unknown => 25.hash(state),
        }
    }
}

impl PartialEq for TypeInternal {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TypeInternal::Bool, TypeInternal::Bool) => true,
            (TypeInternal::S8, TypeInternal::S8) => true,
            (TypeInternal::U8, TypeInternal::U8) => true,
            (TypeInternal::S16, TypeInternal::S16) => true,
            (TypeInternal::U16, TypeInternal::U16) => true,
            (TypeInternal::S32, TypeInternal::S32) => true,
            (TypeInternal::U32, TypeInternal::U32) => true,
            (TypeInternal::S64, TypeInternal::S64) => true,
            (TypeInternal::U64, TypeInternal::U64) => true,
            (TypeInternal::F32, TypeInternal::F32) => true,
            (TypeInternal::F64, TypeInternal::F64) => true,
            (TypeInternal::Chr, TypeInternal::Chr) => true,
            (TypeInternal::Str, TypeInternal::Str) => true,
            (TypeInternal::List(t1), TypeInternal::List(t2)) => t1 == t2,
            (TypeInternal::Tuple(ts1), TypeInternal::Tuple(ts2)) => ts1 == ts2,
            (TypeInternal::Record(fs1), TypeInternal::Record(fs2)) => fs1 == fs2,
            (TypeInternal::Flags(vs1), TypeInternal::Flags(vs2)) => vs1 == vs2,
            (TypeInternal::Enum(vs1), TypeInternal::Enum(vs2)) => vs1 == vs2,
            (TypeInternal::Option(t1), TypeInternal::Option(t2)) => t1 == t2,
            (
                TypeInternal::Result {
                    ok: ok1,
                    error: error1,
                },
                TypeInternal::Result {
                    ok: ok2,
                    error: error2,
                },
            ) => ok1 == ok2 && error1 == error2,
            (TypeInternal::Variant(vs1), TypeInternal::Variant(vs2)) => vs1 == vs2,
            (
                TypeInternal::Resource {
                    resource_id: id1,
                    resource_mode: mode1,
                },
                TypeInternal::Resource {
                    resource_id: id2,
                    resource_mode: mode2,
                },
            ) => id1 == id2 && mode1 == mode2,
            (
                TypeInternal::Range {
                    from: from1,
                    to: to1,
                },
                TypeInternal::Range {
                    from: from2,
                    to: to2,
                },
            ) => from1 == from2 && to1 == to2,
            (
                TypeInternal::Instance { instance_type: t1 },
                TypeInternal::Instance { instance_type: t2 },
            ) => t1 == t2,
            (TypeInternal::Unknown, TypeInternal::Unknown) => true,

            // **Fix: Sort & Compare for OneOf, AllOf, Sequence**
            (TypeInternal::OneOf(ts1), TypeInternal::OneOf(ts2)) => {
                let mut ts1_sorted = ts1.clone();
                let mut ts2_sorted = ts2.clone();
                ts1_sorted.sort();
                ts2_sorted.sort();
                ts1_sorted == ts2_sorted
            }
            (TypeInternal::AllOf(ts1), TypeInternal::AllOf(ts2)) => {
                let mut ts1_sorted = ts1.clone();
                let mut ts2_sorted = ts2.clone();
                ts1_sorted.sort();
                ts2_sorted.sort();
                ts1_sorted == ts2_sorted
            }
            (TypeInternal::Sequence(ts1), TypeInternal::Sequence(ts2)) => {
                let mut ts1_sorted = ts1.clone();
                let mut ts2_sorted = ts2.clone();
                ts1_sorted.sort();
                ts2_sorted.sort();
                ts1_sorted == ts2_sorted
            }

            _ => false,
        }
    }
}

#[derive(PartialEq, Clone, Debug)]
pub enum InferredNumber {
    S8,
    U8,
    S16,
    U16,
    S32,
    U32,
    S64,
    U64,
    F32,
    F64,
}

impl From<InferredNumber> for TypeInternal {
    fn from(inferred_number: InferredNumber) -> Self {
        match inferred_number {
            InferredNumber::S8 => TypeInternal::S8,
            InferredNumber::U8 => TypeInternal::U8,
            InferredNumber::S16 => TypeInternal::S16,
            InferredNumber::U16 => TypeInternal::U16,
            InferredNumber::S32 => TypeInternal::S32,
            InferredNumber::U32 => TypeInternal::U32,
            InferredNumber::S64 => TypeInternal::S64,
            InferredNumber::U64 => TypeInternal::U64,
            InferredNumber::F32 => TypeInternal::F32,
            InferredNumber::F64 => TypeInternal::F64,
        }
    }
}

impl From<&BigDecimal> for TypeInternal {
    fn from(value: &BigDecimal) -> Self {
        let sign = value.sign();

        if value.fractional_digit_count() <= 0 {
            match sign {
                Sign::NoSign => TypeInternal::U64,
                Sign::Minus => TypeInternal::S64,
                Sign::Plus => TypeInternal::U64,
            }
        } else {
            TypeInternal::F64
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct RangeType {
    from: Box<TypeInternal>,
    to: Option<Box<TypeInternal>>,
}

impl Display for InferredNumber {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let type_name = TypeName::from(self);
        write!(f, "{}", type_name)
    }
}

impl InferredType {
    pub fn printable(&self) -> String {
        // Try a fully blown type name or if it fails,
        // get the `kind` of inferred type
        TypeName::try_from(self.clone())
            .map(|tn| tn.to_string())
            .unwrap_or(self.get_type_hint().to_string())
    }

    pub fn contains_only_number(&self) -> bool {
        match self.inner.deref() {
            TypeInternal::S8
            | TypeInternal::U8
            | TypeInternal::S16
            | TypeInternal::U16
            | TypeInternal::S32
            | TypeInternal::U32
            | TypeInternal::S64
            | TypeInternal::U64
            | TypeInternal::F32
            | TypeInternal::F64 => true,
            TypeInternal::AllOf(types) => types.iter().all(|t| t.contains_only_number()),
            TypeInternal::OneOf(types) => types.iter().all(|t| t.contains_only_number()),
            TypeInternal::Bool => false,
            TypeInternal::Chr => false,
            TypeInternal::Str => false,
            TypeInternal::List(_) => false,
            TypeInternal::Tuple(_) => false,
            TypeInternal::Record(_) => false,
            TypeInternal::Flags(_) => false,
            TypeInternal::Enum(_) => false,
            TypeInternal::Option(_) => false,
            TypeInternal::Result { .. } => false,
            TypeInternal::Variant(_) => false,
            TypeInternal::Resource { .. } => false,
            TypeInternal::Range { .. } => false,
            TypeInternal::Instance { .. } => false,
            TypeInternal::Unknown => false,
            TypeInternal::Sequence(_) => false,
        }
    }

    pub fn as_number(&self) -> Result<InferredNumber, String> {
        fn go(with_origin: &InferredType, found: &mut Vec<InferredNumber>) -> Result<(), String> {
            match with_origin.inner.deref() {
                TypeInternal::S8 => {
                    found.push(InferredNumber::S8);
                    Ok(())
                }
                TypeInternal::U8 => {
                    found.push(InferredNumber::U8);
                    Ok(())
                }
                TypeInternal::S16 => {
                    found.push(InferredNumber::S16);
                    Ok(())
                }
                TypeInternal::U16 => {
                    found.push(InferredNumber::U16);
                    Ok(())
                }
                TypeInternal::S32 => {
                    found.push(InferredNumber::U16);
                    Ok(())
                }
                TypeInternal::U32 => {
                    found.push(InferredNumber::U32);
                    Ok(())
                }
                TypeInternal::S64 => {
                    found.push(InferredNumber::S64);
                    Ok(())
                }
                TypeInternal::U64 => {
                    found.push(InferredNumber::U64);
                    Ok(())
                }
                TypeInternal::F32 => {
                    found.push(InferredNumber::F32);
                    Ok(())
                }
                TypeInternal::F64 => {
                    found.push(InferredNumber::F64);
                    Ok(())
                }
                TypeInternal::AllOf(all_variables) => {
                    let mut previous: Option<InferredNumber> = None;
                    for variable in all_variables {
                        go(variable, found)?;

                        if let Some(current) = found.first() {
                            match &previous {
                                None => {
                                    previous = Some(current.clone());
                                    found.push(current.clone());
                                }
                                Some(previous) => {
                                    if previous != current {
                                        return Err(format!(
                                            "expected the same type of number. But found {}, {}",
                                            current, previous
                                        ));
                                    }

                                    found.push(current.clone());
                                }
                            }
                        } else {
                            return Err("failed to get a number".to_string());
                        }
                    }

                    Ok(())
                }
                TypeInternal::Range { .. } => Err("used as range".to_string()),
                TypeInternal::Bool => Err(format!("used as {}", "bool")),
                TypeInternal::Chr => Err(format!("used as {}", "char")),
                TypeInternal::Str => Err(format!("used as {}", "string")),
                TypeInternal::List(_) => Err(format!("used as {}", "list")),
                TypeInternal::Tuple(_) => Err(format!("used as {}", "tuple")),
                TypeInternal::Record(_) => Err(format!("used as {}", "record")),
                TypeInternal::Flags(_) => Err(format!("used as {}", "flags")),
                TypeInternal::Enum(_) => Err(format!("used as {}", "enum")),
                TypeInternal::Option(_) => Err(format!("used as {}", "option")),
                TypeInternal::Result { .. } => Err(format!("used as {}", "result")),
                TypeInternal::Variant(_) => Err(format!("used as {}", "variant")),

                // It's ok to have one-of as far as there is a precise number already `found`
                TypeInternal::OneOf(_) => {
                    if found.is_empty() {
                        Err("not a number.".to_string())
                    } else {
                        Ok(())
                    }
                }
                TypeInternal::Unknown => Err("found unknown".to_string()),

                TypeInternal::Sequence(_) => {
                    Err(format!("used as {}", "function-multi-parameter-return"))
                }
                TypeInternal::Resource { .. } => Err(format!("used as {}", "resource")),
                TypeInternal::Instance { .. } => Err(format!("used as {}", "instance")),
            }
        }

        let mut found: Vec<InferredNumber> = vec![];
        go(self, &mut found)?;
        found.first().cloned().ok_or("Failed".to_string())
    }

    pub fn number(source_span: &SourceSpan) -> InferredType {
        let inferred_type = TypeInternal::OneOf(vec![
            InferredType::default(TypeInternal::U64, source_span),
            InferredType::default(TypeInternal::U32, source_span),
            InferredType::default(TypeInternal::U8, source_span),
            InferredType::default(TypeInternal::U16, source_span),
            InferredType::default(TypeInternal::S64, source_span),
            InferredType::default(TypeInternal::S32, source_span),
            InferredType::default(TypeInternal::S8, source_span),
            InferredType::default(TypeInternal::S16, source_span),
            InferredType::default(TypeInternal::F64, source_span),
            InferredType::default(TypeInternal::F32, source_span),
        ]);

        InferredType {
            inner: Box::new(inferred_type),
            origin: TypeOrigin::Default(source_span.clone()),
        }
    }

    pub fn un_resolved(&self) -> bool {
        self.is_unknown() || self.is_one_of()
    }

    pub fn all_of(types: Vec<InferredType>) -> Option<InferredType> {
        let flattened = InferredType::flatten_all_of_inferred_types(&types);

        let mut types: Vec<InferredType> =
            flattened.into_iter().filter(|t| !t.is_unknown()).collect();

        let mut unique_types: HashSet<InferredType> = HashSet::new();
        types.retain(|t| unique_types.insert(t.clone()));

        if unique_types.is_empty() {
            None
        } else if unique_types.len() == 1 {
            unique_types.into_iter().next()
        } else {
            let mut unique_all_of_types: Vec<InferredType> = unique_types.into_iter().collect();
            unique_all_of_types.sort();
            let origin = TypeOrigin::Multiple(
                unique_all_of_types
                    .iter()
                    .map(|x| x.origin.clone())
                    .collect(),
            );

            Some(InferredType {
                inner: Box::new(TypeInternal::AllOf(unique_all_of_types)),
                origin,
            })
        }
    }

    pub fn one_of(types: Vec<InferredType>) -> Option<InferredType> {
        let flattened = InferredType::flatten_one_of_inferred_types(&types);

        let mut types: Vec<InferredType> =
            flattened.into_iter().filter(|t| !t.is_unknown()).collect();

        // Make sure they are unique types
        let mut unique_types: HashSet<InferredType> = HashSet::new();
        types.retain(|t| unique_types.insert(t.clone()));

        if types.is_empty() {
            None
        } else if types.len() == 1 {
            types.into_iter().next()
        } else {
            let mut unique_one_of_types: Vec<InferredType> = unique_types.into_iter().collect();
            unique_one_of_types.sort();

            let origin = TypeOrigin::Multiple(
                unique_one_of_types
                    .iter()
                    .map(|x| x.origin.clone())
                    .collect(),
            );

            Some(InferredType {
                inner: Box::new(TypeInternal::OneOf(unique_one_of_types)),
                origin,
            })
        }
    }

    pub fn is_unit(&self) -> bool {
        match self.inner.deref() {
            TypeInternal::Sequence(types) => types.is_empty(),
            _ => false,
        }
    }
    pub fn is_unknown(&self) -> bool {
        matches!(self.inner.deref(), TypeInternal::Unknown)
    }

    pub fn is_one_of(&self) -> bool {
        matches!(self.inner.deref(), TypeInternal::OneOf(_))
    }

    pub fn is_valid_wit_type(&self) -> bool {
        AnalysedType::try_from(self).is_ok()
    }

    pub fn is_all_of(&self) -> bool {
        matches!(self.inner.deref(), TypeInternal::AllOf(_))
    }

    pub fn is_number(&self) -> bool {
        matches!(
            self.inner.deref(),
            TypeInternal::S8
                | TypeInternal::U8
                | TypeInternal::S16
                | TypeInternal::U16
                | TypeInternal::S32
                | TypeInternal::U32
                | TypeInternal::S64
                | TypeInternal::U64
                | TypeInternal::F32
                | TypeInternal::F64
        )
    }

    pub fn is_string(&self) -> bool {
        matches!(self.inner.deref(), TypeInternal::Str)
    }

    pub fn flatten_all_of_inferred_types(types: &Vec<InferredType>) -> Vec<InferredType> {
        flatten_all_of_list(types)
    }

    pub fn flatten_one_of_inferred_types(types: &Vec<InferredType>) -> Vec<InferredType> {
        flatten_one_of_list(types)
    }

    // Here unification returns an inferred type, but it doesn't necessarily imply
    // its valid type, which can be converted to a wasm type.
    pub fn try_unify(&self) -> Result<InferredType, String> {
        unification::try_unify_type(self)
    }

    pub fn unify(&self) -> Result<InferredType, String> {
        unification::unify(self).map(|unified| unified.inferred_type())
    }

    pub fn unify_all_alternative_types(types: &Vec<InferredType>) -> InferredType {
        unification::unify_all_alternative_types(types)
    }

    pub fn unify_all_required_types(types: &Vec<InferredType>) -> Result<InferredType, String> {
        unification::unify_all_required_types(types)
    }

    // Unify types where both types do matter. Example in reality x can form to be both U64 and U32 in the IR, resulting in AllOf
    // Result of this type hardly becomes OneOf
    pub fn unify_with_required(&self, other: &InferredType) -> Result<InferredType, String> {
        unification::unify_with_required(self, other)
    }

    pub fn unify_with_alternative(&self, other: &InferredType) -> Result<InferredType, String> {
        unification::unify_with_alternative(self, other)
    }

    // There is only one way to merge types. If they are different, they are merged into AllOf
    pub fn merge(&self, new_inferred_type: InferredType) -> InferredType {
        if !internal::need_update(self, &new_inferred_type) {
            return self.clone();
        }

        match (self.inner.deref(), new_inferred_type.inner.deref()) {
            (TypeInternal::Unknown, _) => new_inferred_type,

            (TypeInternal::AllOf(existing_types), TypeInternal::AllOf(new_types)) => {
                let mut all_types = new_types.clone();
                all_types.extend(existing_types.clone());

                InferredType::all_of(all_types).unwrap_or(InferredType::unknown())
            }

            (TypeInternal::AllOf(existing_types), new_type) => {
                let mut all_types = existing_types.clone();
                all_types.push(new_inferred_type);

                InferredType::all_of(all_types).unwrap_or(InferredType::unknown())
            }

            (_, TypeInternal::AllOf(new_types)) => {
                let mut all_types = new_types.clone();
                all_types.push(self.clone());

                InferredType::all_of(all_types).unwrap_or(InferredType::unknown())
            }

            (TypeInternal::OneOf(existing_types), TypeInternal::OneOf(new_types)) => {
                let mut one_of_types = new_types.clone();
                if new_types == existing_types {
                    return new_inferred_type;
                } else {
                    one_of_types.extend(existing_types.clone());
                }

                InferredType::one_of(one_of_types).unwrap_or(InferredType::unknown())
            }

            (TypeInternal::OneOf(existing_types), new_type_internal) => {
                if existing_types.contains(&new_inferred_type) {
                    new_inferred_type
                } else {
                    InferredType::all_of(vec![self.clone(), new_inferred_type])
                        .unwrap_or(InferredType::unknown())
                }
            }

            (current_type_internal, TypeInternal::OneOf(newtypes)) => {
                if newtypes.contains(self) {
                    self.clone()
                } else {
                    InferredType::all_of(vec![self.clone(), new_inferred_type])
                        .unwrap_or(InferredType::unknown())
                }
            }

            (_, new_type_internal) => {
                InferredType::all_of(vec![self.clone(), new_inferred_type.clone()])
                    .unwrap_or(InferredType::unknown())
            }
        }
    }

    pub fn from_type_variant(type_variant: &TypeVariant) -> InferredType {
        let cases = type_variant
            .cases
            .iter()
            .map(|name_type_pair| {
                (
                    name_type_pair.name.clone(),
                    name_type_pair.typ.as_ref().map(|t| t.into()),
                )
            })
            .collect();

        InferredType::from_variant_cases(cases)
    }

    pub fn from_variant_cases(cases: Vec<(String, Option<InferredType>)>) -> InferredType {
        InferredType::without_origin(TypeInternal::Variant(cases))
    }

    pub fn from_enum_cases(type_enum: &TypeEnum) -> InferredType {
        InferredType::without_origin(TypeInternal::Enum(type_enum.cases.clone()))
    }
}

impl TryFrom<&InferredType> for AnalysedType {
    type Error = String;

    fn try_from(value: &InferredType) -> Result<Self, Self::Error> {
        match value.inner.deref() {
            TypeInternal::Bool => Ok(bool()),
            TypeInternal::S8 => Ok(s8()),
            TypeInternal::U8 => Ok(u8()),
            TypeInternal::S16 => Ok(s16()),
            TypeInternal::U16 => Ok(u16()),
            TypeInternal::S32 => Ok(s32()),
            TypeInternal::U32 => Ok(u32()),
            TypeInternal::S64 => Ok(s64()),
            TypeInternal::U64 => Ok(u64()),
            TypeInternal::F32 => Ok(f32()),
            TypeInternal::F64 => Ok(f64()),
            TypeInternal::Chr => Ok(chr()),
            TypeInternal::Str => Ok(str()),
            TypeInternal::List(typ) => {
                let typ: AnalysedType = typ.try_into()?;
                Ok(list(typ))
            }
            TypeInternal::Tuple(types) => {
                let types: Vec<AnalysedType> = types
                    .into_iter()
                    .map(|t| t.try_into())
                    .collect::<Result<Vec<AnalysedType>, _>>()?;
                Ok(tuple(types))
            }
            TypeInternal::Record(field_and_types) => {
                let mut field_pairs: Vec<NameTypePair> = vec![];
                for (name, typ) in field_and_types {
                    let typ: AnalysedType = typ.try_into()?;
                    let name = name.clone();
                    field_pairs.push(NameTypePair { name, typ });
                }
                Ok(record(field_pairs))
            }
            TypeInternal::Flags(names) => Ok(AnalysedType::Flags(TypeFlags {
                names: names.clone(),
            })),
            TypeInternal::Enum(cases) => Ok(AnalysedType::Enum(TypeEnum {
                cases: cases.clone(),
            })),
            TypeInternal::Option(typ) => {
                let typ: AnalysedType = typ.try_into()?;
                Ok(option(typ))
            }
            TypeInternal::Result { ok, error } => {
                let ok_option: Option<AnalysedType> =
                    ok.as_ref().map(|t| t.try_into()).transpose()?;
                let ok = ok_option.ok_or("Expected ok type in result".to_string())?;
                let error_option: Option<AnalysedType> =
                    error.as_ref().map(|t| t.try_into()).transpose()?;
                let error = error_option.ok_or("Expected error type in result".to_string())?;
                Ok(result(ok, error))
            }
            TypeInternal::Variant(name_and_optiona_inferred_types) => {
                let mut cases: Vec<NameOptionTypePair> = vec![];
                for (name, typ) in name_and_optiona_inferred_types {
                    let typ: Option<AnalysedType> =
                        typ.as_ref().map(|t| t.try_into()).transpose()?;
                    cases.push(NameOptionTypePair {
                        name: name.clone(),
                        typ,
                    });
                }
                Ok(variant(cases))
            }
            TypeInternal::Resource {
                resource_id,
                resource_mode,
            } => Ok(handle(
                AnalysedResourceId(*resource_id),
                match resource_mode {
                    0 => AnalysedResourceMode::Owned,
                    1 => AnalysedResourceMode::Borrowed,
                    _ => return Err("Invalid resource mode".to_string()),
                },
            )),
            TypeInternal::Instance { .. } => {
                Err("Cannot convert instance type to analysed type".to_string())
            }
            TypeInternal::OneOf(_) => {
                Err("Cannot convert one of type to analysed type".to_string())
            }
            TypeInternal::AllOf(_) => {
                Err("Cannot convert all of type to analysed type".to_string())
            }
            TypeInternal::Unknown => {
                Err("Cannot convert unknown type to analysed type".to_string())
            }
            TypeInternal::Sequence(_) => {
                Err("Cannot convert function return sequence type to analysed type".to_string())
            }
            TypeInternal::Range { from, to } => {
                let from: AnalysedType = from.try_into()?;
                let to: Option<AnalysedType> = to.as_ref().map(|t| t.try_into()).transpose()?;
                let analysed_type = match (from, to) {
                    (from_type, Some(to_type)) => record(vec![
                        field("from", option(from_type)),
                        field("to", option(to_type)),
                        field("inclusive", bool()),
                    ]),

                    (from_type, None) => record(vec![
                        field("from", option(from_type)),
                        field("inclusive", bool()),
                    ]),
                };
                Ok(analysed_type)
            }
        }
    }
}

impl From<&AnalysedType> for InferredType {
    fn from(analysed_type: &AnalysedType) -> Self {
        match analysed_type {
            AnalysedType::Bool(_) => InferredType::bool(),
            AnalysedType::S8(_) => InferredType::s8(),
            AnalysedType::U8(_) => InferredType::u8(),
            AnalysedType::S16(_) => InferredType::s16(),
            AnalysedType::U16(_) => InferredType::u16(),
            AnalysedType::S32(_) => InferredType::s32(),
            AnalysedType::U32(_) => InferredType::u32(),
            AnalysedType::S64(_) => InferredType::s64(),
            AnalysedType::U64(_) => InferredType::u64(),
            AnalysedType::F32(_) => InferredType::f32(),
            AnalysedType::F64(_) => InferredType::f64(),
            AnalysedType::Chr(_) => InferredType::char(),
            AnalysedType::Str(_) => InferredType::string(),
            AnalysedType::List(t) => InferredType::list(t.inner.as_ref().into()),
            AnalysedType::Tuple(ts) => {
                InferredType::tuple(ts.items.iter().map(|t| t.into()).collect())
            }
            AnalysedType::Record(fs) => InferredType::record(
                fs.fields
                    .iter()
                    .map(|name_type| (name_type.name.clone(), (&name_type.typ).into()))
                    .collect(),
            ),
            AnalysedType::Flags(vs) => InferredType::flags(vs.names.clone()),
            AnalysedType::Enum(vs) => InferredType::from_enum_cases(vs),
            AnalysedType::Option(t) => InferredType::option(t.inner.as_ref().into()),
            AnalysedType::Result(golem_wasm_ast::analysis::TypeResult { ok, err, .. }) => {
                InferredType::result(
                    ok.as_ref().map(|t| t.as_ref().into()),
                    err.as_ref().map(|t| t.as_ref().into()),
                )
            }
            AnalysedType::Variant(vs) => InferredType::from_type_variant(vs),
            AnalysedType::Handle(golem_wasm_ast::analysis::TypeHandle { resource_id, mode }) => {
                InferredType::resource(
                    resource_id.0,
                    match mode {
                        AnalysedResourceMode::Owned => 0,
                        AnalysedResourceMode::Borrowed => 1,
                    },
                )
            }
        }
    }
}

mod internal {
    use crate::{InferredType, TypeInternal};

    pub(crate) fn need_update(
        current_inferred_type: &InferredType,
        new_inferred_type: &InferredType,
    ) -> bool {
        current_inferred_type != new_inferred_type && !new_inferred_type.is_unknown()
    }
}

#[cfg(test)]
mod test {
    use crate::InvalidItem::Type;
    use crate::{InferredType, TypeOrigin};

    #[test]
    fn test_flatten_one_of() {
        use super::TypeInternal;
        let one_of = vec![
            InferredType::u8(),
            InferredType::u16(),
            InferredType::u32(),
            InferredType::one_of(vec![
                InferredType::u8(),
                InferredType::u16(),
                InferredType::u32(),
                InferredType::all_of(vec![
                    InferredType::u64(),
                    InferredType::one_of(vec![InferredType::u64(), InferredType::u8()]).unwrap(),
                ])
                .unwrap(),
            ])
            .unwrap(),
        ];

        let flattened = InferredType::flatten_one_of_inferred_types(&one_of);

        let expected = vec![
            InferredType::u8(),
            InferredType::u16(),
            InferredType::u32(),
            InferredType::u8(),
            InferredType::u16(),
            InferredType::u32(),
            InferredType::new(
                TypeInternal::AllOf(vec![
                    InferredType::u64(),
                    InferredType::new(
                        TypeInternal::OneOf(vec![InferredType::u64(), InferredType::u8()]),
                        TypeOrigin::Multiple(vec![]),
                    ),
                ]),
                TypeOrigin::Multiple(vec![]),
            ),
        ];

        assert_eq!(flattened, expected)
    }
}
