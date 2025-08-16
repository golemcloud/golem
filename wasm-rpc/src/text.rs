// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{IntoValueAndType, Value, ValueAndType};
use golem_wasm_ast::analysis::AnalysedType;
use std::borrow::Cow;
use std::collections::HashSet;
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue, WasmValueError};
use wasm_wave::{from_str, to_string};

pub fn parse_value_and_type(
    analysed_type: &AnalysedType,
    input: &str,
) -> Result<ValueAndType, String> {
    let parsed: ValueAndType = from_str(analysed_type, input).map_err(|err| err.to_string())?;
    Ok(parsed)
}

pub fn print_value_and_type(value: &ValueAndType) -> Result<String, String> {
    if value.typ.contains_handle() {
        Err("Cannot print handle type".to_string())
    } else {
        to_string(value).map_err(|err| err.to_string())
    }
}

impl WasmValue for ValueAndType {
    type Type = AnalysedType;

    fn kind(&self) -> WasmTypeKind {
        self.typ.kind()
    }

    fn make_bool(val: bool) -> Self {
        val.into_value_and_type()
    }

    fn make_s8(val: i8) -> Self {
        val.into_value_and_type()
    }

    fn make_s16(val: i16) -> Self {
        val.into_value_and_type()
    }

    fn make_s32(val: i32) -> Self {
        val.into_value_and_type()
    }

    fn make_s64(val: i64) -> Self {
        val.into_value_and_type()
    }

    fn make_u8(val: u8) -> Self {
        val.into_value_and_type()
    }

    fn make_u16(val: u16) -> Self {
        val.into_value_and_type()
    }

    fn make_u32(val: u32) -> Self {
        val.into_value_and_type()
    }

    fn make_u64(val: u64) -> Self {
        val.into_value_and_type()
    }

    fn make_f32(val: f32) -> Self {
        val.into_value_and_type()
    }

    fn make_f64(val: f64) -> Self {
        val.into_value_and_type()
    }

    fn make_char(val: char) -> Self {
        val.into_value_and_type()
    }

    fn make_string(val: Cow<str>) -> Self {
        val.to_string().into_value_and_type()
    }

    fn make_list(
        ty: &Self::Type,
        vals: impl IntoIterator<Item = Self>,
    ) -> Result<Self, WasmValueError> {
        Ok(ValueAndType {
            value: Value::List(vals.into_iter().map(|vnt| vnt.value).collect()),
            typ: ty.clone(),
        })
    }

    fn make_record<'a>(
        ty: &Self::Type,
        fields: impl IntoIterator<Item = (&'a str, Self)>,
    ) -> Result<Self, WasmValueError> {
        Ok(ValueAndType {
            value: Value::Record(fields.into_iter().map(|(_, vnt)| vnt.value).collect()),
            typ: ty.clone(),
        })
    }

    fn make_tuple(
        ty: &Self::Type,
        vals: impl IntoIterator<Item = Self>,
    ) -> Result<Self, WasmValueError> {
        Ok(ValueAndType {
            value: Value::Tuple(vals.into_iter().map(|vnt| vnt.value).collect()),
            typ: ty.clone(),
        })
    }

    fn make_variant(
        ty: &Self::Type,
        case: &str,
        val: Option<Self>,
    ) -> Result<Self, WasmValueError> {
        if let AnalysedType::Variant(typ) = ty {
            let case_idx = typ
                .cases
                .iter()
                .position(|pair| pair.name == case)
                .ok_or_else(|| WasmValueError::UnknownCase(case.to_string()))?
                as u32;
            Ok(ValueAndType {
                value: Value::Variant {
                    case_idx,
                    case_value: val.map(|vnt| Box::new(vnt.value)),
                },
                typ: ty.clone(),
            })
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: WasmTypeKind::Variant,
                ty: ty.kind().to_string(),
            })
        }
    }

    fn make_enum(ty: &Self::Type, case: &str) -> Result<Self, WasmValueError> {
        if let AnalysedType::Enum(typ) = ty {
            let case_idx = typ
                .cases
                .iter()
                .position(|c| c == case)
                .ok_or_else(|| WasmValueError::UnknownCase(case.to_string()))?
                as u32;
            Ok(ValueAndType {
                value: Value::Enum(case_idx),
                typ: ty.clone(),
            })
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: WasmTypeKind::Enum,
                ty: ty.kind().to_string(),
            })
        }
    }

    fn make_option(ty: &Self::Type, val: Option<Self>) -> Result<Self, WasmValueError> {
        Ok(ValueAndType {
            value: Value::Option(val.map(|vnt| Box::new(vnt.value))),
            typ: ty.clone(),
        })
    }

    fn make_result(
        ty: &Self::Type,
        val: Result<Option<Self>, Option<Self>>,
    ) -> Result<Self, WasmValueError> {
        Ok(ValueAndType {
            value: Value::Result(
                val.map(|maybe_ok| maybe_ok.map(|vnt| Box::new(vnt.value)))
                    .map_err(|maybe_err| maybe_err.map(|vnt| Box::new(vnt.value))),
            ),
            typ: ty.clone(),
        })
    }

    fn make_flags<'a>(
        ty: &Self::Type,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, WasmValueError> {
        if let AnalysedType::Flags(typ) = ty {
            let mut bitmap = Vec::new();
            let names: HashSet<&'a str> = HashSet::from_iter(names);
            for name in &typ.names {
                bitmap.push(names.contains(name.as_str()));
            }
            Ok(ValueAndType {
                value: Value::Flags(bitmap),
                typ: ty.clone(),
            })
        } else {
            Err(WasmValueError::WrongTypeKind {
                kind: WasmTypeKind::Flags,
                ty: ty.kind().to_string(),
            })
        }
    }

    fn unwrap_bool(&self) -> bool {
        match self.value {
            Value::Bool(val) => val,
            _ => panic!("Expected bool, found {self:?}"),
        }
    }

    fn unwrap_s8(&self) -> i8 {
        match self.value {
            Value::S8(val) => val,
            _ => panic!("Expected s8, found {self:?}"),
        }
    }

    fn unwrap_s16(&self) -> i16 {
        match self.value {
            Value::S16(val) => val,
            _ => panic!("Expected s16, found {self:?}"),
        }
    }

    fn unwrap_s32(&self) -> i32 {
        match self.value {
            Value::S32(val) => val,
            _ => panic!("Expected s32, found {self:?}"),
        }
    }

    fn unwrap_s64(&self) -> i64 {
        match self.value {
            Value::S64(val) => val,
            _ => panic!("Expected s64, found {self:?}"),
        }
    }

    fn unwrap_u8(&self) -> u8 {
        match self.value {
            Value::U8(val) => val,
            _ => panic!("Expected u8, found {self:?}"),
        }
    }

    fn unwrap_u16(&self) -> u16 {
        match self.value {
            Value::U16(val) => val,
            _ => panic!("Expected u16, found {self:?}"),
        }
    }

    fn unwrap_u32(&self) -> u32 {
        match self.value {
            Value::U32(val) => val,
            _ => panic!("Expected u32, found {self:?}"),
        }
    }

    fn unwrap_u64(&self) -> u64 {
        match self.value {
            Value::U64(val) => val,
            _ => panic!("Expected u64, found {self:?}"),
        }
    }

    fn unwrap_f32(&self) -> f32 {
        match self.value {
            Value::F32(val) => val,
            _ => panic!("Expected f32, found {self:?}"),
        }
    }

    fn unwrap_f64(&self) -> f64 {
        match self.value {
            Value::F64(val) => val,
            _ => panic!("Expected f64, found {self:?}"),
        }
    }

    fn unwrap_char(&self) -> char {
        match self.value {
            Value::Char(val) => val,
            _ => panic!("Expected char, found {self:?}"),
        }
    }

    fn unwrap_string(&self) -> Cow<'_, str> {
        match &self.value {
            Value::String(val) => Cow::Borrowed(val),
            _ => panic!("Expected string, found {self:?}"),
        }
    }

    fn unwrap_list(&self) -> Box<dyn Iterator<Item = Cow<'_, Self>> + '_> {
        match (&self.value, &self.typ) {
            (Value::List(vals), AnalysedType::List(typ)) => Box::new(vals.iter().map(|v| {
                Cow::Owned(ValueAndType {
                    value: v.clone(),
                    typ: (*typ.inner).clone(),
                })
            })),
            _ => panic!("Expected list, found {self:?}"),
        }
    }

    fn unwrap_record(&self) -> Box<dyn Iterator<Item = (Cow<'_, str>, Cow<'_, Self>)> + '_> {
        match (&self.value, &self.typ) {
            (Value::Record(vals), AnalysedType::Record(typ)) => {
                Box::new(vals.iter().zip(typ.fields.iter()).map(|(v, f)| {
                    (
                        Cow::Borrowed(f.name.as_str()),
                        Cow::Owned(ValueAndType {
                            value: v.clone(),
                            typ: f.typ.clone(),
                        }),
                    )
                }))
            }
            _ => panic!("Expected record, found {self:?}"),
        }
    }

    fn unwrap_tuple(&self) -> Box<dyn Iterator<Item = Cow<'_, Self>> + '_> {
        match (&self.value, &self.typ) {
            (Value::Tuple(vals), AnalysedType::Tuple(typ)) => {
                Box::new(vals.iter().zip(typ.items.iter()).map(|(v, t)| {
                    Cow::Owned(ValueAndType {
                        value: v.clone(),
                        typ: t.clone(),
                    })
                }))
            }
            _ => panic!("Expected tuple, found {self:?}"),
        }
    }

    fn unwrap_variant(&self) -> (Cow<'_, str>, Option<Cow<'_, Self>>) {
        match (&self.value, &self.typ) {
            (
                Value::Variant {
                    case_idx,
                    case_value,
                },
                AnalysedType::Variant(typ),
            ) => {
                let case_name = &typ.cases[*case_idx as usize].name;
                let case_value = case_value.as_ref().map(|v| {
                    let typ = &typ.cases[*case_idx as usize].typ;
                    Cow::Owned(ValueAndType {
                        value: *v.clone(),
                        typ: typ
                            .as_ref()
                            .unwrap_or_else(|| {
                                panic!("No type information for non-unit variant case {case_name}")
                            })
                            .clone(),
                    })
                });
                (Cow::Borrowed(case_name), case_value)
            }
            _ => panic!("Expected variant, found {self:?}"),
        }
    }

    fn unwrap_enum(&self) -> Cow<'_, str> {
        match (&self.value, &self.typ) {
            (Value::Enum(case_idx), AnalysedType::Enum(typ)) => {
                Cow::Borrowed(&typ.cases[*case_idx as usize])
            }
            _ => panic!("Expected enum, found {self:?}"),
        }
    }

    fn unwrap_option(&self) -> Option<Cow<'_, Self>> {
        match (&self.value, &self.typ) {
            (Value::Option(Some(val)), AnalysedType::Option(typ)) => {
                Some(Cow::Owned(ValueAndType {
                    value: *val.clone(),
                    typ: (*typ.inner).clone(),
                }))
            }
            (Value::Option(None), AnalysedType::Option(_)) => None,
            _ => panic!("Expected option, found {self:?}"),
        }
    }

    fn unwrap_result(&self) -> Result<Option<Cow<'_, Self>>, Option<Cow<'_, Self>>> {
        match (&self.value, &self.typ) {
            (Value::Result(Ok(Some(val))), AnalysedType::Result(typ)) => {
                Ok(Some(Cow::Owned(ValueAndType {
                    value: *val.clone(),
                    typ: *typ
                        .ok
                        .as_ref()
                        .expect("No type information for non-unit ok value")
                        .clone(),
                })))
            }
            (Value::Result(Ok(None)), AnalysedType::Result(_)) => Ok(None),
            (Value::Result(Err(Some(val))), AnalysedType::Result(typ)) => {
                Err(Some(Cow::Owned(ValueAndType {
                    value: *val.clone(),
                    typ: *typ
                        .err
                        .as_ref()
                        .expect("No type information for non-unit error value")
                        .clone(),
                })))
            }
            (Value::Result(Err(None)), AnalysedType::Result(_)) => Err(None),
            _ => panic!("Expected result, found {self:?}"),
        }
    }

    fn unwrap_flags(&self) -> Box<dyn Iterator<Item = Cow<'_, str>> + '_> {
        match (&self.value, &self.typ) {
            (Value::Flags(bitmap), AnalysedType::Flags(typ)) => Box::new(
                bitmap
                    .iter()
                    .zip(typ.names.iter())
                    .filter_map(|(is_set, name)| {
                        if *is_set {
                            Some(Cow::Borrowed(name.as_str()))
                        } else {
                            None
                        }
                    }),
            ),
            _ => panic!("Expected flags, found {self:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::text::{parse_value_and_type, print_value_and_type};
    use crate::{Value, ValueAndType};
    use golem_wasm_ast::analysis::analysed_type::{
        bool, case, chr, f32, f64, field, flags, list, option, r#enum, record, result_err,
        result_ok, s16, s32, s64, s8, str, tuple, u16, u32, u64, u8, unit_case, variant,
    };
    use golem_wasm_ast::analysis::AnalysedType;

    fn round_trip(value: Value, typ: AnalysedType) {
        let typed_value = ValueAndType::new(value.clone(), typ.clone());

        let s = print_value_and_type(&typed_value).unwrap();
        let round_trip_value: ValueAndType = parse_value_and_type(&typ, &s).unwrap();
        let result: Value = Value::from(round_trip_value);
        assert_eq!(value, result);
    }

    #[test]
    fn round_trip_u8() {
        round_trip(Value::U8(42), u8());
    }

    #[test]
    fn round_trip_u16() {
        round_trip(Value::U16(1234), u16());
    }

    #[test]
    fn round_trip_u32() {
        round_trip(Value::U32(123456), u32());
    }

    #[test]
    fn round_trip_u64() {
        round_trip(Value::U64(1234567890123456), u64());
    }

    #[test]
    fn round_trip_s8() {
        round_trip(Value::S8(-42), s8());
    }

    #[test]
    fn round_trip_s16() {
        round_trip(Value::S16(-1234), s16());
    }

    #[test]
    fn round_trip_s32() {
        round_trip(Value::S32(-123456), s32());
    }

    #[test]
    fn round_trip_s64() {
        round_trip(Value::S64(-1234567890123456), s64());
    }

    #[test]
    fn round_trip_f32() {
        round_trip(Value::F32(1234.5678), f32());
    }

    #[test]
    fn round_trip_f64() {
        round_trip(Value::F64(1_234_567_890_123_456.8), f64());
    }

    #[test]
    fn round_trip_bool() {
        round_trip(Value::Bool(true), bool());
    }

    #[test]
    fn round_trip_char() {
        round_trip(Value::Char('a'), chr());
    }

    #[test]
    fn round_trip_string() {
        round_trip(Value::String("hello".to_string()), str());
    }

    #[test]
    fn round_trip_list_1() {
        round_trip(
            Value::List(vec![Value::U8(1), Value::U8(2), Value::U8(3)]),
            list(u8()),
        );
    }

    #[test]
    fn round_trip_list_2() {
        round_trip(
            Value::List(vec![Value::List(vec![
                Value::String("hello".to_string()),
                Value::String("world".to_string()),
            ])]),
            list(list(str())),
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
            record(vec![
                field("a", u8()),
                field("b", str()),
                field("c", bool()),
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
            tuple(vec![u8(), str(), bool()]),
        );
    }

    #[test]
    fn round_trip_variant() {
        round_trip(
            Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(Value::String("hello".to_string()))),
            },
            variant(vec![unit_case("A"), case("B", str())]),
        );
    }

    #[test]
    fn round_trip_enum() {
        round_trip(Value::Enum(1), r#enum(&["A", "B"]));
    }

    #[test]
    fn round_trip_option() {
        round_trip(Value::Option(Some(Box::new(Value::U8(1)))), option(u8()));
    }

    #[test]
    fn round_trip_result_ok() {
        round_trip(
            Value::Result(Ok(Some(Box::new(Value::U8(1))))),
            result_ok(u8()),
        );
    }

    #[test]
    fn round_trip_result_err() {
        round_trip(
            Value::Result(Err(Some(Box::new(Value::U8(1))))),
            result_err(u8()),
        );
    }

    #[test]
    fn round_trip_flags() {
        round_trip(
            Value::Flags(vec![true, false, true]),
            flags(&["A", "B", "C"]),
        );
    }
}
