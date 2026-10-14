use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Mapping {
    suffix: String,
    path: Vec<String>,
    kind: Kind,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum Kind {
    String,
    Integer,
    Boolean,
}

pub(super) fn collect(root: &Value) -> Result<Vec<Mapping>, String> {
    let mut mappings = Vec::new();
    let mut names = BTreeSet::new();
    scan(root, &[], true, &mut mappings, &mut names)?;
    Ok(mappings)
}

fn scan(
    schema: &Value,
    path: &[String],
    properties_reachable: bool,
    mappings: &mut Vec<Mapping>,
    names: &mut BTreeSet<String>,
) -> Result<(), String> {
    let Some(object) = schema.as_object() else {
        return Ok(());
    };
    if let Some(annotation) = object.get("x-mcp-header") {
        if path.is_empty() || !properties_reachable {
            return Err("x-mcp-header is only permitted on properties-chain paths".into());
        }
        let suffix = annotation.as_str().ok_or("x-mcp-header must be a string")?;
        if suffix.is_empty() || !suffix.bytes().all(is_tchar) {
            return Err("x-mcp-header must be a nonempty HTTP token".into());
        }
        if !names.insert(suffix.to_ascii_lowercase()) {
            return Err("x-mcp-header values must be case-insensitively unique".into());
        }
        let kind = match object.get("type").and_then(Value::as_str) {
            Some("string") => Kind::String,
            Some("integer") => Kind::Integer,
            Some("boolean") => Kind::Boolean,
            _ => {
                return Err(
                    "x-mcp-header property must have type string, integer, or boolean".into(),
                );
            }
        };
        mappings.push(Mapping {
            suffix: suffix.into(),
            path: path.into(),
            kind,
        });
    }

    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        for (name, child) in properties {
            let mut child_path = path.to_vec();
            child_path.push(name.clone());
            scan(child, &child_path, properties_reachable, mappings, names)?;
        }
    }
    for keyword in [
        "$defs",
        "definitions",
        "patternProperties",
        "dependentSchemas",
    ] {
        if let Some(schemas) = object.get(keyword).and_then(Value::as_object) {
            for child in schemas.values() {
                scan(child, path, false, mappings, names)?;
            }
        }
    }
    for keyword in [
        "additionalProperties",
        "additionalItems",
        "unevaluatedProperties",
        "unevaluatedItems",
        "propertyNames",
        "items",
        "contains",
        "not",
        "if",
        "then",
        "else",
        "contentSchema",
    ] {
        if let Some(child) = object.get(keyword) {
            scan(child, path, false, mappings, names)?;
        }
    }
    if let Some(dependencies) = object.get("dependencies").and_then(Value::as_object) {
        for child in dependencies.values().filter(|value| !value.is_array()) {
            scan(child, path, false, mappings, names)?;
        }
    }
    for keyword in ["prefixItems", "allOf", "anyOf", "oneOf"] {
        if let Some(children) = object.get(keyword).and_then(Value::as_array) {
            for child in children {
                scan(child, path, false, mappings, names)?;
            }
        }
    }
    Ok(())
}

fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

pub(super) fn extract(
    mappings: &[Mapping],
    arguments: &Value,
) -> Result<Vec<(String, String)>, String> {
    let mut result = Vec::new();
    for mapping in mappings {
        let Some(value) = at_path(arguments, &mapping.path) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let plain = match mapping.kind {
            Kind::String => value.as_str().ok_or_else(|| bad(mapping, "string"))?.into(),
            Kind::Boolean => value
                .as_bool()
                .map(|v| v.to_string())
                .ok_or_else(|| bad(mapping, "boolean"))?,
            Kind::Integer => {
                exact_safe_integer(value).ok_or_else(|| bad(mapping, "safe integer"))?
            }
        };
        result.push((format!("Mcp-Param-{}", mapping.suffix), encode(&plain)));
    }
    Ok(result)
}

fn at_path<'a>(mut value: &'a Value, path: &[String]) -> Option<&'a Value> {
    for part in path {
        value = value.as_object()?.get(part)?;
    }
    Some(value)
}

fn bad(mapping: &Mapping, expected: &str) -> String {
    format!("parameter {} must be a {expected}", mapping.path.join("."))
}

fn exact_safe_integer(value: &Value) -> Option<String> {
    const MAX: i128 = 9_007_199_254_740_991;
    let lexical = value.as_number()?.to_string();
    let (negative, unsigned) = lexical
        .strip_prefix('-')
        .map_or((false, lexical.as_str()), |rest| (true, rest));
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or(Some((unsigned, 0)), |(mantissa, exponent)| {
            exponent.parse::<i32>().ok().map(|e| (mantissa, e))
        })?;
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits = format!("{whole}{fraction}");
    let decimal_position = i64::try_from(whole.len()).ok()? + i64::from(exponent);
    if decimal_position < 0 {
        return digits.bytes().all(|b| b == b'0').then(|| "0".into());
    }
    let decimal_position = usize::try_from(decimal_position).ok()?;
    if decimal_position < digits.len() {
        if !digits[decimal_position..].bytes().all(|b| b == b'0') {
            return None;
        }
        digits.truncate(decimal_position);
    } else {
        digits.extend(std::iter::repeat_n('0', decimal_position - digits.len()));
    }
    let magnitude = digits.parse::<i128>().ok()?;
    (magnitude <= MAX).then(|| {
        if negative && magnitude != 0 {
            format!("-{magnitude}")
        } else {
            magnitude.to_string()
        }
    })
}

pub(crate) fn encode(value: &str) -> String {
    let bytes = value.as_bytes();
    let safe = bytes.iter().all(|b| (0x20..=0x7e).contains(b))
        && !bytes.first().is_some_and(u8::is_ascii_whitespace)
        && !bytes.last().is_some_and(u8::is_ascii_whitespace)
        && !(value.starts_with("=?base64?") && value.ends_with("?="));
    if safe {
        value.into()
    } else {
        format!("=?base64?{}?=", STANDARD.encode(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use test_r::test;

    #[test]
    fn nested_properties_extract_and_encode_values() {
        let schema = json!({"type":"object","properties":{
            "tenant":{"type":"object","properties":{
                "id":{"type":"integer","x-mcp-header":"Tenant"},
                "greeting":{"type":"string","x-mcp-header":"Greeting"},
                "flag":{"type":"boolean","x-mcp-header":"Flag"},
                "missing":{"type":"string","x-mcp-header":"Missing"}
            }}
        }});
        let mappings = collect(&schema).unwrap();
        assert_eq!(extract(&mappings, &json!({"tenant":{"id":9007199254740991_i64,"greeting":"Hello, 世界","flag":false,"missing":null}})).unwrap(), vec![
            ("Mcp-Param-Flag".into(), "false".into()),
            ("Mcp-Param-Greeting".into(), "=?base64?SGVsbG8sIOS4lueVjA==?=".into()),
            ("Mcp-Param-Tenant".into(), "9007199254740991".into()),
        ]);
        assert!(
            extract(
                &mappings,
                &json!({"tenant":{"id":9007199254740992_u64,"greeting":"ok","flag":true}})
            )
            .unwrap_err()
            .contains("safe integer")
        );
    }

    #[test]
    fn unsafe_strings_use_the_unambiguous_sentinel() {
        let schema =
            json!({"type":"object","properties":{"x":{"type":"string","x-mcp-header":"X"}}});
        let mappings = collect(&schema).unwrap();
        assert_eq!(encode("a\tb"), "=?base64?YQli?=");
        for value in [" padded ", "line\nfeed", "=?base64?literal?="] {
            assert!(
                extract(&mappings, &json!({"x":value})).unwrap()[0]
                    .1
                    .starts_with("=?base64?")
            );
        }
        assert_eq!(
            extract(&mappings, &json!({})).unwrap(),
            Vec::<(String, String)>::new()
        );
    }

    #[test]
    fn rejects_annotations_outside_property_chains_and_malformed_annotations() {
        for schema in [
            json!({"type":"object","x-mcp-header":"Root"}),
            json!({"type":"object","properties":{"x":{"allOf":[{"type":"string","x-mcp-header":"X"}]}}}),
            json!({"type":"object","properties":{"x":{"items":{"type":"string","x-mcp-header":"X"}}}}),
            json!({"type":"object","properties":{"x":{"$ref":"#/$defs/x"}},"$defs":{"x":{"type":"string","x-mcp-header":"X"}}}),
            json!({"type":"object","properties":{"x":{"type":"number","x-mcp-header":"X"}}}),
            json!({"type":"object","properties":{"x":{"type":"string","x-mcp-header":""}}}),
            json!({"type":"object","properties":{"x":{"type":"string","x-mcp-header":3}}}),
            json!({"type":"object","properties":{"x":{"type":"string","x-mcp-header":"bad name"}}}),
        ] {
            assert!(collect(&schema).is_err(), "{schema}");
        }
        assert!(collect(&json!({"type":"object","properties":{"x":{"type":"string","x-mcp-header":"Name"},"y":{"type":"boolean","x-mcp-header":"name"}}})).unwrap_err().contains("unique"));
    }

    #[test]
    fn ignores_annotation_looking_data_in_non_schema_keywords() {
        let schema = json!({"type":"object","properties":{"x":{"type":"string","enum":[{"x-mcp-header":"not-a-schema"}],"default":{"x-mcp-header":"also-data"},"examples":[{"x-mcp-header":"data"}]}}});
        assert!(collect(&schema).unwrap().is_empty());
    }
}
