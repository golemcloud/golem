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
        return Err("byte size must contain only ASCII characters".to_string());
    }
    let bytes = input
        .parse::<humanize_rs::bytes::Bytes<u64>>()
        .map_err(|err| err.to_string())?;
    NonZeroU64::new(bytes.size())
        .ok_or_else(|| "byte size must be greater than zero bytes".to_string())
}

pub mod required {
    use super::parse_positive;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &usize, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("{value} B"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error> {
        let value = String::deserialize(deserializer)?;
        let bytes = parse_positive(&value).map_err(serde::de::Error::custom)?;
        usize::try_from(bytes.get()).map_err(serde::de::Error::custom)
    }
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
mod tests;
