use golem_wasm_ast::analysis::analysed_type::{
    bool, f64, field, list, option, record, result_err, result_ok, s64, str, u64,
};
use golem_wasm_ast::analysis::AnalysedType;
use serde_json::Value;

// This should be used only for testing
// that we don't need to manually create the analysed_type
// to test the happy path compilations of the worker service related rib expressions,
pub fn infer_analysed_type(value: &Value) -> AnalysedType {
    match value {
        Value::Bool(_) => bool(),
        Value::Number(n) => {
            if n.as_u64().is_some() {
                u64()
            } else if n.as_i64().is_some() {
                s64()
            } else {
                // Only other possibility in serde_json::Number ADT is f64
                f64()
            }
        }
        Value::String(value) => {
            if value.parse::<u64>().is_ok() {
                u64()
            } else if value.parse::<i64>().is_ok() {
                s64()
            } else if value.parse::<f64>().is_ok() {
                f64()
            } else if value.parse::<bool>().is_ok() {
                bool()
            } else {
                str()
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                list(str())
            } else {
                let inferred_type = infer_analysed_type(&arr[0]);
                list(inferred_type)
            }
        }
        Value::Object(map) => {
            let mut fields = Vec::new();
            for (key, value) in map {
                let field_type0 = infer_analysed_type(value);

                // We break and return as soon as we find ok or err
                if key == "ok" {
                    return result_ok(field_type0);
                } else if key == "err" {
                    return result_err(field_type0);
                } else {
                    fields.push(field(key, field_type0));
                }
            }

            record(fields)
        }
        Value::Null => option(str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_analysed_type() {
        let value = Value::Bool(true);
        assert_eq!(infer_analysed_type(&value), bool());

        let value = Value::Number(serde_json::Number::from(1));
        assert_eq!(infer_analysed_type(&value), u64());

        let value = Value::Number(serde_json::Number::from(-1));
        assert_eq!(infer_analysed_type(&value), s64());

        let value = Value::Number(serde_json::Number::from_f64(1.0).unwrap());
        assert_eq!(infer_analysed_type(&value), f64());

        let value = Value::String("foo".to_string());
        assert_eq!(infer_analysed_type(&value), str());

        let value = Value::String("1".to_string());
        assert_eq!(infer_analysed_type(&value), u64());

        let value = Value::String("-1".to_string());
        assert_eq!(infer_analysed_type(&value), s64());

        let value = Value::String("1.2".to_string());
        assert_eq!(infer_analysed_type(&value), f64());

        let value = Value::String("true".to_string());
        assert_eq!(infer_analysed_type(&value), bool());

        let value = Value::String("false".to_string());
        assert_eq!(infer_analysed_type(&value), bool());

        let value = Value::Array(vec![]);
        assert_eq!(infer_analysed_type(&value), list(str()));

        let value = Value::Array(vec![Value::String("hello".to_string())]);
        assert_eq!(infer_analysed_type(&value), list(str()));

        let value = Value::Object(serde_json::map::Map::new());
        assert_eq!(infer_analysed_type(&value), record(vec![]));

        let value = Value::Object(serde_json::map::Map::new());
        assert_eq!(infer_analysed_type(&value), record(vec![]));

        let value = Value::Null;
        assert_eq!(infer_analysed_type(&value), option(str()));

        let mut map = serde_json::map::Map::new();
        map.insert("ok".to_string(), Value::String("hello".to_string()));
        let value = Value::Object(map);
        assert_eq!(infer_analysed_type(&value), result_ok(str()));

        let mut map = serde_json::map::Map::new();
        map.insert("err".to_string(), Value::String("hello".to_string()));
        let value = Value::Object(map);
        assert_eq!(infer_analysed_type(&value), result_err(str()));
    }
}
