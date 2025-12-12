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

impl<T: IntoValue> IntoValue for nonempty_collections::NEVec<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let vec: Vec<T> = self.into();
        let builder = builder.record();
        let builder = builder.item();
        vec.add_to_builder(builder).finish()
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(
            Some("NEVec".to_string()),
            Some("nonempty_collections".to_string()),
        );
        let builder = builder.field("items");
        <Vec<T>>::add_to_type_builder(builder).finish()
    }
}

impl<T: FromValueAndType> FromValueAndType for nonempty_collections::NEVec<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing items field".to_string())?;
        <Vec<T>>::from_extractor(&value).and_then(|vec| {
            nonempty_collections::NEVec::try_from_vec(vec)
                .ok_or_else(|| "Expected non-empty vector".to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::roundtrip_test;
    use proptest::strategy::Strategy;
    use test_r::test;

    roundtrip_test!(
        prop_roundtrip_nevec_u32,
        nonempty_collections::NEVec<u32>,
        proptest::collection::vec(0u32.., 1..100)
            .prop_map(|v| nonempty_collections::NEVec::try_from_vec(v).unwrap())
    );
}
