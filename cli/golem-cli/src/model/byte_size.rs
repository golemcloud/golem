// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::num::NonZeroU64;

pub fn parse_positive(input: &str) -> Result<NonZeroU64, String> {
    if !input.is_ascii() {
        return Err("memory size must contain only ASCII characters".to_string());
    }
    let bytes = input
        .parse::<humanize_rs::bytes::Bytes<u64>>()
        .map_err(|err| err.to_string())?;
    NonZeroU64::new(bytes.size())
        .ok_or_else(|| "memory size must be greater than zero bytes".to_string())
}

pub mod optional {
    use super::parse_positive;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::num::NonZeroU64;

    pub fn serialize<S: Serializer>(
        value: &Option<NonZeroU64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => serializer.serialize_some(&format!("{value} B")),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<NonZeroU64>, D::Error> {
        Option::<serde_json::Value>::deserialize(deserializer)?
            .map(|value| {
                let value = value.as_str().ok_or_else(|| {
                    serde::de::Error::custom("expected a byte-size string, e.g. 2GiB")
                })?;
                parse_positive(value).map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_positive;
    use test_r::test;

    #[test]
    fn positive_byte_sizes() {
        for (input, expected) in [
            ("1mb", 1_000_000),
            ("1MB", 1_000_000),
            ("1MiB", 1_048_576),
            (" 2 GiB ", 2_147_483_648),
            ("1536 MiB", 1_610_612_736),
            ("42", 42),
            ("18446744073709551615 B", u64::MAX),
        ] {
            assert_eq!(parse_positive(input).unwrap().get(), expected, "{input}");
        }
    }

    #[test]
    fn invalid_byte_sizes() {
        for input in [
            "",
            "0",
            "0 B",
            "-1 MB",
            "1.5 GiB",
            "garbage",
            "18446744073709551616 B",
            "18446744073709551615 KiB",
            "1💾 MB",
            "１ MB",
            "1\u{a0}MiB",
        ] {
            assert!(parse_positive(input).is_err(), "accepted {input}");
        }
    }
}
