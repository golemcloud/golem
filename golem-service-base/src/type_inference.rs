use golem_wasm_ast::analysis::AnalysedType;
use serde_json::Value;

pub fn infer_analysed_type(value: &Value) -> Option<AnalysedType> {
    match value {
        Value::Bool(_) => Some(AnalysedType::Bool),
        Value::Number(n) => {
            if let Some(n) = n.as_i64() {
                if n >= i8::MIN as i64 && n <= i8::MAX as i64 {
                    Some(AnalysedType::S8)
                } else if n >= u8::MIN as i64 && n <= u8::MAX as i64 {
                    Some(AnalysedType::U8)
                } else if n >= i16::MIN as i64 && n <= i16::MAX as i64 {
                    Some(AnalysedType::S16)
                } else if n >= u16::MIN as i64 && n <= u16::MAX as i64 {
                    Some(AnalysedType::U16)
                } else if n >= i32::MIN as i64 && n <= i32::MAX as i64 {
                    Some(AnalysedType::S32)
                } else if n >= u32::MIN as i64 && n <= u32::MAX as i64 {
                    Some(AnalysedType::U32)
                } else if n >= i64::MIN && n <= i64::MAX {
                    Some(AnalysedType::S64)
                } else {
                    Some(AnalysedType::U64)
                }
            } else if let Some(n) = n.as_f64() {
                if n.fract() == 0.0 {
                    Some(AnalysedType::F64)
                } else {
                    Some(AnalysedType::F32)
                }
            } else {
                None
            }
        }
        Value::String(_) => Some(AnalysedType::Str),
        Value::Array(arr) => {
            if arr.is_empty() {
                None
            } else {
                let inferred_type = infer_analysed_type(&arr[0]);
                inferred_type.map(|t| AnalysedType::List(Box::new(t)))
            }
        }
        Value::Object(map) => {
            let mut fields = Vec::new();
            for (key, value) in map {
                if let Some(field_type) = infer_analysed_type(value) {
                    fields.push((key.clone(), field_type));
                }
            }
            Some(AnalysedType::Record(fields))
        }
        _ => None,
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
        assert_eq!(infer_analysed_type(&value), Some(AnalysedType::List(Box::new(AnalysedType::S8))));

        let value = Value::Array(vec![]);
        assert_eq!(infer_analysed_type(&value), None);

        let value = Value::Object(serde_json::Map::new());
        assert_eq!(infer_analysed_type(&value), Some(AnalysedType::Record(vec![])));

        let value = Value::Object({
            let mut map = serde_json::Map::new();
            map.insert("foo".to_string(), Value::Number(serde_json::Number::from(42)));
            map
        });
        assert_eq!(infer_analysed_type(&value), Some(AnalysedType::Record(vec![("foo".to_string(), AnalysedType::S8)])));
    }
}
```