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

//! Serde byte-size configuration for `usize` fields, using SI or IEC size strings.

use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(bytes: &usize, serializer: S) -> Result<S::Ok, S::Error> {
    let formatted = humansize::format_size(*bytes, humansize::BINARY);
    // Human-readable formatters round fractional units. Configuration defaults are
    // serialized before merging overrides, so rounded values must not change limits.
    if parse_size::parse_size(&formatted).ok() == Some(*bytes as u64) {
        serializer.serialize_str(&formatted)
    } else {
        serializer.collect_str(&format_args!("{bytes} B"))
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error> {
    let value = String::deserialize(deserializer)?;
    let size = parse_size::parse_size(value).map_err(serde::de::Error::custom)?;
    usize::try_from(size).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use test_r::test;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Config {
        #[serde(with = "super")]
        size: usize,
    }

    #[test]
    fn human_size_accepts_si_and_iec_and_rejects_invalid_values() {
        for (input, expected) in [("8 MiB", 8_388_608), ("8 MB", 8_000_000), ("1.5 KiB", 1536)] {
            let config: Config =
                serde_json::from_value(serde_json::json!({"size": input})).unwrap();
            assert_eq!(config.size, expected);
        }
        for value in [
            serde_json::json!("nonsense"),
            serde_json::json!("-1 B"),
            serde_json::json!(-1),
            serde_json::json!("18446744073709551616 B"),
        ] {
            assert!(
                serde_json::from_value::<Config>(serde_json::json!({"size": value})).is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn human_size_serialization_never_rounds_configuration() {
        for size in [0, 1, 1024, 1025, 8_388_608, 8_388_609, usize::MAX] {
            let original = Config { size };
            let json = serde_json::to_value(&original).unwrap();
            assert_eq!(serde_json::from_value::<Config>(json).unwrap(), original);
            let toml = toml::to_string(&original).unwrap();
            assert_eq!(toml::from_str::<Config>(&toml).unwrap(), original);
        }
        assert_eq!(
            serde_json::to_value(Config { size: 8_388_608 }).unwrap()["size"],
            "8 MiB"
        );
    }
}
