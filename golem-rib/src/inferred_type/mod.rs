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

pub use type_internal::*;

pub(crate) use flatten::*;
pub(crate) use type_origin::*;

mod flatten;
mod type_internal;
mod type_origin;
mod unification;

use crate::instance_type::InstanceType;
use crate::rib_source_span::SourceSpan;
use crate::type_inference::GetTypeHint;
use crate::TypeName;
use bigdecimal::BigDecimal;
use golem_wasm_ast::analysis::*;
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Deref;

#[derive(Debug, Clone, Eq, PartialOrd, Ord)]
pub struct InferredType {
    pub inner: Box<TypeInternal>,
    pub origin: TypeOrigin,
}

impl InferredType {
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
                    found.push(InferredNumber::S32);
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

    pub fn default_type(inferred_type: TypeInternal) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
            origin: TypeOrigin::Default,
        }
    }

    pub fn declared_at(&mut self, source_span: SourceSpan) {
        self.origin
            .add_origin(TypeOrigin::Declared(source_span.clone()))
    }

    pub fn set_as_default(&mut self) {
        self.origin = TypeOrigin::Default;
    }

    pub fn enum_(cases: Vec<String>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Enum(cases)),
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

    pub fn flags(flags: Vec<String>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Flags(flags)),
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

    pub fn internal_type(&self) -> &TypeInternal {
        self.inner.as_ref()
    }

    pub fn list(inner: InferredType) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::List(inner)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn new(inferred_type: TypeInternal, origin: TypeOrigin) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
            origin,
        }
    }

    pub fn option(inner: InferredType) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Option(inner)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn range(from: InferredType, to: Option<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Range { from, to }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn eliminate_default(inferred_types: Vec<&InferredType>) -> Vec<&InferredType> {
        inferred_types
            .into_iter()
            .filter(|&t| !t.origin.is_default())
            .collect::<Vec<_>>()
    }

    pub fn record(fields: Vec<(String, InferredType)>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Record(fields)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn resolved(inferred_type: TypeInternal) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
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

    pub fn result(ok: Option<InferredType>, error: Option<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Result { ok, error }),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn sequence(inferred_types: Vec<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Sequence(inferred_types)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn string() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Str),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s8() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S8),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s16() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S16),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s32() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S32),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn s64() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::S64),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn tuple(inner: Vec<InferredType>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Tuple(inner)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u8() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U8),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn unit() -> InferredType {
        InferredType::tuple(vec![])
    }

    pub fn unknown() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Unknown),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u16() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U16),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u32() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U32),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn u64() -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::U64),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn variant(fields: Vec<(String, Option<InferredType>)>) -> InferredType {
        InferredType {
            inner: Box::new(TypeInternal::Variant(fields)),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn with_origin(&self, origin: TypeOrigin) -> InferredType {
        InferredType {
            inner: self.inner.clone(),
            origin,
        }
    }

    pub fn without_origin(inferred_type: TypeInternal) -> InferredType {
        InferredType {
            inner: Box::new(inferred_type),
            origin: TypeOrigin::NoOrigin,
        }
    }

    pub fn printable(&self) -> String {
        // Try a fully blown type name or if it fails,
        // get the `kind` of inferred type
        TypeName::try_from(self.clone())
            .map(|tn| tn.to_string())
            .unwrap_or(self.get_type_hint().to_string())
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

            (TypeInternal::AllOf(existing_types), _) => {
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

            (TypeInternal::OneOf(existing_types), _) => {
                if existing_types.contains(&new_inferred_type) {
                    new_inferred_type
                } else {
                    InferredType::all_of(vec![self.clone(), new_inferred_type])
                        .unwrap_or(InferredType::unknown())
                }
            }

            (_, TypeInternal::OneOf(newtypes)) => {
                if newtypes.contains(self) {
                    self.clone()
                } else {
                    InferredType::all_of(vec![self.clone(), new_inferred_type])
                        .unwrap_or(InferredType::unknown())
                }
            }

            (_, _) => InferredType::all_of(vec![self.clone(), new_inferred_type.clone()])
                .unwrap_or(InferredType::unknown()),
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

impl PartialEq for InferredType {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Hash for InferredType {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
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

impl From<&InferredNumber> for InferredType {
    fn from(inferred_number: &InferredNumber) -> Self {
        match inferred_number {
            InferredNumber::S8 => InferredType::s8(),
            InferredNumber::U8 => InferredType::u8(),
            InferredNumber::S16 => InferredType::s16(),
            InferredNumber::U16 => InferredType::u16(),
            InferredNumber::S32 => InferredType::s32(),
            InferredNumber::U32 => InferredType::u32(),
            InferredNumber::S64 => InferredType::s64(),
            InferredNumber::U64 => InferredType::u64(),
            InferredNumber::F32 => InferredType::f32(),
            InferredNumber::F64 => InferredType::f64(),
        }
    }
}

impl From<&BigDecimal> for InferredType {
    fn from(value: &BigDecimal) -> Self {
        if value.fractional_digit_count() <= 0 {
            // Rust inspired
            // https://github.com/rust-lang/rfcs/blob/master/text/0212-restore-int-fallback.md#rationale-for-the-choice-of-defaulting-to-i32
            InferredType::s32()
        } else {
            // more precision, almost same perf as f32
            InferredType::f64()
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
    use test_r::test;

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
