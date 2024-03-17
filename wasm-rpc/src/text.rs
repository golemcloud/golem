use crate::Value;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue, WasmValueError};

#[derive(Clone, PartialEq, Eq)]
pub struct AnalysedType(golem_wasm_ast::analysis::AnalysedType);

impl Debug for AnalysedType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A value with known type
#[derive(Debug, Clone, PartialEq)]
pub struct TypedValue {
    value: Value,
    typ: AnalysedType,
}

impl TypedValue {
    #[allow(dead_code)]
    pub fn new(value: Value, typ: AnalysedType) -> Self {
        Self { value, typ }
    }

    pub fn get_type(&self) -> &golem_wasm_ast::analysis::AnalysedType {
        &self.typ.0
    }
}

impl WasmValue for TypedValue {
    type Type = AnalysedType;

    fn ty(&self) -> Self::Type {
        self.typ.clone()
    }

    fn make_bool(val: bool) -> Self {
        TypedValue {
            value: Value::Bool(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::Bool),
        }
    }

    fn make_s8(val: i8) -> Self {
        TypedValue {
            value: Value::S8(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::S8),
        }
    }

    fn make_s16(val: i16) -> Self {
        TypedValue {
            value: Value::S16(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::S16),
        }
    }

    fn make_s32(val: i32) -> Self {
        TypedValue {
            value: Value::S32(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::S32),
        }
    }

    fn make_s64(val: i64) -> Self {
        TypedValue {
            value: Value::S64(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::S64),
        }
    }

    fn make_u8(val: u8) -> Self {
        TypedValue {
            value: Value::U8(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::U8),
        }
    }

    fn make_u16(val: u16) -> Self {
        TypedValue {
            value: Value::U16(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::U16),
        }
    }

    fn make_u32(val: u32) -> Self {
        TypedValue {
            value: Value::U32(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::U32),
        }
    }

    fn make_u64(val: u64) -> Self {
        TypedValue {
            value: Value::U64(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::U64),
        }
    }

    fn make_float32(val: f32) -> Self {
        TypedValue {
            value: Value::F32(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::F32),
        }
    }

    fn make_float64(val: f64) -> Self {
        TypedValue {
            value: Value::F64(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::F64),
        }
    }

    fn make_char(val: char) -> Self {
        TypedValue {
            value: Value::Char(val),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::Chr),
        }
    }

    fn make_string(val: Cow<str>) -> Self {
        TypedValue {
            value: Value::String(val.to_string()),
            typ: AnalysedType(golem_wasm_ast::analysis::AnalysedType::Str),
        }
    }

    fn make_list(
        ty: &Self::Type,
        vals: impl IntoIterator<Item = Self>,
    ) -> Result<Self, WasmValueError> {
        Ok(TypedValue {
            value: Value::List(vals.into_iter().map(|v| v.value).collect()),
            typ: ty.clone(),
        })
    }

    fn make_record<'a>(
        ty: &Self::Type,
        fields: impl IntoIterator<Item = (&'a str, Self)>,
    ) -> Result<Self, WasmValueError> {
        Ok(TypedValue {
            value: Value::Record(fields.into_iter().map(|(_, v)| v.value).collect()),
            typ: ty.clone(),
        })
    }

    fn make_tuple(
        ty: &Self::Type,
        vals: impl IntoIterator<Item = Self>,
    ) -> Result<Self, WasmValueError> {
        Ok(TypedValue {
            value: Value::Tuple(vals.into_iter().map(|v| v.value).collect()),
            typ: ty.clone(),
        })
    }

    fn make_variant(
        ty: &Self::Type,
        case: &str,
        val: Option<Self>,
    ) -> Result<Self, WasmValueError> {
        if let golem_wasm_ast::analysis::AnalysedType::Variant(cases) = &ty.0 {
            let case_idx =
                cases
                    .iter()
                    .enumerate()
                    .find_map(|(idx, (name, _))| if name == case { Some(idx) } else { None });
            if let Some(case_idx) = case_idx {
                Ok(TypedValue {
                    value: Value::Variant {
                        case_idx: case_idx as u32,
                        case_value: val.map(|v| Box::new(v.value)),
                    },
                    typ: ty.clone(),
                })
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
        if let golem_wasm_ast::analysis::AnalysedType::Enum(cases) = &ty.0 {
            let case_idx =
                cases
                    .iter()
                    .enumerate()
                    .find_map(|(idx, name)| if name == case { Some(idx) } else { None });
            if let Some(case_idx) = case_idx {
                Ok(TypedValue {
                    value: Value::Enum(case_idx as u32),
                    typ: ty.clone(),
                })
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
        Ok(TypedValue {
            value: Value::Option(val.map(|v| Box::new(v.value))),
            typ: ty.clone(),
        })
    }

    fn make_result(
        ty: &Self::Type,
        val: Result<Option<Self>, Option<Self>>,
    ) -> Result<Self, WasmValueError> {
        Ok(TypedValue {
            value: Value::Result(match val {
                Ok(Some(v)) => Ok(Some(Box::new(v.value))),
                Ok(None) => Ok(None),
                Err(Some(v)) => Err(Some(Box::new(v.value))),
                Err(None) => Err(None),
            }),
            typ: ty.clone(),
        })
    }

    fn make_flags<'a>(
        ty: &Self::Type,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, WasmValueError> {
        if let golem_wasm_ast::analysis::AnalysedType::Flags(all_names) = &ty.0 {
            let mut flags = vec![false; all_names.len()];
            let names: HashSet<String> =
                HashSet::from_iter(names.into_iter().map(|s| s.to_string()));

            for (idx, name) in all_names.iter().enumerate() {
                if names.contains(name) {
                    flags[idx] = true;
                }
            }

            Ok(TypedValue {
                value: Value::Flags(flags),
                typ: ty.clone(),
            })
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: ty.kind(),
                ty: format!("{ty:?}"),
            })
        }
    }

    fn unwrap_bool(&self) -> bool {
        if let Value::Bool(val) = &self.value {
            *val
        } else {
            panic!("expected bool, got {:?}", self.value);
        }
    }

    fn unwrap_s8(&self) -> i8 {
        if let Value::S8(val) = &self.value {
            *val
        } else {
            panic!("expected s8, got {:?}", self.value);
        }
    }

    fn unwrap_s16(&self) -> i16 {
        if let Value::S16(val) = &self.value {
            *val
        } else {
            panic!("expected s16, got {:?}", self.value);
        }
    }

    fn unwrap_s32(&self) -> i32 {
        if let Value::S32(val) = &self.value {
            *val
        } else {
            panic!("expected s32, got {:?}", self.value);
        }
    }

    fn unwrap_s64(&self) -> i64 {
        if let Value::S64(val) = &self.value {
            *val
        } else {
            panic!("expected s64, got {:?}", self.value);
        }
    }

    fn unwrap_u8(&self) -> u8 {
        if let Value::U8(val) = &self.value {
            *val
        } else {
            panic!("expected u8, got {:?}", self.value);
        }
    }

    fn unwrap_u16(&self) -> u16 {
        if let Value::U16(val) = &self.value {
            *val
        } else {
            panic!("expected u16, got {:?}", self.value);
        }
    }

    fn unwrap_u32(&self) -> u32 {
        if let Value::U32(val) = &self.value {
            *val
        } else {
            panic!("expected u32, got {:?}", self.value);
        }
    }

    fn unwrap_u64(&self) -> u64 {
        if let Value::U64(val) = &self.value {
            *val
        } else {
            panic!("expected u64, got {:?}", self.value);
        }
    }

    fn unwrap_float32(&self) -> f32 {
        if let Value::F32(val) = &self.value {
            *val
        } else {
            panic!("expected f32, got {:?}", self.value);
        }
    }

    fn unwrap_float64(&self) -> f64 {
        if let Value::F64(val) = &self.value {
            *val
        } else {
            panic!("expected f64, got {:?}", self.value);
        }
    }

    fn unwrap_char(&self) -> char {
        if let Value::Char(val) = &self.value {
            *val
        } else {
            panic!("expected char, got {:?}", self.value);
        }
    }

    fn unwrap_string(&self) -> Cow<str> {
        if let Value::String(val) = &self.value {
            Cow::Borrowed(val)
        } else {
            panic!("expected string, got {:?}", self.value);
        }
    }

    fn unwrap_list(&self) -> Box<dyn Iterator<Item = Cow<Self>> + '_> {
        if let golem_wasm_ast::analysis::AnalysedType::List(inner_type) = self.get_type() {
            if let Value::List(vals) = &self.value {
                Box::new(vals.iter().map(|v| {
                    Cow::Owned(TypedValue {
                        value: v.clone(),
                        typ: AnalysedType(*inner_type.clone()),
                    })
                }))
            } else {
                panic!("expected list, got {:?}", self.value);
            }
        } else {
            panic!("expected list, got {:?}", self.typ);
        }
    }

    fn unwrap_record(&self) -> Box<dyn Iterator<Item = (Cow<str>, Cow<Self>)> + '_> {
        if let golem_wasm_ast::analysis::AnalysedType::Record(field_types) = self.get_type() {
            if let Value::Record(fields) = &self.value {
                Box::new(fields.iter().zip(field_types).map(|(v, (n, t))| {
                    (
                        Cow::Borrowed(n.as_str()),
                        Cow::Owned(TypedValue {
                            value: v.clone(),
                            typ: AnalysedType(t.clone()),
                        }),
                    )
                }))
            } else {
                panic!("expected record, got {:?}", self.typ);
            }
        } else {
            panic!("expected record, got {:?}", self.value);
        }
    }

    fn unwrap_tuple(&self) -> Box<dyn Iterator<Item = Cow<Self>> + '_> {
        if let golem_wasm_ast::analysis::AnalysedType::Tuple(val_types) = self.get_type() {
            if let Value::Tuple(vals) = &self.value {
                Box::new(vals.iter().zip(val_types).map(|(v, t)| {
                    Cow::Owned(TypedValue {
                        value: v.clone(),
                        typ: AnalysedType(t.clone()),
                    })
                }))
            } else {
                panic!("expected tuple, got {:?}", self.value);
            }
        } else {
            panic!("expected tuple, got {:?}", self.typ);
        }
    }

    fn unwrap_variant(&self) -> (Cow<str>, Option<Cow<Self>>) {
        if let golem_wasm_ast::analysis::AnalysedType::Variant(cases) = self.get_type() {
            if let Value::Variant {
                case_idx,
                case_value,
            } = &self.value
            {
                let (name, typ) = &cases[*case_idx as usize];
                match typ {
                    None => return (Cow::Borrowed(name), None),
                    Some(typ) => (
                        Cow::Borrowed(name),
                        case_value.as_ref().map(|v| {
                            Cow::Owned(TypedValue {
                                value: *v.clone(),
                                typ: AnalysedType(typ.clone()),
                            })
                        }),
                    ),
                }
            } else {
                panic!("expected variant, got {:?}", self.value);
            }
        } else {
            panic!("expected variant, got {:?}", self.typ);
        }
    }

    fn unwrap_enum(&self) -> Cow<str> {
        if let golem_wasm_ast::analysis::AnalysedType::Enum(cases) = self.get_type() {
            if let Value::Enum(case_idx) = &self.value {
                Cow::Borrowed(&cases[*case_idx as usize])
            } else {
                panic!("expected enum, got {:?}", self.value);
            }
        } else {
            panic!("expected enum, got {:?}", self.typ);
        }
    }

    fn unwrap_option(&self) -> Option<Cow<Self>> {
        if let golem_wasm_ast::analysis::AnalysedType::Option(inner_type) = self.get_type() {
            if let Value::Option(val) = &self.value {
                val.as_ref().map(|v| {
                    Cow::Owned(TypedValue {
                        value: *v.clone(),
                        typ: AnalysedType(*inner_type.clone()),
                    })
                })
            } else {
                panic!("expected option, got {:?}", self.value);
            }
        } else {
            panic!("expected option, got {:?}", self.typ);
        }
    }

    fn unwrap_result(&self) -> Result<Option<Cow<Self>>, Option<Cow<Self>>> {
        if let golem_wasm_ast::analysis::AnalysedType::Result { ok, error } = self.get_type() {
            if let Value::Result(val) = &self.value {
                match val {
                    Ok(Some(v)) => Ok(Some(Cow::Owned(TypedValue {
                        value: *v.clone(),
                        typ: AnalysedType(*(ok.as_ref().unwrap()).clone()),
                    }))),
                    Ok(None) => Ok(None),
                    Err(Some(v)) => Err(Some(Cow::Owned(TypedValue {
                        value: *v.clone(),
                        typ: AnalysedType(*(error.as_ref().unwrap()).clone()),
                    }))),
                    Err(None) => Err(None),
                }
            } else {
                panic!("expected result, got {:?}", self.value);
            }
        } else {
            panic!("expected result, got {:?}", self.typ);
        }
    }

    fn unwrap_flags(&self) -> Box<dyn Iterator<Item = Cow<str>> + '_> {
        if let golem_wasm_ast::analysis::AnalysedType::Flags(names) = self.get_type() {
            if let Value::Flags(flags) = &self.value {
                Box::new(flags.iter().zip(names).filter_map(|(flag, name)| {
                    if *flag {
                        Some(Cow::Borrowed(name.as_str()))
                    } else {
                        None
                    }
                }))
            } else {
                panic!("expected flags, got {:?}", self.value);
            }
        } else {
            panic!("expected flags, got {:?}", self.typ);
        }
    }
}

impl WasmType for AnalysedType {
    fn kind(&self) -> WasmTypeKind {
        match &self.0 {
            golem_wasm_ast::analysis::AnalysedType::Bool => WasmTypeKind::Bool,
            golem_wasm_ast::analysis::AnalysedType::S8 => WasmTypeKind::S8,
            golem_wasm_ast::analysis::AnalysedType::U8 => WasmTypeKind::U8,
            golem_wasm_ast::analysis::AnalysedType::S16 => WasmTypeKind::S16,
            golem_wasm_ast::analysis::AnalysedType::U16 => WasmTypeKind::U16,
            golem_wasm_ast::analysis::AnalysedType::S32 => WasmTypeKind::S32,
            golem_wasm_ast::analysis::AnalysedType::U32 => WasmTypeKind::U32,
            golem_wasm_ast::analysis::AnalysedType::S64 => WasmTypeKind::S64,
            golem_wasm_ast::analysis::AnalysedType::U64 => WasmTypeKind::U64,
            golem_wasm_ast::analysis::AnalysedType::F32 => WasmTypeKind::Float32,
            golem_wasm_ast::analysis::AnalysedType::F64 => WasmTypeKind::Float64,
            golem_wasm_ast::analysis::AnalysedType::Chr => WasmTypeKind::Char,
            golem_wasm_ast::analysis::AnalysedType::Str => WasmTypeKind::String,
            golem_wasm_ast::analysis::AnalysedType::List(_) => WasmTypeKind::List,
            golem_wasm_ast::analysis::AnalysedType::Tuple(_) => WasmTypeKind::Tuple,
            golem_wasm_ast::analysis::AnalysedType::Record(_) => WasmTypeKind::Record,
            golem_wasm_ast::analysis::AnalysedType::Flags(_) => WasmTypeKind::Flags,
            golem_wasm_ast::analysis::AnalysedType::Enum(_) => WasmTypeKind::Enum,
            golem_wasm_ast::analysis::AnalysedType::Option(_) => WasmTypeKind::Option,
            golem_wasm_ast::analysis::AnalysedType::Result { .. } => WasmTypeKind::Result,
            golem_wasm_ast::analysis::AnalysedType::Variant(_) => WasmTypeKind::Variant,
            golem_wasm_ast::analysis::AnalysedType::Resource { .. } => WasmTypeKind::Unsupported,
        }
    }

    fn list_element_type(&self) -> Option<Self> {
        if let golem_wasm_ast::analysis::AnalysedType::List(ty) = &self.0 {
            Some(AnalysedType(*ty.clone()))
        } else {
            None
        }
    }

    fn record_fields(&self) -> Box<dyn Iterator<Item = (Cow<str>, Self)> + '_> {
        if let golem_wasm_ast::analysis::AnalysedType::Record(fields) = &self.0 {
            Box::new(
                fields
                    .iter()
                    .map(|(name, ty)| (Cow::Borrowed(name.as_str()), AnalysedType(ty.clone()))),
            )
        } else {
            Box::new(std::iter::empty())
        }
    }

    fn tuple_element_types(&self) -> Box<dyn Iterator<Item = Self> + '_> {
        if let golem_wasm_ast::analysis::AnalysedType::Tuple(types) = &self.0 {
            Box::new(types.iter().map(|t| AnalysedType(t.clone())))
        } else {
            Box::new(std::iter::empty())
        }
    }

    fn variant_cases(&self) -> Box<dyn Iterator<Item = (Cow<str>, Option<Self>)> + '_> {
        if let golem_wasm_ast::analysis::AnalysedType::Variant(cases) = &self.0 {
            Box::new(cases.iter().map(|(name, ty)| {
                (
                    Cow::Borrowed(name.as_str()),
                    ty.as_ref().map(|t| AnalysedType(t.clone())),
                )
            }))
        } else {
            Box::new(std::iter::empty())
        }
    }

    fn enum_cases(&self) -> Box<dyn Iterator<Item = Cow<str>> + '_> {
        if let golem_wasm_ast::analysis::AnalysedType::Enum(cases) = &self.0 {
            Box::new(cases.iter().map(|name| Cow::Borrowed(name.as_str())))
        } else {
            Box::new(std::iter::empty())
        }
    }

    fn option_some_type(&self) -> Option<Self> {
        if let golem_wasm_ast::analysis::AnalysedType::Option(ty) = &self.0 {
            Some(AnalysedType(*ty.clone()))
        } else {
            None
        }
    }

    fn result_types(&self) -> Option<(Option<Self>, Option<Self>)> {
        if let golem_wasm_ast::analysis::AnalysedType::Result { ok, error } = &self.0 {
            Some((
                ok.as_ref().map(|t| AnalysedType(*t.clone())),
                error.as_ref().map(|t| AnalysedType(*t.clone())),
            ))
        } else {
            None
        }
    }

    fn flags_names(&self) -> Box<dyn Iterator<Item = Cow<str>> + '_> {
        if let golem_wasm_ast::analysis::AnalysedType::Flags(names) = &self.0 {
            Box::new(names.iter().map(|name| Cow::Borrowed(name.as_str())))
        } else {
            Box::new(std::iter::empty())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::text::TypedValue;
    use crate::Value;
    use golem_wasm_ast::analysis::AnalysedType;
    use wasm_wave::{from_str, to_string};

    fn round_trip(value: Value, typ: AnalysedType) {
        let typed_value = TypedValue::new(value.clone(), super::AnalysedType(typ));
        let s = to_string(&typed_value).unwrap();
        let round_trip_value: TypedValue = from_str(&typed_value.typ, &s).unwrap();
        assert_eq!(value, round_trip_value.value);
    }

    #[test]
    fn round_trip_u8() {
        round_trip(Value::U8(42), AnalysedType::U8);
    }

    #[test]
    fn round_trip_u16() {
        round_trip(Value::U16(1234), AnalysedType::U16);
    }

    #[test]
    fn round_trip_u32() {
        round_trip(Value::U32(123456), AnalysedType::U32);
    }

    #[test]
    fn round_trip_u64() {
        round_trip(Value::U64(1234567890123456), AnalysedType::U64);
    }

    #[test]
    fn round_trip_s8() {
        round_trip(Value::S8(-42), AnalysedType::S8);
    }

    #[test]
    fn round_trip_s16() {
        round_trip(Value::S16(-1234), AnalysedType::S16);
    }

    #[test]
    fn round_trip_s32() {
        round_trip(Value::S32(-123456), AnalysedType::S32);
    }

    #[test]
    fn round_trip_s64() {
        round_trip(Value::S64(-1234567890123456), AnalysedType::S64);
    }

    #[test]
    fn round_trip_f32() {
        round_trip(Value::F32(1234.5678), AnalysedType::F32);
    }

    #[test]
    fn round_trip_f64() {
        round_trip(Value::F64(1_234_567_890_123_456.8), AnalysedType::F64);
    }

    #[test]
    fn round_trip_bool() {
        round_trip(Value::Bool(true), AnalysedType::Bool);
    }

    #[test]
    fn round_trip_char() {
        round_trip(Value::Char('a'), AnalysedType::Chr);
    }

    #[test]
    fn round_trip_string() {
        round_trip(Value::String("hello".to_string()), AnalysedType::Str);
    }

    #[test]
    fn round_trip_list_1() {
        round_trip(
            Value::List(vec![Value::U8(1), Value::U8(2), Value::U8(3)]),
            AnalysedType::List(Box::new(AnalysedType::U8)),
        );
    }

    #[test]
    fn round_trip_list_2() {
        round_trip(
            Value::List(vec![Value::List(vec![
                Value::String("hello".to_string()),
                Value::String("world".to_string()),
            ])]),
            AnalysedType::List(Box::new(AnalysedType::List(Box::new(AnalysedType::Str)))),
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
            AnalysedType::Record(vec![
                ("a".to_string(), AnalysedType::U8),
                ("b".to_string(), AnalysedType::Str),
                ("c".to_string(), AnalysedType::Bool),
            ]),
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
            AnalysedType::Tuple(vec![
                AnalysedType::U8,
                AnalysedType::Str,
                AnalysedType::Bool,
            ]),
        );
    }

    #[test]
    fn round_trip_variant() {
        round_trip(
            Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(Value::String("hello".to_string()))),
            },
            AnalysedType::Variant(vec![
                ("A".to_string(), None),
                ("B".to_string(), Some(AnalysedType::Str)),
            ]),
        );
    }

    #[test]
    fn round_trip_enum() {
        round_trip(
            Value::Enum(1),
            AnalysedType::Enum(vec!["A".to_string(), "B".to_string()]),
        );
    }

    #[test]
    fn round_trip_option() {
        round_trip(
            Value::Option(Some(Box::new(Value::U8(1)))),
            AnalysedType::Option(Box::new(AnalysedType::U8)),
        );
    }

    #[test]
    fn round_trip_result_ok() {
        round_trip(
            Value::Result(Ok(Some(Box::new(Value::U8(1))))),
            AnalysedType::Result {
                ok: Some(Box::new(AnalysedType::U8)),
                error: None,
            },
        );
    }

    #[test]
    fn round_trip_result_err() {
        round_trip(
            Value::Result(Err(Some(Box::new(Value::U8(1))))),
            AnalysedType::Result {
                error: Some(Box::new(AnalysedType::U8)),
                ok: None,
            },
        );
    }

    #[test]
    fn round_trip_flags() {
        round_trip(
            Value::Flags(vec![true, false, true]),
            AnalysedType::Flags(vec!["A".to_string(), "B".to_string(), "C".to_string()]),
        );
    }
}
