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
                Some(AnalysedType::U64)
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
                    let field_type = if key == "ok" {
                        AnalysedType::Result {
                            ok: Some(Box::new(field_type)),
                            error: None,
                        }
                    } else if key == "err" {
                        AnalysedType::Result {
                            ok: None,
                            error: Some(Box::new(field_type)),
                        }
                    } else {
                        field_type
                    };

                    fields.push((key.clone(), field_type));
                }
            }
            Some(AnalysedType::Record(fields))
        }
        _ => None,
    }
}
