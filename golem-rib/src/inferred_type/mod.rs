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
use crate::type_inference::GetTypeHint;
use crate::TypeName;
use bigdecimal::num_bigint::Sign;
use bigdecimal::BigDecimal;
use golem_wasm_ast::analysis::analysed_type::*;
use golem_wasm_ast::analysis::*;
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Ord, PartialOrd)]
pub enum InferredType {
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
    List(Box<InferredType>),
    Tuple(Vec<InferredType>),
    Record(Vec<(String, InferredType)>),
    Flags(Vec<String>),
    Enum(Vec<String>),
    Option(Box<InferredType>),
    Result {
        ok: Option<Box<InferredType>>,
        error: Option<Box<InferredType>>,
    },
    Variant(Vec<(String, Option<InferredType>)>),
    Resource {
        resource_id: u64,
        resource_mode: u8,
    },
    Range {
        from: Box<InferredType>,
        to: Option<Box<InferredType>>,
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

impl Eq for InferredType {}

impl Hash for InferredType {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            InferredType::Bool => 0.hash(state),
            InferredType::S8 => 1.hash(state),
            InferredType::U8 => 2.hash(state),
            InferredType::S16 => 3.hash(state),
            InferredType::U16 => 4.hash(state),
            InferredType::S32 => 5.hash(state),
            InferredType::U32 => 6.hash(state),
            InferredType::S64 => 7.hash(state),
            InferredType::U64 => 8.hash(state),
            InferredType::F32 => 9.hash(state),
            InferredType::F64 => 10.hash(state),
            InferredType::Chr => 11.hash(state),
            InferredType::Str => 12.hash(state),
            InferredType::List(inner) => {
                13.hash(state);
                inner.hash(state);
            }
            InferredType::Tuple(inner) => {
                14.hash(state);
                inner.hash(state);
            }
            InferredType::Record(fields) => {
                15.hash(state);
                let mut sorted_fields = fields.clone();
                sorted_fields.sort_by(|a, b| a.0.cmp(&b.0));
                sorted_fields.hash(state);
            }
            InferredType::Flags(flags) => {
                16.hash(state);
                let mut sorted_flags = flags.clone();
                sorted_flags.sort();
                sorted_flags.hash(state);
            }
            InferredType::Enum(variants) => {
                17.hash(state);
                let mut sorted_variants = variants.clone();
                sorted_variants.sort();
                sorted_variants.hash(state);
            }
            InferredType::Option(inner) => {
                18.hash(state);
                inner.hash(state);
            }
            InferredType::Result { ok, error } => {
                19.hash(state);
                ok.hash(state);
                error.hash(state);
            }
            InferredType::Variant(fields) => {
                20.hash(state);
                let mut sorted_fields = fields.clone();
                sorted_fields.sort_by(|a, b| a.0.cmp(&b.0));
                sorted_fields.hash(state);
            }
            InferredType::Resource {
                resource_id,
                resource_mode,
            } => {
                21.hash(state);
                resource_id.hash(state);
                resource_mode.hash(state);
            }
            InferredType::Range { from, to } => {
                22.hash(state);
                from.hash(state);
                to.hash(state);
            }
            InferredType::Instance { instance_type } => {
                23.hash(state);
                instance_type.hash(state);
            }
            InferredType::OneOf(types)
            | InferredType::AllOf(types)
            | InferredType::Sequence(types) => {
                24.hash(state);
                let mut sorted_types = types.clone();
                sorted_types.sort();
                sorted_types.hash(state);
            }
            InferredType::Unknown => 25.hash(state),
        }
    }
}

impl PartialEq for InferredType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (InferredType::Bool, InferredType::Bool) => true,
            (InferredType::S8, InferredType::S8) => true,
            (InferredType::U8, InferredType::U8) => true,
            (InferredType::S16, InferredType::S16) => true,
            (InferredType::U16, InferredType::U16) => true,
            (InferredType::S32, InferredType::S32) => true,
            (InferredType::U32, InferredType::U32) => true,
            (InferredType::S64, InferredType::S64) => true,
            (InferredType::U64, InferredType::U64) => true,
            (InferredType::F32, InferredType::F32) => true,
            (InferredType::F64, InferredType::F64) => true,
            (InferredType::Chr, InferredType::Chr) => true,
            (InferredType::Str, InferredType::Str) => true,
            (InferredType::List(t1), InferredType::List(t2)) => t1 == t2,
            (InferredType::Tuple(ts1), InferredType::Tuple(ts2)) => ts1 == ts2,
            (InferredType::Record(fs1), InferredType::Record(fs2)) => fs1 == fs2,
            (InferredType::Flags(vs1), InferredType::Flags(vs2)) => vs1 == vs2,
            (InferredType::Enum(vs1), InferredType::Enum(vs2)) => vs1 == vs2,
            (InferredType::Option(t1), InferredType::Option(t2)) => t1 == t2,
            (
                InferredType::Result {
                    ok: ok1,
                    error: error1,
                },
                InferredType::Result {
                    ok: ok2,
                    error: error2,
                },
            ) => ok1 == ok2 && error1 == error2,
            (InferredType::Variant(vs1), InferredType::Variant(vs2)) => vs1 == vs2,
            (
                InferredType::Resource {
                    resource_id: id1,
                    resource_mode: mode1,
                },
                InferredType::Resource {
                    resource_id: id2,
                    resource_mode: mode2,
                },
            ) => id1 == id2 && mode1 == mode2,
            (
                InferredType::Range {
                    from: from1,
                    to: to1,
                },
                InferredType::Range {
                    from: from2,
                    to: to2,
                },
            ) => from1 == from2 && to1 == to2,
            (
                InferredType::Instance { instance_type: t1 },
                InferredType::Instance { instance_type: t2 },
            ) => t1 == t2,
            (InferredType::Unknown, InferredType::Unknown) => true,

            // **Fix: Sort & Compare for OneOf, AllOf, Sequence**
            (InferredType::OneOf(ts1), InferredType::OneOf(ts2)) => {
                let mut ts1_sorted = ts1.clone();
                let mut ts2_sorted = ts2.clone();
                ts1_sorted.sort();
                ts2_sorted.sort();
                ts1_sorted == ts2_sorted
            }
            (InferredType::AllOf(ts1), InferredType::AllOf(ts2)) => {
                let mut ts1_sorted = ts1.clone();
                let mut ts2_sorted = ts2.clone();
                ts1_sorted.sort();
                ts2_sorted.sort();
                ts1_sorted == ts2_sorted
            }
            (InferredType::Sequence(ts1), InferredType::Sequence(ts2)) => {
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

impl From<InferredNumber> for InferredType {
    fn from(inferred_number: InferredNumber) -> Self {
        match inferred_number {
            InferredNumber::S8 => InferredType::S8,
            InferredNumber::U8 => InferredType::U8,
            InferredNumber::S16 => InferredType::S16,
            InferredNumber::U16 => InferredType::U16,
            InferredNumber::S32 => InferredType::S32,
            InferredNumber::U32 => InferredType::U32,
            InferredNumber::S64 => InferredType::S64,
            InferredNumber::U64 => InferredType::U64,
            InferredNumber::F32 => InferredType::F32,
            InferredNumber::F64 => InferredType::F64,
        }
    }
}

impl From<&BigDecimal> for InferredType {
    fn from(value: &BigDecimal) -> Self {
        let sign = value.sign();

        if value.fractional_digit_count() <= 0 {
            match sign {
                Sign::NoSign => InferredType::U64,
                Sign::Minus => InferredType::S64,
                Sign::Plus => InferredType::U64,
            }
        } else {
            InferredType::F64
        }
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct RangeType {
    from: Box<InferredType>,
    to: Option<Box<InferredType>>,
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
        match self {
            InferredType::S8
            | InferredType::U8
            | InferredType::S16
            | InferredType::U16
            | InferredType::S32
            | InferredType::U32
            | InferredType::S64
            | InferredType::U64
            | InferredType::F32
            | InferredType::F64 => true,
            InferredType::AllOf(types) => types.iter().all(|t| t.contains_only_number()),
            InferredType::OneOf(types) => types.iter().all(|t| t.contains_only_number()),
            InferredType::Bool => false,
            InferredType::Chr => false,
            InferredType::Str => false,
            InferredType::List(_) => false,
            InferredType::Tuple(_) => false,
            InferredType::Record(_) => false,
            InferredType::Flags(_) => false,
            InferredType::Enum(_) => false,
            InferredType::Option(_) => false,
            InferredType::Result { .. } => false,
            InferredType::Variant(_) => false,
            InferredType::Resource { .. } => false,
            InferredType::Range { .. } => false,
            InferredType::Instance { .. } => false,
            InferredType::Unknown => false,
            InferredType::Sequence(_) => false,
        }
    }

    pub fn as_number(&self) -> Result<InferredNumber, String> {
        fn go(inferred_type: &InferredType, found: &mut Vec<InferredNumber>) -> Result<(), String> {
            match inferred_type {
                InferredType::S8 => {
                    found.push(InferredNumber::S8);
                    Ok(())
                }
                InferredType::U8 => {
                    found.push(InferredNumber::U8);
                    Ok(())
                }
                InferredType::S16 => {
                    found.push(InferredNumber::S16);
                    Ok(())
                }
                InferredType::U16 => {
                    found.push(InferredNumber::U16);
                    Ok(())
                }
                InferredType::S32 => {
                    found.push(InferredNumber::U16);
                    Ok(())
                }
                InferredType::U32 => {
                    found.push(InferredNumber::U32);
                    Ok(())
                }
                InferredType::S64 => {
                    found.push(InferredNumber::S64);
                    Ok(())
                }
                InferredType::U64 => {
                    found.push(InferredNumber::U64);
                    Ok(())
                }
                InferredType::F32 => {
                    found.push(InferredNumber::F32);
                    Ok(())
                }
                InferredType::F64 => {
                    found.push(InferredNumber::F64);
                    Ok(())
                }
                InferredType::AllOf(all_variables) => {
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
                InferredType::Range { .. } => Err("used as range".to_string()),
                InferredType::Bool => Err(format!("used as {}", "bool")),
                InferredType::Chr => Err(format!("used as {}", "char")),
                InferredType::Str => Err(format!("used as {}", "string")),
                InferredType::List(_) => Err(format!("used as {}", "list")),
                InferredType::Tuple(_) => Err(format!("used as {}", "tuple")),
                InferredType::Record(_) => Err(format!("used as {}", "record")),
                InferredType::Flags(_) => Err(format!("used as {}", "flags")),
                InferredType::Enum(_) => Err(format!("used as {}", "enum")),
                InferredType::Option(_) => Err(format!("used as {}", "option")),
                InferredType::Result { .. } => Err(format!("used as {}", "result")),
                InferredType::Variant(_) => Err(format!("used as {}", "variant")),

                // It's ok to have one-of as far as there is a precise number already `found`
                InferredType::OneOf(_) => {
                    if found.is_empty() {
                        Err("not a number.".to_string())
                    } else {
                        Ok(())
                    }
                }
                InferredType::Unknown => Err("found unknown".to_string()),

                InferredType::Sequence(_) => {
                    Err(format!("used as {}", "function-multi-parameter-return"))
                }
                InferredType::Resource { .. } => Err(format!("used as {}", "resource")),
                InferredType::Instance { .. } => Err(format!("used as {}", "instance")),
            }
        }

        let mut found: Vec<InferredNumber> = vec![];
        go(self, &mut found)?;
        found.first().cloned().ok_or("Failed".to_string())
    }

    pub fn number() -> InferredType {
        InferredType::OneOf(vec![
            InferredType::U64,
            InferredType::U32,
            InferredType::U8,
            InferredType::U16,
            InferredType::S64,
            InferredType::S32,
            InferredType::S8,
            InferredType::S16,
            InferredType::F64,
            InferredType::F32,
        ])
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
            Some(InferredType::AllOf(unique_all_of_types))
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
            Some(InferredType::OneOf(unique_one_of_types))
        }
    }

    pub fn is_unit(&self) -> bool {
        match self {
            InferredType::Sequence(types) => types.is_empty(),
            _ => false,
        }
    }
    pub fn is_unknown(&self) -> bool {
        matches!(self, InferredType::Unknown)
    }

    pub fn is_one_of(&self) -> bool {
        matches!(self, InferredType::OneOf(_))
    }

    pub fn is_valid_wit_type(&self) -> bool {
        AnalysedType::try_from(self.clone()).is_ok()
    }

    pub fn is_all_of(&self) -> bool {
        matches!(self, InferredType::AllOf(_))
    }

    pub fn is_number(&self) -> bool {
        matches!(
            self,
            InferredType::S8
                | InferredType::U8
                | InferredType::S16
                | InferredType::U16
                | InferredType::S32
                | InferredType::U32
                | InferredType::S64
                | InferredType::U64
                | InferredType::F32
                | InferredType::F64
        )
    }

    pub fn is_string(&self) -> bool {
        matches!(self, InferredType::Str)
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

        match (self, new_inferred_type) {
            (InferredType::Unknown, new_type) => new_type,

            (InferredType::AllOf(existing_types), InferredType::AllOf(new_types)) => {
                let mut all_types = new_types.clone();
                all_types.extend(existing_types.clone());

                InferredType::all_of(all_types).unwrap_or(InferredType::Unknown)
            }

            (InferredType::AllOf(existing_types), new_type) => {
                let mut all_types = existing_types.clone();
                all_types.push(new_type);

                InferredType::all_of(all_types).unwrap_or(InferredType::Unknown)
            }

            (current_type, InferredType::AllOf(new_types)) => {
                let mut all_types = new_types.clone();
                all_types.push(current_type.clone());

                InferredType::all_of(all_types).unwrap_or(InferredType::Unknown)
            }

            (InferredType::OneOf(existing_types), InferredType::OneOf(new_types)) => {
                let mut one_of_types = new_types.clone();
                if &new_types == existing_types {
                    return InferredType::OneOf(one_of_types);
                } else {
                    one_of_types.extend(existing_types.clone());
                }

                InferredType::one_of(one_of_types).unwrap_or(InferredType::Unknown)
            }

            (InferredType::OneOf(existing_types), new_type) => {
                if existing_types.contains(&new_type) {
                    new_type
                } else {
                    InferredType::all_of(vec![self.clone(), new_type])
                        .unwrap_or(InferredType::Unknown)
                }
            }

            (current_type, InferredType::OneOf(newtypes)) => {
                if newtypes.contains(current_type) {
                    current_type.clone()
                } else {
                    InferredType::all_of(vec![current_type.clone(), InferredType::OneOf(newtypes)])
                        .unwrap_or(InferredType::Unknown)
                }
            }

            (current_type, new_type) => {
                InferredType::all_of(vec![current_type.clone(), new_type.clone()])
                    .unwrap_or(InferredType::Unknown)
            }
        }
    }

    pub fn from_variant_cases(type_variant: &TypeVariant) -> InferredType {
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

        InferredType::Variant(cases)
    }

    pub fn from_enum_cases(type_enum: &TypeEnum) -> InferredType {
        InferredType::Enum(type_enum.cases.clone())
    }
}

impl TryFrom<InferredType> for AnalysedType {
    type Error = String;

    fn try_from(value: InferredType) -> Result<Self, Self::Error> {
        match value {
            InferredType::Bool => Ok(bool()),
            InferredType::S8 => Ok(s8()),
            InferredType::U8 => Ok(u8()),
            InferredType::S16 => Ok(s16()),
            InferredType::U16 => Ok(u16()),
            InferredType::S32 => Ok(s32()),
            InferredType::U32 => Ok(u32()),
            InferredType::S64 => Ok(s64()),
            InferredType::U64 => Ok(u64()),
            InferredType::F32 => Ok(f32()),
            InferredType::F64 => Ok(f64()),
            InferredType::Chr => Ok(chr()),
            InferredType::Str => Ok(str()),
            InferredType::List(typ) => {
                let typ: AnalysedType = (*typ).try_into()?;
                Ok(list(typ))
            }
            InferredType::Tuple(types) => {
                let types: Vec<AnalysedType> = types
                    .into_iter()
                    .map(|t| t.try_into())
                    .collect::<Result<Vec<AnalysedType>, _>>()?;
                Ok(tuple(types))
            }
            InferredType::Record(field_and_types) => {
                let mut field_pairs: Vec<NameTypePair> = vec![];
                for (name, typ) in field_and_types {
                    let typ: AnalysedType = typ.try_into()?;
                    field_pairs.push(NameTypePair { name, typ });
                }
                Ok(record(field_pairs))
            }
            InferredType::Flags(names) => Ok(AnalysedType::Flags(TypeFlags { names })),
            InferredType::Enum(cases) => Ok(AnalysedType::Enum(TypeEnum { cases })),
            InferredType::Option(typ) => {
                let typ: AnalysedType = (*typ).try_into()?;
                Ok(option(typ))
            }
            InferredType::Result { ok, error } => {
                let ok_option: Option<AnalysedType> = ok.map(|t| (*t).try_into()).transpose()?;
                let ok = ok_option.ok_or("Expected ok type in result".to_string())?;
                let error_option: Option<AnalysedType> =
                    error.map(|t| (*t).try_into()).transpose()?;
                let error = error_option.ok_or("Expected error type in result".to_string())?;
                Ok(result(ok, error))
            }
            InferredType::Variant(name_and_optiona_inferred_types) => {
                let mut cases: Vec<NameOptionTypePair> = vec![];
                for (name, typ) in name_and_optiona_inferred_types {
                    let typ: Option<AnalysedType> = typ.map(|t| t.try_into()).transpose()?;
                    cases.push(NameOptionTypePair { name, typ });
                }
                Ok(variant(cases))
            }
            InferredType::Resource {
                resource_id,
                resource_mode,
            } => Ok(handle(
                AnalysedResourceId(resource_id),
                match resource_mode {
                    0 => AnalysedResourceMode::Owned,
                    1 => AnalysedResourceMode::Borrowed,
                    _ => return Err("Invalid resource mode".to_string()),
                },
            )),
            InferredType::Instance { .. } => {
                Err("Cannot convert instance type to analysed type".to_string())
            }
            InferredType::OneOf(_) => {
                Err("Cannot convert one of type to analysed type".to_string())
            }
            InferredType::AllOf(_) => {
                Err("Cannot convert all of type to analysed type".to_string())
            }
            InferredType::Unknown => {
                Err("Cannot convert unknown type to analysed type".to_string())
            }
            InferredType::Sequence(_) => {
                Err("Cannot convert function return sequence type to analysed type".to_string())
            }
            InferredType::Range { from, to } => {
                let from: AnalysedType = (*from).try_into()?;
                let to: Option<AnalysedType> = to.map(|t| (*t).try_into()).transpose()?;
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
            AnalysedType::Bool(_) => InferredType::Bool,
            AnalysedType::S8(_) => InferredType::S8,
            AnalysedType::U8(_) => InferredType::U8,
            AnalysedType::S16(_) => InferredType::S16,
            AnalysedType::U16(_) => InferredType::U16,
            AnalysedType::S32(_) => InferredType::S32,
            AnalysedType::U32(_) => InferredType::U32,
            AnalysedType::S64(_) => InferredType::S64,
            AnalysedType::U64(_) => InferredType::U64,
            AnalysedType::F32(_) => InferredType::F32,
            AnalysedType::F64(_) => InferredType::F64,
            AnalysedType::Chr(_) => InferredType::Chr,
            AnalysedType::Str(_) => InferredType::Str,
            AnalysedType::List(t) => InferredType::List(Box::new(t.inner.as_ref().into())),
            AnalysedType::Tuple(ts) => {
                InferredType::Tuple(ts.items.iter().map(|t| t.into()).collect())
            }
            AnalysedType::Record(fs) => InferredType::Record(
                fs.fields
                    .iter()
                    .map(|name_type| (name_type.name.clone(), (&name_type.typ).into()))
                    .collect(),
            ),
            AnalysedType::Flags(vs) => InferredType::Flags(vs.names.clone()),
            AnalysedType::Enum(vs) => InferredType::from_enum_cases(&vs),
            AnalysedType::Option(t) => InferredType::Option(Box::new(t.inner.as_ref().into())),
            AnalysedType::Result(golem_wasm_ast::analysis::TypeResult { ok, err, .. }) => {
                InferredType::Result {
                    ok: ok.as_ref().map(|t| Box::new(t.as_ref().into())),
                    error: err.as_ref().map(|t| Box::new(t.as_ref().into())),
                }
            }
            AnalysedType::Variant(vs) => InferredType::from_variant_cases(&vs),
            AnalysedType::Handle(golem_wasm_ast::analysis::TypeHandle { resource_id, mode }) => {
                InferredType::Resource {
                    resource_id: resource_id.0,
                    resource_mode: match mode {
                        AnalysedResourceMode::Owned => 0,
                        AnalysedResourceMode::Borrowed => 1,
                    },
                }
            }
        }
    }
}

mod internal {
    use crate::InferredType;

    pub(crate) fn need_update(
        current_inferred_type: &InferredType,
        new_inferred_type: &InferredType,
    ) -> bool {
        current_inferred_type != new_inferred_type && !new_inferred_type.is_unknown()
    }
}

#[cfg(test)]
mod test {

    #[test]
    fn test_flatten_one_of() {
        use super::InferredType;
        let one_of = vec![
            InferredType::U8,
            InferredType::U16,
            InferredType::U32,
            InferredType::OneOf(vec![
                InferredType::U8,
                InferredType::U16,
                InferredType::U32,
                InferredType::AllOf(vec![
                    InferredType::U64,
                    InferredType::OneOf(vec![InferredType::U64, InferredType::U8]),
                ]),
            ]),
        ];

        let flattened = InferredType::flatten_one_of_inferred_types(&one_of);

        let expected = vec![
            InferredType::U8,
            InferredType::U16,
            InferredType::U32,
            InferredType::U8,
            InferredType::U16,
            InferredType::U32,
            InferredType::AllOf(vec![
                InferredType::U64,
                InferredType::OneOf(vec![InferredType::U64, InferredType::U8]),
            ]),
        ];

        assert_eq!(flattened, expected)
    }
}
