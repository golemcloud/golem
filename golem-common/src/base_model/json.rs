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

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A JSON value that is always normalized for deterministic serialization and comparison.
///
/// Normalization: numbers with no fractional part are converted to integers.
/// Object keys in `serde_json` are already ordered (BTreeMap by default).
///
/// Use this wherever JSON values need to be compared or hashed deterministically
/// (e.g. agent config values, diff computation).
#[derive(Debug, Clone)]
pub struct NormalizedJsonValue(pub serde_json::Value);

impl NormalizedJsonValue {
    /// Construct by normalizing a raw JSON value.
    pub fn new(value: serde_json::Value) -> Self {
        let mut v = value;
        normalize_json(&mut v);
        NormalizedJsonValue(v)
    }

    pub fn into_inner(self) -> serde_json::Value {
        self.0
    }

    /// Normalize a raw JSON value and return the inner `serde_json::Value`.
    /// Useful when the normalized value is needed directly without the newtype wrapper.
    pub fn normalize(value: serde_json::Value) -> serde_json::Value {
        Self::new(value).0
    }
}

impl From<serde_json::Value> for NormalizedJsonValue {
    fn from(value: serde_json::Value) -> Self {
        NormalizedJsonValue::new(value)
    }
}

impl From<NormalizedJsonValue> for serde_json::Value {
    fn from(v: NormalizedJsonValue) -> Self {
        v.0
    }
}

impl PartialEq for NormalizedJsonValue {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

// serde_json::Value::PartialEq is reflexive and consistent, so Eq is sound.
impl Eq for NormalizedJsonValue {}

impl std::fmt::Display for NormalizedJsonValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for NormalizedJsonValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for NormalizedJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(NormalizedJsonValue::new(value))
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinarySerializer for NormalizedJsonValue {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        let s = self.0.to_string();
        desert_rust::BinarySerializer::serialize(&s, context)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinaryDeserializer for NormalizedJsonValue {
    fn deserialize(
        context: &mut desert_rust::DeserializationContext<'_>,
    ) -> desert_rust::Result<Self> {
        let s = <String as desert_rust::BinaryDeserializer>::deserialize(context)?;
        let value: serde_json::Value = serde_json::from_str(&s).map_err(|e| {
            desert_rust::Error::DeserializationFailure(
                format!("Invalid JSON in NormalizedJsonValue: {e}").into(),
            )
        })?;
        Ok(NormalizedJsonValue::new(value))
    }
}

/// Normalize a JSON value in place:
/// - Numbers with no fractional part are converted to integers.
pub fn normalize_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64()
                && f.fract() == 0.0
            {
                *value = serde_json::Value::Number(serde_json::Number::from(f as i64));
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                normalize_json(v);
            }
        }
        serde_json::Value::Object(map) => {
            // Object keys are assumed to be sorted. This holds as long as serde_json is
            // compiled without the `preserve_order` feature (which would use IndexMap).
            // The workspace Cargo.toml uses serde_json with `raw_value` only, so
            // serde_json::Map is BTreeMap and keys are always in sorted order.
            // *** Key ordering is required for deterministic hashing — see unit test below. ***
            for v in map.values_mut() {
                normalize_json(v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn object_keys_are_sorted_after_normalization() {
        // Build an object via serde_json's map (BTreeMap — keys come out sorted).
        // This test guards against accidentally enabling preserve_order, which would
        // break deterministic hashing.
        let json: serde_json::Value = serde_json::json!({
            "zebra": 1,
            "apple": 2,
            "mango": 3
        });
        let normalized = NormalizedJsonValue::new(json);
        let keys: Vec<&str> = normalized
            .0
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            keys,
            vec!["apple", "mango", "zebra"],
            "Object keys must be in sorted order for deterministic hashing"
        );
    }

    #[test]
    fn nested_object_keys_are_sorted() {
        let json: serde_json::Value = serde_json::json!({
            "z": { "b": 1, "a": 2 },
            "a": { "y": 3, "x": 4 }
        });
        let normalized = NormalizedJsonValue::new(json);
        let outer_keys: Vec<&str> = normalized
            .0
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(outer_keys, vec!["a", "z"]);

        let inner_a_keys: Vec<&str> = normalized.0["a"]
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(inner_a_keys, vec!["x", "y"]);
    }

    #[test]
    fn float_with_no_fractional_part_normalized_to_integer() {
        let json = serde_json::json!({ "value": 42.0 });
        let normalized = NormalizedJsonValue::new(json);
        assert_eq!(
            normalized.0["value"],
            serde_json::Value::Number(serde_json::Number::from(42i64))
        );
    }

    #[test]
    fn float_with_fractional_part_left_unchanged() {
        let json = serde_json::json!({ "value": 42.5 });
        let normalized = NormalizedJsonValue::new(json);
        assert!(normalized.0["value"].is_f64());
    }

    #[test]
    fn two_equivalent_values_are_equal_after_normalization() {
        let a = NormalizedJsonValue::new(serde_json::json!({"x": 1.0, "y": 2}));
        let b = NormalizedJsonValue::new(serde_json::json!({"y": 2, "x": 1}));
        // Both should normalize to {"x": 1, "y": 2} (sorted keys, int 1)
        assert_eq!(a, b);
    }
}
