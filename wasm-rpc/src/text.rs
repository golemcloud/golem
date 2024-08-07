use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
use crate::protobuf::typed_result::ResultValue;
use crate::protobuf::{
    NameValuePair, TypedEnum, TypedFlags, TypedList, TypedOption, TypedRecord, TypedTuple,
    TypedVariant,
};
use crate::protobuf::{TypeAnnotatedValue as RootTypeAnnotatedValue, TypedResult};
use golem_wasm_ast::analysis::{protobuf, TypeEnum, TypeFlags};
use golem_wasm_ast::analysis::{AnalysedType, TypeList, TypeRecord, TypeTuple, TypeVariant};
use std::borrow::Cow;
use std::ops::Deref;
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue, WasmValueError};
use wasm_wave::{from_str, to_string};

pub fn type_annotated_value_from_str(
    analysed_type: &AnalysedType,
    input: &str,
) -> Result<TypeAnnotatedValue, String> {
    let parsed_typed_value: TypeAnnotatedValuePrintable =
        from_str(analysed_type, input).map_err(|err| err.to_string())?;

    Ok(parsed_typed_value.0)
}

pub fn type_annotated_value_to_string(value: &TypeAnnotatedValue) -> Result<String, String> {
    let printable_typed_value: TypeAnnotatedValuePrintable =
        TypeAnnotatedValuePrintable(value.clone());

    let typed_value_str = to_string(&printable_typed_value).map_err(|err| err.to_string())?;

    Ok(typed_value_str)
}

#[derive(Debug, Clone)]
pub struct TypeAnnotatedValuePrintable(pub TypeAnnotatedValue);

impl WasmValue for TypeAnnotatedValuePrintable {
    type Type = AnalysedType;

    fn kind(&self) -> WasmTypeKind {
        let analysed_type = AnalysedType::try_from(&self.0)
            .expect("Failed to retrieve AnalysedType from TypeAnnotatedValue");
        analysed_type.kind()
    }

    fn make_bool(val: bool) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::Bool(val))
    }

    fn make_s8(val: i8) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::S8(val as i32))
    }

    fn make_s16(val: i16) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::S16(val as i32))
    }

    fn make_s32(val: i32) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::S32(val))
    }

    fn make_s64(val: i64) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::S64(val))
    }

    fn make_u8(val: u8) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::U8(val as u32))
    }

    fn make_u16(val: u16) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::U16(val as u32))
    }

    fn make_u32(val: u32) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::U32(val))
    }

    fn make_u64(val: u64) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::U64(val))
    }

    fn make_float32(val: f32) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::F32(val))
    }

    fn make_float64(val: f64) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::F64(val))
    }

    fn make_char(val: char) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::Char(val as i32))
    }

    fn make_string(val: Cow<str>) -> Self {
        TypeAnnotatedValuePrintable(TypeAnnotatedValue::Str(val.to_string()))
    }

    fn make_list(
        ty: &Self::Type,
        vals: impl IntoIterator<Item = Self>,
    ) -> Result<Self, WasmValueError> {
        if let AnalysedType::List(TypeList { inner: typ }) = ty {
            let list = TypedList {
                values: vals
                    .into_iter()
                    .map(|v| RootTypeAnnotatedValue {
                        type_annotated_value: Some(v.0),
                    })
                    .collect(),
                typ: Some(typ.deref().into()),
            };

            Ok(TypeAnnotatedValuePrintable(TypeAnnotatedValue::List(list)))
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }

    fn make_record<'a>(
        ty: &Self::Type,
        fields: impl IntoIterator<Item = (&'a str, Self)>,
    ) -> Result<Self, WasmValueError> {
        if let AnalysedType::Record(TypeRecord { fields: types }) = ty {
            let record = TypedRecord {
                value: fields
                    .into_iter()
                    .map(|(name, value)| NameValuePair {
                        name: name.to_string(),
                        value: Some(RootTypeAnnotatedValue {
                            type_annotated_value: Some(value.0),
                        }),
                    })
                    .collect(),
                typ: types
                    .iter()
                    .map(|pair| protobuf::NameTypePair {
                        name: pair.name.clone(),
                        typ: Some((&pair.typ).into()),
                    })
                    .collect(),
            };
            Ok(TypeAnnotatedValuePrintable(TypeAnnotatedValue::Record(
                record,
            )))
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }

    fn make_tuple(
        ty: &Self::Type,
        vals: impl IntoIterator<Item = Self>,
    ) -> Result<Self, WasmValueError> {
        if let AnalysedType::Tuple(TypeTuple { items: types }) = ty {
            let tuple = TypedTuple {
                value: vals
                    .into_iter()
                    .map(|v| RootTypeAnnotatedValue {
                        type_annotated_value: Some(v.0),
                    })
                    .collect(),
                typ: types.iter().map(|t| t.into()).collect(),
            };
            Ok(TypeAnnotatedValuePrintable(TypeAnnotatedValue::Tuple(
                tuple,
            )))
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }

    fn make_variant(
        ty: &Self::Type,
        case: &str,
        val: Option<Self>,
    ) -> Result<Self, WasmValueError> {
        if let AnalysedType::Variant(TypeVariant { cases }) = ty {
            let case_type = cases.iter().find_map(|pair| {
                if pair.name == case {
                    Some(&pair.typ)
                } else {
                    None
                }
            });
            if case_type.is_some() {
                let variant = TypedVariant {
                    typ: Some(protobuf::TypeVariant {
                        cases: cases
                            .iter()
                            .map(|pair| protobuf::NameOptionTypePair {
                                name: pair.name.clone(),
                                typ: pair.typ.as_ref().map(|v| v.into()),
                            })
                            .collect(),
                    }),
                    case_name: case.to_string(),
                    case_value: val.map(|v| {
                        Box::new(RootTypeAnnotatedValue {
                            type_annotated_value: Some(v.0),
                        })
                    }),
                };
                Ok(TypeAnnotatedValuePrintable(TypeAnnotatedValue::Variant(
                    Box::new(variant),
                )))
            } else {
                Err(WasmValueError::UnknownCase(case.to_string()))
            }
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }

    fn make_enum(ty: &Self::Type, case: &str) -> Result<Self, WasmValueError> {
        if let AnalysedType::Enum(TypeEnum { cases }) = ty {
            if cases.contains(&case.to_string()) {
                let enum_value = TypedEnum {
                    typ: cases.to_vec(),
                    value: case.to_string(),
                };
                Ok(TypeAnnotatedValuePrintable(TypeAnnotatedValue::Enum(
                    enum_value,
                )))
            } else {
                Err(WasmValueError::UnknownCase(case.to_string()))
            }
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }

    fn make_option(ty: &Self::Type, val: Option<Self>) -> Result<Self, WasmValueError> {
        let option = TypedOption {
            typ: Some(ty.into()),
            value: val.map(|v| {
                Box::new(RootTypeAnnotatedValue {
                    type_annotated_value: Some(v.0),
                })
            }),
        };

        Ok(TypeAnnotatedValuePrintable(TypeAnnotatedValue::Option(
            Box::new(option),
        )))
    }

    fn make_result(
        ty: &Self::Type,
        val: Result<Option<Self>, Option<Self>>,
    ) -> Result<Self, WasmValueError> {
        if let AnalysedType::Result(golem_wasm_ast::analysis::TypeResult { ok, err }) = ty {
            let result0 = TypedResult {
                ok: ok.clone().map(|v| v.deref().into()),
                error: err.clone().map(|v| v.deref().into()),
                result_value: match val {
                    Ok(Some(v)) => Some(ResultValue::OkValue(Box::new(RootTypeAnnotatedValue {
                        type_annotated_value: Some(v.0),
                    }))),
                    Ok(None) => None,
                    Err(Some(v)) => {
                        Some(ResultValue::ErrorValue(Box::new(RootTypeAnnotatedValue {
                            type_annotated_value: Some(v.0),
                        })))
                    }
                    Err(None) => None,
                },
            };
            Ok(TypeAnnotatedValuePrintable(TypeAnnotatedValue::Result(
                Box::new(result0),
            )))
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }

    fn make_flags<'a>(
        ty: &Self::Type,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, WasmValueError> {
        if let AnalysedType::Flags(TypeFlags { names: all_names }) = ty {
            let names: Vec<String> = names.into_iter().map(|name| name.to_string()).collect();

            let invalid_names: Vec<String> = names
                .iter()
                .filter(|&name| !all_names.contains(&name.to_string()))
                .cloned()
                .collect();

            if invalid_names.is_empty() {
                let flags = TypedFlags {
                    typ: all_names.to_vec(),
                    values: names.to_vec(),
                };

                Ok(TypeAnnotatedValuePrintable(TypeAnnotatedValue::Flags(
                    flags,
                )))
            } else {
                Err(WasmValueError::UnknownCase(invalid_names.join(", ")))
            }
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }

    fn unwrap_bool(&self) -> bool {
        match self.0 {
            TypeAnnotatedValue::Bool(value) => value,
            _ => panic!("Expected bool, found {:?}", self),
        }
    }

    fn unwrap_s8(&self) -> i8 {
        match self.0 {
            TypeAnnotatedValue::S8(value) => value as i8,
            _ => panic!("Expected s8, found {:?}", self),
        }
    }

    fn unwrap_s16(&self) -> i16 {
        match self.0 {
            TypeAnnotatedValue::S16(value) => value as i16,
            _ => panic!("Expected s16, found {:?}", self),
        }
    }

    fn unwrap_s32(&self) -> i32 {
        match self.0 {
            TypeAnnotatedValue::S32(value) => value,
            _ => panic!("Expected s32, found {:?}", self),
        }
    }

    fn unwrap_s64(&self) -> i64 {
        match self.0 {
            TypeAnnotatedValue::S64(value) => value,
            _ => panic!("Expected s64, found {:?}", self),
        }
    }

    fn unwrap_u8(&self) -> u8 {
        match self.0 {
            TypeAnnotatedValue::U8(value) => value as u8,
            _ => panic!("Expected u8, found {:?}", self),
        }
    }

    fn unwrap_u16(&self) -> u16 {
        match self.0 {
            TypeAnnotatedValue::U16(value) => value as u16,
            _ => panic!("Expected u16, found {:?}", self),
        }
    }

    fn unwrap_u32(&self) -> u32 {
        match self.0 {
            TypeAnnotatedValue::U32(value) => value,
            _ => panic!("Expected u32, found {:?}", self),
        }
    }

    fn unwrap_u64(&self) -> u64 {
        match self.0 {
            TypeAnnotatedValue::U64(value) => value,
            _ => panic!("Expected u64, found {:?}", self),
        }
    }

    fn unwrap_float32(&self) -> f32 {
        match self.0 {
            TypeAnnotatedValue::F32(value) => value,
            _ => panic!("Expected f32, found {:?}", self),
        }
    }

    fn unwrap_float64(&self) -> f64 {
        match self.0 {
            TypeAnnotatedValue::F64(value) => value,
            _ => panic!("Expected f64, found {:?}", self),
        }
    }

    fn unwrap_char(&self) -> char {
        match self.0 {
            TypeAnnotatedValue::Char(value) => char::from_u32(value as u32).unwrap(),
            _ => panic!("Expected chr, found {:?}", self),
        }
    }

    fn unwrap_string(&self) -> Cow<str> {
        match self.0.clone() {
            TypeAnnotatedValue::Str(value) => Cow::Owned(value.clone()),
            _ => panic!("Expected string, found {:?}", self),
        }
    }

    fn unwrap_list(&self) -> Box<dyn Iterator<Item = Cow<Self>> + '_> {
        match self.0.clone() {
            TypeAnnotatedValue::List(TypedList { typ: _, values }) => {
                Box::new(values.into_iter().map(|v| {
                    Cow::Owned(TypeAnnotatedValuePrintable(
                        v.type_annotated_value.as_ref().unwrap().clone(),
                    ))
                }))
            }
            _ => panic!("Expected list, found {:?}", self),
        }
    }

    fn unwrap_record(&self) -> Box<dyn Iterator<Item = (Cow<str>, Cow<Self>)> + '_> {
        match self.0.clone() {
            TypeAnnotatedValue::Record(TypedRecord { typ: _, value }) => {
                Box::new(value.into_iter().map(|name_value| {
                    let name = name_value.name.clone();
                    let type_annotated_value =
                        name_value.value.unwrap().type_annotated_value.unwrap();
                    (
                        Cow::Owned(name),
                        Cow::Owned(TypeAnnotatedValuePrintable(type_annotated_value)),
                    )
                }))
            }
            _ => panic!("Expected record, found {:?}", self),
        }
    }

    fn unwrap_tuple(&self) -> Box<dyn Iterator<Item = Cow<Self>> + '_> {
        match self.0.clone() {
            TypeAnnotatedValue::Tuple(TypedTuple { typ: _, value }) => {
                Box::new(value.into_iter().map(|x| {
                    if let Some(ref v) = x.type_annotated_value {
                        Cow::Owned(TypeAnnotatedValuePrintable(v.clone()))
                    } else {
                        panic!("Expected value, found None")
                    }
                }))
            }
            _ => panic!("Expected tuple, found {:?}", self),
        }
    }

    fn unwrap_variant(&self) -> (Cow<str>, Option<Cow<Self>>) {
        match self.0.clone() {
            TypeAnnotatedValue::Variant(variant) => {
                let case_name = Cow::Owned(variant.case_name);
                let case_value = variant.case_value.clone().map(|v| {
                    Cow::Owned(TypeAnnotatedValuePrintable(v.type_annotated_value.unwrap()))
                });
                (case_name, case_value)
            }
            _ => panic!("Expected variant, found {:?}", self),
        }
    }

    fn unwrap_enum(&self) -> Cow<str> {
        match self.0.clone() {
            TypeAnnotatedValue::Enum(TypedEnum { typ: _, value }) => Cow::Owned(value),
            _ => panic!("Expected enum, found {:?}", self),
        }
    }

    fn unwrap_option(&self) -> Option<Cow<Self>> {
        match self.0.clone() {
            TypeAnnotatedValue::Option(option) => option.value.as_ref().and_then(|v| {
                v.type_annotated_value
                    .as_ref()
                    .map(|inner| Cow::Owned(TypeAnnotatedValuePrintable(inner.clone())))
            }),
            _ => panic!("Expected option, found {:?}", self),
        }
    }

    fn unwrap_result(&self) -> Result<Option<Cow<Self>>, Option<Cow<Self>>> {
        match self.0.clone() {
            TypeAnnotatedValue::Result(result0) => match result0.result_value {
                Some(result) => match result {
                    ResultValue::OkValue(ok) => match ok.type_annotated_value {
                        Some(ok_value) => {
                            Ok(Some(Cow::Owned(TypeAnnotatedValuePrintable(ok_value))))
                        }
                        None => panic!("Expected ok, found None"),
                    },
                    ResultValue::ErrorValue(error) => match error.type_annotated_value {
                        Some(error_value) => {
                            Err(Some(Cow::Owned(TypeAnnotatedValuePrintable(error_value))))
                        }
                        None => panic!("Expected error, found None"),
                    },
                },
                None => panic!("Expected ok, found None"),
            },
            _ => panic!("Expected result, found {:?}", self),
        }
    }

    fn unwrap_flags(&self) -> Box<dyn Iterator<Item = Cow<str>> + '_> {
        match self.0.clone() {
            TypeAnnotatedValue::Flags(TypedFlags { typ: _, values }) => {
                Box::new(values.into_iter().map(Cow::Owned))
            }
            _ => panic!("Expected flags, found {:?}", self),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
    use crate::text::type_annotated_value_from_str;
    use crate::{type_annotated_value_to_string, TypeAnnotatedValueConstructors, Value};
    use golem_wasm_ast::analysis::{
        AnalysedType, NameOptionTypePair, NameTypePair, TypeBool, TypeChr, TypeEnum, TypeF32,
        TypeF64, TypeFlags, TypeOption, TypeRecord, TypeResult, TypeS16, TypeS32, TypeS64, TypeS8,
        TypeStr, TypeTuple, TypeU16, TypeU32, TypeU64, TypeU8, TypeVariant,
    };

    fn round_trip(value: Value, typ: AnalysedType) {
        let typed_value = TypeAnnotatedValue::create(&value, &typ).unwrap();
        println!("{:?}", typed_value.clone());

        let s = type_annotated_value_to_string(&typed_value).unwrap();
        let round_trip_value: TypeAnnotatedValue =
            type_annotated_value_from_str(&AnalysedType::try_from(&typed_value).unwrap(), &s)
                .unwrap();
        let result: Value = Value::try_from(round_trip_value).unwrap();
        assert_eq!(value, result);
    }

    #[test]
    fn round_trip_u8() {
        round_trip(Value::U8(42), AnalysedType::U8(TypeU8));
    }

    #[test]
    fn round_trip_u16() {
        round_trip(Value::U16(1234), AnalysedType::U16(TypeU16));
    }

    #[test]
    fn round_trip_u32() {
        round_trip(Value::U32(123456), AnalysedType::U32(TypeU32));
    }

    #[test]
    fn round_trip_u64() {
        round_trip(Value::U64(1234567890123456), AnalysedType::U64(TypeU64));
    }

    #[test]
    fn round_trip_s8() {
        round_trip(Value::S8(-42), AnalysedType::S8(TypeS8));
    }

    #[test]
    fn round_trip_s16() {
        round_trip(Value::S16(-1234), AnalysedType::S16(TypeS16));
    }

    #[test]
    fn round_trip_s32() {
        round_trip(Value::S32(-123456), AnalysedType::S32(TypeS32));
    }

    #[test]
    fn round_trip_s64() {
        round_trip(Value::S64(-1234567890123456), AnalysedType::S64(TypeS64));
    }

    #[test]
    fn round_trip_f32() {
        round_trip(Value::F32(1234.5678), AnalysedType::F32(TypeF32));
    }

    #[test]
    fn round_trip_f64() {
        round_trip(
            Value::F64(1_234_567_890_123_456.8),
            AnalysedType::F64(TypeF64),
        );
    }

    #[test]
    fn round_trip_bool() {
        round_trip(Value::Bool(true), AnalysedType::Bool(TypeBool));
    }

    #[test]
    fn round_trip_char() {
        round_trip(Value::Char('a'), AnalysedType::Chr(TypeChr));
    }

    #[test]
    fn round_trip_string() {
        round_trip(
            Value::String("hello".to_string()),
            AnalysedType::Str(TypeStr),
        );
    }

    #[test]
    fn round_trip_list_1() {
        round_trip(
            Value::List(vec![Value::U8(1), Value::U8(2), Value::U8(3)]),
            AnalysedType::List(golem_wasm_ast::analysis::TypeList {
                inner: Box::new(AnalysedType::U8(TypeU8)),
            }),
        );
    }

    #[test]
    fn round_trip_list_2() {
        round_trip(
            Value::List(vec![Value::List(vec![
                Value::String("hello".to_string()),
                Value::String("world".to_string()),
            ])]),
            AnalysedType::List(golem_wasm_ast::analysis::TypeList {
                inner: Box::new(AnalysedType::List(golem_wasm_ast::analysis::TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                })),
            }),
        );
    }

    #[test]
    fn round_trip_record() {
        round_trip(
            Value::Record(vec![
                Value::U8(1),
                Value::String("hello".to_string()),
                Value::Bool(true),
            ]),
            AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "a".to_string(),
                        typ: AnalysedType::U8(TypeU8),
                    },
                    NameTypePair {
                        name: "b".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                    NameTypePair {
                        name: "c".to_string(),
                        typ: AnalysedType::Bool(TypeBool),
                    },
                ],
            }),
        );
    }

    #[test]
    fn round_trip_tuple() {
        round_trip(
            Value::Tuple(vec![
                Value::U8(1),
                Value::String("hello".to_string()),
                Value::Bool(true),
            ]),
            AnalysedType::Tuple(TypeTuple {
                items: vec![
                    AnalysedType::U8(TypeU8),
                    AnalysedType::Str(TypeStr),
                    AnalysedType::Bool(TypeBool),
                ],
            }),
        );
    }

    #[test]
    fn round_trip_variant() {
        round_trip(
            Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(Value::String("hello".to_string()))),
            },
            AnalysedType::Variant(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "A".to_string(),
                        typ: None,
                    },
                    NameOptionTypePair {
                        name: "B".to_string(),
                        typ: Some(AnalysedType::Str(TypeStr)),
                    },
                ],
            }),
        );
    }

    #[test]
    fn round_trip_enum() {
        round_trip(
            Value::Enum(1),
            AnalysedType::Enum(TypeEnum {
                cases: vec!["A".to_string(), "B".to_string()],
            }),
        );
    }

    #[test]
    fn round_trip_option() {
        round_trip(
            Value::Option(Some(Box::new(Value::U8(1)))),
            AnalysedType::Option(TypeOption {
                inner: Box::new(AnalysedType::U8(TypeU8)),
            }),
        );
    }

    #[test]
    fn round_trip_result_ok() {
        round_trip(
            Value::Result(Ok(Some(Box::new(Value::U8(1))))),
            AnalysedType::Result(TypeResult {
                ok: Some(Box::new(AnalysedType::U8(TypeU8))),
                err: None,
            }),
        );
    }

    #[test]
    fn round_trip_result_err() {
        round_trip(
            Value::Result(Err(Some(Box::new(Value::U8(1))))),
            AnalysedType::Result(TypeResult {
                err: Some(Box::new(AnalysedType::U8(TypeU8))),
                ok: None,
            }),
        );
    }

    #[test]
    fn round_trip_flags() {
        round_trip(
            Value::Flags(vec![true, false, true]),
            AnalysedType::Flags(TypeFlags {
                names: vec!["A".to_string(), "B".to_string(), "C".to_string()],
            }),
        );
    }
}
