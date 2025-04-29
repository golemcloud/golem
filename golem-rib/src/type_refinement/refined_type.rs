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

use crate::type_refinement::{ExtractInnerType, ExtractInnerTypes, GetInferredTypeByName};
use crate::{InferredType, TypeInternal};
use std::vec::IntoIter;

#[derive(Clone, PartialEq, Debug)]
pub enum RefinedType<A> {
    AllOf(Vec<RefinedType<A>>),
    Value(A),
}

pub struct HeterogeneousCollectionType(pub Vec<InferredType>);

impl HeterogeneousCollectionType {
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl IntoIterator for HeterogeneousCollectionType {
    type Item = InferredType;
    type IntoIter = IntoIter<InferredType>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a HeterogeneousCollectionType {
    type Item = &'a InferredType;
    type IntoIter = std::slice::Iter<'a, InferredType>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a mut HeterogeneousCollectionType {
    type Item = &'a mut InferredType;
    type IntoIter = std::slice::IterMut<'a, InferredType>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}

impl HeterogeneousCollectionType {
    pub fn zip_with(&self, other: &HeterogeneousCollectionType) -> HeterogeneousCollectionType {
        let zipped = self
            .0
            .iter()
            .zip(other.0.iter())
            .map(|(a, b)| match a.internal_type() {
                TypeInternal::AllOf(types) => {
                    let mut all_ofs = types.clone();
                    all_ofs.push(b.clone());
                    InferredType::all_of(all_ofs).unwrap_or(InferredType::unknown())
                }
                _ => InferredType::all_of(vec![a.clone(), b.clone()])
                    .unwrap_or(InferredType::unknown()),
            })
            .collect::<Vec<_>>();

        HeterogeneousCollectionType(zipped)
    }
}

impl<A> RefinedType<A> {
    // Example: Given `RefinedType::AllOf(Option(x), Option(y), Option(z))`
    // this method returns `InferredType::AllOf(x, y, z)`
    // Example: Given `RefinedType::AllOf(RefinedType::Value(vec![x, y, z]))`
    // this method returns `InferredType::AllOf(x, y, z)`
    pub fn inner_type(&self) -> InferredType
    where
        A: ExtractInnerType,
    {
        match self {
            RefinedType::AllOf(inner) => {
                // Handle the nested `AllOf`
                let mut required_types = vec![];

                // Recursively call `inner_type` on the nested structure
                inner.iter().for_each(|v| {
                    required_types.push(v.inner_type());
                });

                InferredType::all_of(required_types).unwrap_or(InferredType::unknown())
            }
            RefinedType::Value(value) => {
                // Directly convert the list of values to the `InferredType`
                value.inner_type()
            }
        }
    }

    pub fn inner_types(&self) -> HeterogeneousCollectionType
    where
        A: ExtractInnerTypes,
    {
        match self {
            RefinedType::AllOf(inner) => {
                let x = inner.iter().map(|v| v.inner_types()).collect::<Vec<_>>();
                internal::combine(x, InferredType::all_of)
            }
            RefinedType::Value(value) => HeterogeneousCollectionType(value.inner_types()),
        }
    }

    // Example: Given `RefinedType::AllOf(RecordType(x -> y), RecordType(x -> z))`
    // inner_type_by_field("x") returns InferredTyp::AllOf(y, z)
    pub fn inner_type_by_name(&self, field_name: &str) -> InferredType
    where
        A: GetInferredTypeByName,
    {
        match self {
            RefinedType::AllOf(inner) => {
                let collected_types = inner
                    .iter()
                    .map(|v| v.inner_type_by_name(field_name))
                    .collect::<Vec<_>>();

                InferredType::all_of(collected_types).unwrap_or(InferredType::unknown())
            }
            RefinedType::Value(value) => {
                InferredType::all_of(value.get(field_name)).unwrap_or(InferredType::unknown())
            }
        }
    }
}

mod internal {
    use crate::type_refinement::HeterogeneousCollectionType;
    use crate::InferredType;

    // Combine takes a list of heterogeneous collection types, zips them by their positions,
    // and produces a single heterogeneous collection type.
    // Example:
    // let typ1 = Heterogeneous((U64, U32, U16));
    // let typ2 = Heterogeneous((Str, Str, Str));
    // let typ3 = Heterogeneous((AllOf(U64, Str), AllOf(U32, Str), AllOf(U16, Str)));
    pub(crate) fn combine<F>(
        input: Vec<HeterogeneousCollectionType>,
        pack: F,
    ) -> HeterogeneousCollectionType
    where
        F: Fn(Vec<InferredType>) -> Option<InferredType>,
    {
        let mut transposed = vec![];

        if let Some(first) = input.first() {
            let length = first.0.len();
            for i in 0..length {
                let mut grouped = vec![];
                for col_type in &input {
                    if let Some(inferred) = col_type.0.get(i) {
                        grouped.push(inferred.clone());
                    }
                }
                if let Some(inferred) = pack(grouped) {
                    transposed.push(inferred);
                }
            }
        }

        HeterogeneousCollectionType(transposed) // This is now correct
    }
}
