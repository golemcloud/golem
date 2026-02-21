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
use ::http::Uri;
use golem_wasm::{NodeBuilder, WitValueExtractor};

impl IntoValue for Uri {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = builder.item();
        builder.string(&self.to_string()).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("Uri".to_string()), Some("http".to_string()));
        let builder = builder.field("value");
        builder.string().finish()
    }
}

impl FromValueAndType for Uri {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing value field".to_string())?;
        value
            .string()
            .ok_or_else(|| "Expected string for Uri".to_string())
            .and_then(|s| {
                s.parse::<Uri>()
                    .map_err(|_| "Failed to parse Uri from string".to_string())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;
    use proptest::prop_assert_eq;
    use proptest::proptest;
    use test_r::test;

    #[test]
    fn prop_roundtrip_uri() {
        use proptest::string::string_regex;
        proptest!(|(uri_str in string_regex("https://[a-z0-9]+(\\.[a-z0-9]+)*(/[a-z0-9]*)?").unwrap())| {
            let value = uri_str.parse::<Uri>().unwrap();
            let typ = Uri::get_type();
            let value_and_type = ValueAndType {
                value: value.clone().into_value(),
                typ,
            };
            let recovered = Uri::from_value_and_type(value_and_type)
                .expect("roundtrip conversion should succeed");
            prop_assert_eq!(recovered, value);
        });
    }
}
