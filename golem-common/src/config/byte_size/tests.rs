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

use super::parse_positive;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
use test_r::test;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Config {
    #[serde(with = "super::required")]
    required: usize,
    #[serde(default, with = "super::optional")]
    optional: Option<NonZeroU64>,
}

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

#[test]
fn serde_adapters_share_positive_integer_syntax() {
    for (input, expected) in [("8 MiB", 8_388_608), ("8 MB", 8_000_000), ("1536 B", 1536)] {
        let config: Config = serde_json::from_value(serde_json::json!({
            "required": input,
            "optional": input,
        }))
        .unwrap();
        assert_eq!(config.required, expected);
        assert_eq!(config.optional.unwrap().get(), expected as u64);
    }
    for value in [
        serde_json::json!("0 B"),
        serde_json::json!("1.5 KiB"),
        serde_json::json!("nonsense"),
        serde_json::json!("-1 B"),
        serde_json::json!(1),
        serde_json::json!("18446744073709551616 B"),
        serde_json::json!("18446744073709551615 KiB"),
    ] {
        for field in ["required", "optional"] {
            let mut json = serde_json::json!({"required": "1 B", "optional": "1 B"});
            json[field] = value.clone();
            assert!(
                serde_json::from_value::<Config>(json).is_err(),
                "accepted {field}: {value}"
            );
        }
    }
    assert!(serde_json::from_value::<Config>(serde_json::json!({"required": null})).is_err());
    assert!(serde_json::from_value::<Config>(serde_json::json!({})).is_err());
    for json in [
        serde_json::json!({"required": "1 B"}),
        serde_json::json!({"required": "1 B", "optional": null}),
    ] {
        assert_eq!(
            serde_json::from_value::<Config>(json).unwrap().optional,
            None
        );
    }
}

#[test]
fn serde_serialization_preserves_exact_bytes() {
    for size in [1, 1024, 1025, 8_388_608, 8_388_609, usize::MAX] {
        let original = Config {
            required: size,
            optional: NonZeroU64::new(size as u64),
        };
        let json = serde_json::to_value(&original).unwrap();
        assert_eq!(json["required"], format!("{size} B"));
        assert_eq!(json["optional"], format!("{size} B"));
        assert_eq!(serde_json::from_value::<Config>(json).unwrap(), original);
        let toml = toml::to_string(&original).unwrap();
        assert_eq!(toml::from_str::<Config>(&toml).unwrap(), original);
    }
    let config = Config {
        required: 1,
        optional: None,
    };
    let json = serde_json::to_value(&config).unwrap();
    assert!(json["optional"].is_null());
    assert_eq!(serde_json::from_value::<Config>(json).unwrap(), config);
}
