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

use crate::{InferredType, TypeInternal};
use std::collections::HashSet;

pub fn flatten_all_of_list(types: &Vec<InferredType>) -> Vec<InferredType> {
    let mut all_of_types = vec![];
    let mut seen = HashSet::new();

    for typ in types {
        match typ.internal_type() {
            TypeInternal::AllOf(all_of) => {
                let flattened = flatten_all_of_list(all_of);
                for t in flattened {
                    if seen.insert(t.clone()) {
                        all_of_types.push(t);
                    }
                }
            }
            _ => {
                all_of_types.push(typ.clone());
            }
        }
    }

    all_of_types
}
