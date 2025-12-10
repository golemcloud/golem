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

impl IntoValue for bytes::Bytes {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut list_builder = builder.list();
        for byte in self.iter() {
            list_builder = byte.add_to_builder(list_builder.item());
        }
        list_builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        u8::add_to_type_builder(builder.list(None, None)).finish()
    }
}

impl FromValueAndType for bytes::Bytes {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .list_elements(|elem| u8::from_extractor(&elem))
            .ok_or_else(|| "Expected list for Bytes".to_string())
            .and_then(|list| list.into_iter().collect::<Result<Vec<u8>, String>>())
            .map(bytes::Bytes::from)
    }
}

#[cfg(test)]
mod tests {
    use crate::roundtrip_test;
    use proptest::strategy::Strategy;
    use test_r::test;

    roundtrip_test!(
        prop_roundtrip_bytes,
        bytes::Bytes,
        proptest::collection::vec(0u8.., 0..100).prop_map(bytes::Bytes::from)
    );
}
