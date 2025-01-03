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

use crate::type_refinement::precise_types::{
    ErrType, ListType, OkType, OptionalType, RecordType, TupleType, VariantType,
};
use crate::InferredType;

pub trait ExtractInnerType {
    fn inner_type(&self) -> InferredType;
}

impl ExtractInnerType for OptionalType {
    fn inner_type(&self) -> InferredType {
        self.0.clone()
    }
}

impl ExtractInnerType for OkType {
    fn inner_type(&self) -> InferredType {
        self.0.clone()
    }
}

impl ExtractInnerType for ErrType {
    fn inner_type(&self) -> InferredType {
        self.0.clone()
    }
}

impl ExtractInnerType for ListType {
    fn inner_type(&self) -> InferredType {
        self.0.clone()
    }
}

pub trait ExtractInnerTypes {
    fn inner_types(&self) -> Vec<InferredType>;
}

impl ExtractInnerTypes for TupleType {
    fn inner_types(&self) -> Vec<InferredType> {
        self.0.clone()
    }
}

// While many types allow simple extraction of inner field,
// certain types requires looking up by a index or a field name.
// Further-more, there is no guarantee that the type associated with that field
// is a singleton
pub trait GetInferredTypeByName {
    fn get(&self, name: &str) -> Vec<InferredType>;
}

impl GetInferredTypeByName for RecordType {
    fn get(&self, field_name: &str) -> Vec<InferredType> {
        self.0
            .iter()
            .filter_map(|(name, typ)| {
                if name == field_name {
                    Some(typ.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

impl GetInferredTypeByName for VariantType {
    fn get(&self, name: &str) -> Vec<InferredType> {
        self.0
            .iter()
            .filter_map(|(n, typ)| if n == name { typ.clone() } else { None })
            .collect()
    }
}
