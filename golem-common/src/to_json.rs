use golem_api_grpc::proto::golem::common::json_value::Kind;
use golem_api_grpc::proto::golem::common::{JsonArray, JsonObject, JsonValue as ProtoJson};
use serde_json::{Number, Value};
use std::collections::HashMap;

pub trait FromToJson {
    fn to_serde_json_value(&self) -> Value;
    fn from_serde_json_value(input: &Value) -> Self;
}

impl FromToJson for ProtoJson {
    fn to_serde_json_value(&self) -> Value {
        match self.kind {
            Some(Kind::NullValue(_)) => Value::Null,
            Some(Kind::PosIntValue(ref v)) => Value::Number(serde_json::Number::from(v.value)),
            Some(Kind::NegIntValue(ref v)) => Value::Number(serde_json::Number::from(v.value)),
            Some(Kind::NumberValue(v)) => Value::Number(Number::from_f64(v).unwrap()),
            Some(Kind::StringValue(ref v)) => Value::String(v.clone()),
            Some(Kind::BoolValue(v)) => Value::Bool(v),
            Some(Kind::ArrayValue(ref v)) => {
                Value::Array(v.values.iter().map(|x| x.to_serde_json_value()).collect())
            }
            Some(Kind::ObjectValue(ref v)) => Value::Object(
                v.fields
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_serde_json_value()))
                    .collect(),
            ),
            None => Value::Null,
        }
    }

    fn from_serde_json_value(input: &Value) -> Self {
        let kind = match input {
            Value::Null => Kind::NullValue(0),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    if i >= 0 {
                        Kind::PosIntValue(golem_api_grpc::proto::golem::common::PosIntValue {
                            value: i as u64,
                        })
                    } else {
                        Kind::NegIntValue(golem_api_grpc::proto::golem::common::NegIntValue {
                            value: i,
                        })
                    }
                } else {
                    Kind::NumberValue(n.as_f64().unwrap())
                }
            }
            Value::String(s) => Kind::StringValue(s.clone()),
            Value::Bool(b) => Kind::BoolValue(*b),
            Value::Array(a) => Kind::ArrayValue(JsonArray {
                values: a.iter().map(ProtoJson::from_serde_json_value).collect(),
            }),
            Value::Object(o) => Kind::ObjectValue(JsonObject {
                fields: {
                    let mut map = HashMap::new();

                    for (k, v) in o.iter() {
                        map.insert(k.clone(), ProtoJson::from_serde_json_value(v));
                    }

                    map
                },
            }),
        };
        ProtoJson { kind: Some(kind) }
    }
}
