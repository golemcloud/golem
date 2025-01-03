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
pub(crate) use unification_result::*;
pub(crate) use validation::*;
mod flatten;
mod unification;
mod unification_result;
mod validation;

use std::collections::HashSet;

use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::*;

#[derive(Debug, Hash, Clone, Eq, PartialEq, PartialOrd, Ord, Encode, Decode)]
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
    OneOf(Vec<InferredType>),
    AllOf(Vec<InferredType>),
    Unknown,
    // Because function result can be a vector of types
    Sequence(Vec<InferredType>),
}

impl InferredType {
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
            let mut unique_one_of_types: Vec<InferredType> = unique_types.into_iter().collect(); // Step 1: Col
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
        unification::unify(self)
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
                one_of_types.extend(existing_types.clone());

                InferredType::one_of(one_of_types).unwrap_or(InferredType::Unknown)
            }

            (InferredType::OneOf(_), new_type) => {
                InferredType::all_of(vec![self.clone(), new_type]).unwrap_or(InferredType::Unknown)
            }

            (current_type, InferredType::OneOf(newtypes)) => {
                InferredType::all_of(vec![current_type.clone(), InferredType::OneOf(newtypes)])
                    .unwrap_or(InferredType::Unknown)
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
                    name_type_pair.typ.clone().map(|t| t.into()),
                )
            })
            .collect();

        InferredType::Variant(cases)
    }

    pub fn from_enum_cases(type_enum: &TypeEnum) -> InferredType {
        InferredType::Enum(type_enum.cases.clone())
    }
}

impl From<AnalysedType> for InferredType {
    fn from(analysed_type: AnalysedType) -> Self {
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
            AnalysedType::List(t) => InferredType::List(Box::new((*t.inner).into())),
            AnalysedType::Tuple(ts) => {
                InferredType::Tuple(ts.items.into_iter().map(|t| t.into()).collect())
            }
            AnalysedType::Record(fs) => InferredType::Record(
                fs.fields
                    .into_iter()
                    .map(|name_type| (name_type.name, name_type.typ.into()))
                    .collect(),
            ),
            AnalysedType::Flags(vs) => InferredType::Flags(vs.names),
            AnalysedType::Enum(vs) => InferredType::from_enum_cases(&vs),
            AnalysedType::Option(t) => InferredType::Option(Box::new((*t.inner).into())),
            AnalysedType::Result(golem_wasm_ast::analysis::TypeResult { ok, err, .. }) => {
                InferredType::Result {
                    ok: ok.map(|t| Box::new((*t).into())),
                    error: err.map(|t| Box::new((*t).into())),
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
