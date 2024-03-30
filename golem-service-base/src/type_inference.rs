use golem_wasm_ast::analysis::AnalysedType;
use serde_json::Value;

// This not to be used unless necessary
// This is mostly used in worker-bridge request body resolution where users
// can send arbitrary json to without specifying type.
// Note that json::null is inferred as AnalysedType::Option(Box<AnalysedType::Str>)
// and empty array is inferred AnalysedType:List(Box<AnalysedType::Str>)
pub fn infer_analysed_type(value: &Value) -> AnalysedType {
    match value {
        Value::Bool(_) => AnalysedType::Bool,
        Value::Number(n) => {
            if let Some(n) = n.as_i64() {
                if n >= i8::MIN as i64 && n <= i8::MAX as i64 {
                    AnalysedType::S8
                } else if n >= u8::MIN as i64 && n <= u8::MAX as i64 {
                    AnalysedType::U8
                } else if n >= i16::MIN as i64 && n <= i16::MAX as i64 {
                    AnalysedType::S16
                } else if n >= u16::MIN as i64 && n <= u16::MAX as i64 {
                    AnalysedType::U16
                } else if n >= i32::MIN as i64 && n <= i32::MAX as i64 {
                    AnalysedType::S32
                } else if n >= u32::MIN as i64 && n <= u32::MAX as i64 {
                    AnalysedType::U32
                } else if n >= i64::MIN && n <= i64::MAX {
                    AnalysedType::S64
                } else {
                    AnalysedType::U64
                }
            } else if let Some(n) = n.as_f64() {
                if n.fract() == 0.0 {
                    AnalysedType::F64
                } else {
                    AnalysedType::F32
                }
            } else {
                AnalysedType::U64
            }
        }
        Value::String(_) => AnalysedType::Str,
        Value::Array(arr) => {
            if arr.is_empty() {
                AnalysedType::List(Box::new(AnalysedType::Str))
            } else {
                let inferred_type = infer_analysed_type(&arr[0]);
                AnalysedType::List(Box::new(inferred_type))
            }
        }
        Value::Object(map) => {
            let mut fields = Vec::new();
            for (key, value) in map {
                let field_type0 = infer_analysed_type(value);

                // We break and return as soon as we find ok or err
                if key == "ok" {
                    return AnalysedType::Result {
                        ok: Some(Box::new(field_type0)),
                        error: None,
                    }
                } else if key == "err" {
                    return AnalysedType::Result {
                        ok: None,
                        error: Some(Box::new(field_type0)),
                    }
                } else {
                    fields.push((key.clone(), field_type0));
                }
            }

            AnalysedType::Record(fields)
        }
        Value::Null => AnalysedType::Option(Box::new(AnalysedType::Str)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_analysed_type() {
        let value = Value::Bool(true);
        assert_eq!(infer_analysed_type(&value), Some(AnalysedType::Bool));

        let value = Value::Number(serde_json::Number::from(42));
        assert_eq!(infer_analysed_type(&value), Some(AnalysedType::S8));

        let value = Value::Number(serde_json::Number::from(42.0));
        assert_eq!(infer_analysed_type(&value), Some(AnalysedType::F64));

        let value = Value::String("hello".to_string());
        assert_eq!(infer_analysed_type(&value), Some(AnalysedType::Str));

        let value = Value::Array(vec![Value::Number(serde_json::Number::from(42))]);
        assert_eq!(
            infer_analysed_type(&value),
            Some(AnalysedType::List(Box::new(AnalysedType::S8)))
        );

        let value = Value::Array(vec![]);
        assert_eq!(infer_analysed_type(&value), None);

        let value = Value::Object(serde_json::Map::new());
        assert_eq!(
            infer_analysed_type(&value),
            Some(AnalysedType::Record(vec![]))
        );

        let value = Value::Object({
            let mut map = serde_json::Map::new();
            map.insert(
                "foo".to_string(),
                Value::Number(serde_json::Number::from(42)),
            );
            map
        });
        assert_eq!(
            infer_analysed_type(&value),
            Some(AnalysedType::Record(vec![(
                "foo".to_string(),
                AnalysedType::S8
            )]))
        );
    }
}
