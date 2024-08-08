use golem_wasm_ast::analysis::{
    AnalysedType, NameTypePair, TypeBool, TypeF64, TypeList, TypeOption, TypeRecord, TypeResult,
    TypeS64, TypeStr, TypeU64,
};
use serde_json::Value;

// This not to be used unless necessary
// This is mostly used in worker-bridge request body resolution where users
// can send arbitrary json to without specifying type.
// Note that json::null is inferred as AnalysedType::Option(Box<AnalysedType::Str>)
// and empty array is inferred AnalysedType:List(Box<AnalysedType::Str>)
pub fn infer_analysed_type(value: &Value) -> AnalysedType {
    match value {
        Value::Bool(_) => AnalysedType::Bool(TypeBool),
        Value::Number(n) => {
            if n.as_u64().is_some() {
                AnalysedType::U64(TypeU64)
            } else if n.as_i64().is_some() {
                AnalysedType::S64(TypeS64)
            } else {
                // Only other possibility in serde_json::Number ADT is f64
                AnalysedType::F64(TypeF64)
            }
        }
        Value::String(value) => {
            if value.parse::<u64>().is_ok() {
                AnalysedType::U64(TypeU64)
            } else if value.parse::<i64>().is_ok() {
                AnalysedType::S64(TypeS64)
            } else if value.parse::<f64>().is_ok() {
                AnalysedType::F64(TypeF64)
            } else if value.parse::<bool>().is_ok() {
                AnalysedType::Bool(TypeBool)
            } else {
                AnalysedType::Str(TypeStr)
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                })
            } else {
                let inferred_type = infer_analysed_type(&arr[0]);
                AnalysedType::List(TypeList {
                    inner: Box::new(inferred_type),
                })
            }
        }
        Value::Object(map) => {
            let mut fields = Vec::new();
            for (key, value) in map {
                let field_type0 = infer_analysed_type(value);

                // We break and return as soon as we find ok or err
                if key == "ok" {
                    return AnalysedType::Result(TypeResult {
                        ok: Some(Box::new(field_type0)),
                        err: None,
                    });
                } else if key == "err" {
                    return AnalysedType::Result(TypeResult {
                        ok: None,
                        err: Some(Box::new(field_type0)),
                    });
                } else {
                    fields.push(NameTypePair {
                        name: key.clone(),
                        typ: field_type0,
                    });
                }
            }

            AnalysedType::Record(TypeRecord { fields })
        }
        Value::Null => AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::Str(TypeStr)),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_analysed_type() {
        let value = Value::Bool(true);
        assert_eq!(infer_analysed_type(&value), AnalysedType::Bool(TypeBool));

        let value = Value::Number(serde_json::Number::from(1));
        assert_eq!(infer_analysed_type(&value), AnalysedType::U64(TypeU64));

        let value = Value::Number(serde_json::Number::from(-1));
        assert_eq!(infer_analysed_type(&value), AnalysedType::S64(TypeS64));

        let value = Value::Number(serde_json::Number::from_f64(1.0).unwrap());
        assert_eq!(infer_analysed_type(&value), AnalysedType::F64(TypeF64));

        let value = Value::String("foo".to_string());
        assert_eq!(infer_analysed_type(&value), AnalysedType::Str(TypeStr));

        let value = Value::String("1".to_string());
        assert_eq!(infer_analysed_type(&value), AnalysedType::U64(TypeU64));

        let value = Value::String("-1".to_string());
        assert_eq!(infer_analysed_type(&value), AnalysedType::S64(TypeS64));

        let value = Value::String("1.2".to_string());
        assert_eq!(infer_analysed_type(&value), AnalysedType::F64(TypeF64));

        let value = Value::String("true".to_string());
        assert_eq!(infer_analysed_type(&value), AnalysedType::Bool(TypeBool));

        let value = Value::String("false".to_string());
        assert_eq!(infer_analysed_type(&value), AnalysedType::Bool(TypeBool));

        let value = Value::Array(vec![]);
        assert_eq!(
            infer_analysed_type(&value),
            AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::Str(TypeStr))
            })
        );

        let value = Value::Array(vec![Value::String("hello".to_string())]);
        assert_eq!(
            infer_analysed_type(&value),
            AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::Str(TypeStr))
            })
        );

        let value = Value::Object(serde_json::map::Map::new());
        assert_eq!(
            infer_analysed_type(&value),
            AnalysedType::Record(TypeRecord { fields: vec![] })
        );

        let value = Value::Object(serde_json::map::Map::new());
        assert_eq!(
            infer_analysed_type(&value),
            AnalysedType::Record(TypeRecord { fields: vec![] })
        );

        let value = Value::Null;
        assert_eq!(
            infer_analysed_type(&value),
            AnalysedType::Option(TypeOption {
                inner: Box::new(AnalysedType::Str(TypeStr))
            })
        );

        let mut map = serde_json::map::Map::new();
        map.insert("ok".to_string(), Value::String("hello".to_string()));
        let value = Value::Object(map);
        assert_eq!(
            infer_analysed_type(&value),
            AnalysedType::Result(TypeResult {
                ok: Some(Box::new(AnalysedType::Str(TypeStr))),
                err: None,
            })
        );

        let mut map = serde_json::map::Map::new();
        map.insert("err".to_string(), Value::String("hello".to_string()));
        let value = Value::Object(map);
        assert_eq!(
            infer_analysed_type(&value),
            AnalysedType::Result(TypeResult {
                ok: None,
                err: Some(Box::new(AnalysedType::Str(TypeStr))),
            })
        );
    }
}
