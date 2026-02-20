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

use super::type_builder::TypeNodeBuilder;
use crate::value_and_type::{FromValueAndType, IntoValue};
use golem_wasm::{NodeBuilder, WitValueExtractor};

impl IntoValue for bit_vec::BitVec {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let vec: Vec<bool> = self.iter().collect();
        let builder = builder.record();
        let builder = builder.item();
        vec.add_to_builder(builder).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("BitVec".to_string()), Some("bit_vec".to_string()));
        let builder = builder.field("value");
        <Vec<bool>>::add_to_type_builder(builder).finish()
    }
}

impl FromValueAndType for bit_vec::BitVec {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing value field".to_string())?;
        <Vec<bool>>::from_extractor(&value).map(|v| v.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_wasm::golem_core_1_5_x::types::ValueAndType;
    use proptest::prop_assert_eq;
    use proptest::proptest;
    use test_r::test;

    #[test]
    fn prop_roundtrip_bitvec() {
        proptest!(|(vec in proptest::collection::vec(proptest::bool::ANY, 0..100))| {
            let value: bit_vec::BitVec = vec.into_iter().collect();
            let typ = bit_vec::BitVec::get_type();
            let value_and_type = ValueAndType {
                value: value.clone().into_value(),
                typ,
            };
            let recovered = bit_vec::BitVec::from_value_and_type(value_and_type)
                .expect("roundtrip conversion should succeed");
            prop_assert_eq!(recovered, value);
        });
    }
}
