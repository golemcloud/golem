use crate::TypeAnnotatedValue;
use std::borrow::Cow;
use std::fmt::{Debug, Formatter};
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue, WasmValueError};

#[derive(Clone, PartialEq, Eq)]
pub struct AnalysedType(golem_wasm_ast::analysis::AnalysedType);

impl Debug for AnalysedType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
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

impl WasmValue for TypeAnnotatedValue {
    type Type = AnalysedType;
    fn ty(&self) -> Self::Type {
        AnalysedType(golem_wasm_ast::analysis::AnalysedType::from(self.clone()))
    }

    fn make_bool(val: bool) -> Self {
        TypeAnnotatedValue::Bool(val)
    }

    fn make_s8(val: i8) -> Self {
        TypeAnnotatedValue::S8(val)
    }

    fn make_s16(val: i16) -> Self {
        TypeAnnotatedValue::S16(val)
    }

    fn make_s32(val: i32) -> Self {
        TypeAnnotatedValue::S32(val)
    }

    fn make_s64(val: i64) -> Self {
        TypeAnnotatedValue::S64(val)
    }

    fn make_u8(val: u8) -> Self {
        TypeAnnotatedValue::U8(val)
    }

    fn make_u16(val: u16) -> Self {
        TypeAnnotatedValue::U16(val)
    }

    fn make_u32(val: u32) -> Self {
        TypeAnnotatedValue::U32(val)
    }

    fn make_u64(val: u64) -> Self {
        TypeAnnotatedValue::U64(val)
    }

    fn make_float32(val: f32) -> Self {
        TypeAnnotatedValue::F32(val)
    }

    fn make_float64(val: f64) -> Self {
        TypeAnnotatedValue::F64(val)
    }

    fn make_char(val: char) -> Self {
        TypeAnnotatedValue::Chr(val)
    }

    fn make_string(val: Cow<str>) -> Self {
        TypeAnnotatedValue::Str(val.to_string())
    }

    fn make_list(
        ty: &Self::Type,
        vals: impl IntoIterator<Item = Self>,
    ) -> Result<Self, WasmValueError> {
        if let golem_wasm_ast::analysis::AnalysedType::List(typ) = &ty.0 {
            Ok(TypeAnnotatedValue::List {
                values: vals.into_iter().collect(),
                typ: *typ.clone(),
            })
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
        if let golem_wasm_ast::analysis::AnalysedType::Record(types) = &ty.0 {
            Ok(TypeAnnotatedValue::Record {
                value: fields
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value))
                    .collect(),
                typ: types.clone(),
            })
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
        if let golem_wasm_ast::analysis::AnalysedType::Tuple(types) = &ty.0 {
            Ok(TypeAnnotatedValue::Tuple {
                value: vals.into_iter().collect(),
                typ: types.clone(),
            })
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
        if let golem_wasm_ast::analysis::AnalysedType::Variant(cases) = &ty.0 {
            let case_type =
                cases.iter().find_map(
                    |(name, case_type)| {
                        if name == case {
                            Some(case_type)
                        } else {
                            None
                        }
                    },
                );
            if case_type.is_some() {
                Ok(TypeAnnotatedValue::Variant {
                    typ: cases.clone(),
                    case_name: case.to_string(),
                    case_value: val.map(Box::new),
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
            if cases.contains(&case.to_string()) {
                Ok(TypeAnnotatedValue::Enum {
                    typ: cases.clone(),
                    value: case.to_string(),
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
        Ok(TypeAnnotatedValue::Option {
            typ: ty.clone().0,
            value: val.map(Box::new),
        })
    }

    fn make_result(
        ty: &Self::Type,
        val: Result<Option<Self>, Option<Self>>,
    ) -> Result<Self, WasmValueError> {
        if let golem_wasm_ast::analysis::AnalysedType::Result { ok, error } = &ty.0 {
            Ok(TypeAnnotatedValue::Result {
                value: match val {
                    Ok(Some(v)) => Ok(Some(Box::new(v))),
                    Ok(None) => Ok(None),
                    Err(Some(v)) => Err(Some(Box::new(v))),
                    Err(None) => Err(None),
                },
                ok: ok.clone(),
                error: error.clone(),
            })
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
        if let golem_wasm_ast::analysis::AnalysedType::Flags(all_names) = &ty.0 {
            let names: Vec<String> = names.into_iter().map(|name| name.to_string()).collect();

            let invalid_names: Vec<String> = names
                .iter()
                .filter(|&name| !all_names.contains(&name.to_string()))
                .cloned()
                .collect();

            if invalid_names.is_empty() {
                Ok(TypeAnnotatedValue::Flags {
                    typ: all_names.clone(),
                    values: names,
                })
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
        match self {
            TypeAnnotatedValue::Bool(value) => *value,
            _ => panic!("Expected bool, found {:?}", self),
        }
    }

    fn unwrap_s8(&self) -> i8 {
        match self {
            TypeAnnotatedValue::S8(value) => *value,
            _ => panic!("Expected s8, found {:?}", self),
        }
    }

    fn unwrap_s16(&self) -> i16 {
        match self {
            TypeAnnotatedValue::S16(value) => *value,
            _ => panic!("Expected s16, found {:?}", self),
        }
    }

    fn unwrap_s32(&self) -> i32 {
        match self {
            TypeAnnotatedValue::S32(value) => *value,
            _ => panic!("Expected s32, found {:?}", self),
        }
    }

    fn unwrap_s64(&self) -> i64 {
        match self {
            TypeAnnotatedValue::S64(value) => *value,
            _ => panic!("Expected s64, found {:?}", self),
        }
    }

    fn unwrap_u8(&self) -> u8 {
        match self {
            TypeAnnotatedValue::U8(value) => *value,
            _ => panic!("Expected u8, found {:?}", self),
        }
    }

    fn unwrap_u16(&self) -> u16 {
        match self {
            TypeAnnotatedValue::U16(value) => *value,
            _ => panic!("Expected u16, found {:?}", self),
        }
    }

    fn unwrap_u32(&self) -> u32 {
        match self {
            TypeAnnotatedValue::U32(value) => *value,
            _ => panic!("Expected u32, found {:?}", self),
        }
    }

    fn unwrap_u64(&self) -> u64 {
        match self {
            TypeAnnotatedValue::U64(value) => *value,
            _ => panic!("Expected u64, found {:?}", self),
        }
    }

    fn unwrap_float32(&self) -> f32 {
        match self {
            TypeAnnotatedValue::F32(value) => *value,
            _ => panic!("Expected f32, found {:?}", self),
        }
    }

    fn unwrap_float64(&self) -> f64 {
        match self {
            TypeAnnotatedValue::F64(value) => *value,
            _ => panic!("Expected f64, found {:?}", self),
        }
    }

    fn unwrap_char(&self) -> char {
        match self {
            TypeAnnotatedValue::Chr(value) => *value,
            _ => panic!("Expected chr, found {:?}", self),
        }
    }

    fn unwrap_string(&self) -> Cow<str> {
        match self {
            TypeAnnotatedValue::Str(value) => Cow::Borrowed(value),
            _ => panic!("Expected string, found {:?}", self),
        }
    }

    fn unwrap_list(&self) -> Box<dyn Iterator<Item = Cow<Self>> + '_> {
        match self {
            TypeAnnotatedValue::List { typ: _, values } => {
                Box::new(values.iter().map(Cow::Borrowed))
            }
            _ => panic!("Expected list, found {:?}", self),
        }
    }

    fn unwrap_record(&self) -> Box<dyn Iterator<Item = (Cow<str>, Cow<Self>)> + '_> {
        match self {
            TypeAnnotatedValue::Record { typ: _, value } => Box::new(
                value
                    .iter()
                    .map(|(name, value)| (Cow::Borrowed(name.as_str()), Cow::Borrowed(value))),
            ),
            _ => panic!("Expected record, found {:?}", self),
        }
    }

    fn unwrap_tuple(&self) -> Box<dyn Iterator<Item = Cow<Self>> + '_> {
        match self {
            TypeAnnotatedValue::Tuple { typ: _, value } => {
                Box::new(value.iter().map(Cow::Borrowed))
            }
            _ => panic!("Expected tuple, found {:?}", self),
        }
    }

    fn unwrap_variant(&self) -> (Cow<str>, Option<Cow<Self>>) {
        match self {
            TypeAnnotatedValue::Variant {
                typ: _,
                case_name,
                case_value,
            } => {
                let case_name = Cow::Borrowed(case_name.as_str());
                let case_value = case_value.clone().map(|v| Cow::Owned(*v));
                (case_name, case_value)
            }
            _ => panic!("Expected variant, found {:?}", self),
        }
    }

    fn unwrap_enum(&self) -> Cow<str> {
        match self {
            TypeAnnotatedValue::Enum { typ: _, value } => Cow::Borrowed(value.as_str()),
            _ => panic!("Expected enum, found {:?}", self),
        }
    }

    fn unwrap_option(&self) -> Option<Cow<Self>> {
        match self {
            TypeAnnotatedValue::Option { typ: _, value } => {
                value.as_ref().map(|v| Cow::Owned(*v.clone()))
            }
            _ => panic!("Expected option, found {:?}", self),
        }
    }

    fn unwrap_result(&self) -> Result<Option<Cow<Self>>, Option<Cow<Self>>> {
        match self {
            TypeAnnotatedValue::Result {
                ok: _,
                error: _,
                value,
            } => match value {
                Ok(Some(v)) => Ok(Some(Cow::Borrowed(v))),
                Ok(None) => Ok(None),
                Err(Some(v)) => Err(Some(Cow::Borrowed(v))),
                Err(None) => Err(None),
            },
            _ => panic!("Expected result, found {:?}", self),
        }
    }

    fn unwrap_flags(&self) -> Box<dyn Iterator<Item = Cow<str>> + '_> {
        match self {
            TypeAnnotatedValue::Flags { typ: _, values } => {
                Box::new(values.iter().map(|v| Cow::Borrowed(v.as_str())))
            }
            _ => panic!("Expected flags, found {:?}", self),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{TypeAnnotatedValue, Value};
    use golem_wasm_ast::analysis::AnalysedType;
    use wasm_wave::{from_str, to_string};

    fn round_trip(value: Value, typ: AnalysedType) {
        let typed_value = TypeAnnotatedValue::from_value(&value, &typ).unwrap();
        println!("{:?}", typed_value.clone());

        let s = to_string(&typed_value).unwrap();
        let round_trip_value: TypeAnnotatedValue =
            from_str(&super::AnalysedType(AnalysedType::from(typed_value)), &s).unwrap();
        let result: Value = round_trip_value.try_into().unwrap();
        assert_eq!(value, result);
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
