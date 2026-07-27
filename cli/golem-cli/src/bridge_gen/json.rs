// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

/// Converts JSON numbers whose schema meaning requires lossless transport to
/// strings. Other numbers, including unrelated properties named `value`, are
/// deliberately left untouched.
pub(crate) fn stringify_precision_sensitive_numbers(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            let is_numeric_bound = object
                .get("kind")
                .and_then(|kind| kind.as_str())
                .is_some_and(|kind| matches!(kind, "signed" | "unsigned" | "float-bits"));
            if is_numeric_bound
                && let Some(bound_value) = object.get_mut("value")
                && bound_value.is_number()
            {
                *bound_value = serde_json::Value::String(bound_value.to_string());
            }
            if let Some(mantissa) = object.get_mut("mantissa")
                && mantissa.is_number()
            {
                *mantissa = serde_json::Value::String(mantissa.to_string());
            }
            for child in object.values_mut() {
                stringify_precision_sensitive_numbers(child);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                stringify_precision_sensitive_numbers(child);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::stringify_precision_sensitive_numbers;
    use serde_json::json;
    use test_r::test;

    #[test]
    fn stringifies_only_semantically_precision_sensitive_numbers() {
        let mut value = json!({
            "bound": { "kind": "unsigned", "value": u64::MAX },
            "quantity": { "mantissa": i64::MIN, "scale": 2 },
            "metadata": { "value": 9007199254740993_u64 },
            "other": { "kind": "metadata", "value": 9007199254740993_u64 }
        });
        stringify_precision_sensitive_numbers(&mut value);
        assert_eq!(value["bound"]["value"], json!(u64::MAX.to_string()));
        assert_eq!(value["quantity"]["mantissa"], json!(i64::MIN.to_string()));
        assert_eq!(value["metadata"]["value"], json!(9007199254740993_u64));
        assert_eq!(value["other"]["value"], json!(9007199254740993_u64));
    }
}
