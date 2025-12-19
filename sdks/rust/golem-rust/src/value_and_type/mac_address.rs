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

impl IntoValue for mac_address::MacAddress {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let bytes = self.bytes().to_vec();
        let builder = builder.record();
        let builder = builder.item();
        bytes.add_to_builder(builder).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("MacAddress".to_string()),
            Some("mac_address".to_string()),
        );
        let builder = builder.field("bytes");
        <Vec<u8>>::add_to_type_builder(builder).finish()
    }
}

impl FromValueAndType for mac_address::MacAddress {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing bytes field".to_string())?;
        <Vec<u8>>::from_extractor(&value).and_then(|bytes| {
            if bytes.len() != 6 {
                return Err("MacAddress must be 6 bytes".to_string());
            }
            Ok(mac_address::MacAddress::new([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
            ]))
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::roundtrip_test;
    use proptest::strategy::Strategy;
    use test_r::test;

    roundtrip_test!(
        prop_roundtrip_macaddress,
        mac_address::MacAddress,
        proptest::collection::vec(0u8.., 6..=6).prop_map(|bytes| mac_address::MacAddress::new([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
        ]))
    );
}
