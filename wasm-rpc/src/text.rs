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
use std::io;
use wasm_wave::wasm::{WasmType, WasmTypeKind, WasmValue, WasmValueError};
use wasm_wave::from_str;
use wasm_wave::writer as wave_writer;
use wasm_wave::writer::{WriterError};

#[cfg(all(feature = "typeinfo", feature = "protobuf"))]
pub use type_annotated_value::*;

pub fn parse_value_and_type(
    analysed_type: &AnalysedType,
    input: &str,
) -> Result<ValueAndType, String> {
    let parsed: ValueAndType = from_str(analysed_type, input).map_err(|err| err.to_string())?;
    Ok(parsed)
}

pub fn print_value_and_type(value: &ValueAndType) -> Result<String, String> {
    let mut buf = vec![];
    let inner_writer = wave_writer::Writer::new(&mut buf);
    let mut text_writer = TextWriter::new(inner_writer);

    text_writer.write_value(value).map_err(|err| err.to_string())?;
    Ok(String::from_utf8(buf).unwrap_or_else(|err| panic!("invalid UTF-8: {err:?}")))
}

/// A writer that serializes `WasmValue` implementors to the WAVE text format,
/// by wrapping an existing `wasm_wave::writer::Writer`.
pub struct TextWriter<W: io::Write> {
    inner_wave_writer: wave_writer::Writer<W>,
}

impl<W: io::Write> TextWriter<W> {
    /// Creates a new `TextWriter` from an existing `wave_writer::Writer<W>` instance.
    /// The `wave_writer::Writer` itself wraps an `io::Write` sink.
    pub fn new(inner_wave_writer: wave_writer::Writer<W>) -> Self {
        Self { inner_wave_writer }
    }

    fn has_unsupported<V>(&mut self, val: &V) -> bool
    where
        V: WasmValue,
    {
        match val.kind() {
            WasmTypeKind::List => val.unwrap_list().any(|item_cow| self.has_unsupported(item_cow.as_ref())),
            WasmTypeKind::Record => val.unwrap_record().any(|(_, item_cow)| self.has_unsupported(item_cow.as_ref())),
            WasmTypeKind::Tuple => val.unwrap_tuple().any(|item_cow| self.has_unsupported(item_cow.as_ref())),
            WasmTypeKind::Variant => {
                val.unwrap_variant().1.map_or(false, |inner_val_cow| self.has_unsupported(inner_val_cow.as_ref()))
            }
            WasmTypeKind::Option => val.unwrap_option().map_or(false, |inner_val_cow| self.has_unsupported(inner_val_cow.as_ref())),
            WasmTypeKind::Result => {
                match val.unwrap_result() {
                    Ok(Some(ok_val_cow)) => self.has_unsupported(ok_val_cow.as_ref()),
                    Err(Some(err_val_cow)) => self.has_unsupported(err_val_cow.as_ref()),
                    _ => false, // Ok(None) or Err(None)
                }
            }
            WasmTypeKind::Unsupported => true,
            _ => false, // Primitives and other types
        }
    }

    /// Writes a `WasmValue` to the underlying stream in WAVE text format.
    ///
    /// This method directly delegates to the `write_value` method of the wrapped
    /// `wasm_wave::writer::Writer`.
    ///
    /// # Arguments
    /// * `val`: A reference to a value that implements `WasmValue`.
    ///
    /// # Errors
    /// Returns a `TextWriterError` if writing fails, typically by propagating
    /// an error from the underlying `wasm-wave` writer or an IO error.
    pub fn write_value<V>(&mut self, val: &V) -> Result<(), WriterError>
    where
        V: WasmValue,
    {
        if self.has_unsupported(val) {
            let placeholder_value_and_type = ValueAndType::make_string(Cow::Borrowed("<unsupported>"));
            return self.inner_wave_writer.write_value(&placeholder_value_and_type);
        }
        self.inner_wave_writer.write_value(val)?;
        Ok(())
    }

    // /// Gets a mutable reference to the underlying `io::Write` sink
    // /// originally wrapped by the `wave_writer::Writer`.
    // pub fn get_mut(&mut self) -> &mut W {
    //     self.inner_wave_writer.as_mut()
    // }
    //
    // /// Unwraps this `TextWriter`, returning the underlying `wasm_wave::writer::Writer<W>`.
    // pub fn into_wave_writer(self) -> wave_writer::Writer<W> {
    //     self.inner_wave_writer
    // }
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

    fn make_string(val: Cow<'_, str>) -> Self {
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

    use crate::text::{parse_value_and_type, print_value_and_type, TextWriter};
    use crate::{Value, ValueAndType};
    use golem_wasm_ast::analysis::analysed_type::{
        bool, case, chr, f32, f64, field, flags, list, option, r#enum, record, result_err,
        result_ok, s16, s32, s64, s8, str, tuple, u16, u32, u64, u8, unit_case, variant,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use wasm_wave::writer as wave_writer; // For creating the wave_writer::Writer

    fn round_trip(value: Value, typ: AnalysedType) {
        let typed_value = ValueAndType::new(value.clone(), typ.clone());

        let s_via_to_string = print_value_and_type(&typed_value).unwrap();
        let round_trip_value_from_string: ValueAndType =
            parse_value_and_type(&typ, &s_via_to_string).unwrap();
        assert_eq!(value, Value::from(round_trip_value_from_string));

        let mut buffer = Vec::new();
        let inner_writer = wave_writer::Writer::new(&mut buffer);
        let mut text_writer = TextWriter::new(inner_writer);
        text_writer.write_value(&typed_value).unwrap();
        let s_via_writer = String::from_utf8(buffer).unwrap();

        assert_eq!(s_via_to_string, s_via_writer, "Output from to_string and TextWriter should match");

        let round_trip_value_from_writer: ValueAndType =
            parse_value_and_type(&typ, &s_via_writer).unwrap();
        assert_eq!(value, Value::from(round_trip_value_from_writer));
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

#[cfg(all(feature = "typeinfo", feature = "protobuf"))]
mod type_annotated_value {
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
    use wasm_wave::from_str;
    use wasm_wave::writer::Writer;
    use crate::text::TextWriter;

    pub fn parse_type_annotated_value(
        analysed_type: &AnalysedType,
        input: &str,
    ) -> Result<TypeAnnotatedValue, String> {
        let parsed_typed_value: TypeAnnotatedValuePrintable =
            from_str(analysed_type, input).map_err(|err| err.to_string())?;

        Ok(parsed_typed_value.0)
    }

    pub fn print_type_annotated_value(value: &TypeAnnotatedValue) -> Result<String, String> {
        let mut buf = vec![];
        let inner_writer = Writer::new(&mut buf);
        let mut text_writer = TextWriter::new(inner_writer);

        let printable_typed_value = TypeAnnotatedValuePrintable(value.clone());
        text_writer.write_value(&printable_typed_value).map_err(|err| err.to_string())?;

        Ok(String::from_utf8(buf).unwrap_or_else(|err| panic!("invalid UTF-8: {err:?}")))
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

        fn make_f32(val: f32) -> Self {
            TypeAnnotatedValuePrintable(TypeAnnotatedValue::F32(val))
        }

        fn make_f64(val: f64) -> Self {
            TypeAnnotatedValuePrintable(TypeAnnotatedValue::F64(val))
        }

        fn make_char(val: char) -> Self {
            TypeAnnotatedValuePrintable(TypeAnnotatedValue::Char(val as i32))
        }

        fn make_string(val: Cow<'_, str>) -> Self {
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
                typ: Some(match ty {
                    AnalysedType::Option(opt_inner) => opt_inner.inner.as_ref().into(),
                    _ => ty.into(),
                }),
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
                        Ok(Some(v)) => {
                            Some(ResultValue::OkValue(Box::new(RootTypeAnnotatedValue {
                                type_annotated_value: Some(v.0),
                            })))
                        }
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
                _ => panic!("Expected bool, found {self:?}"),
            }
        }

        fn unwrap_s8(&self) -> i8 {
            match self.0 {
                TypeAnnotatedValue::S8(value) => value as i8,
                _ => panic!("Expected s8, found {self:?}"),
            }
        }

        fn unwrap_s16(&self) -> i16 {
            match self.0 {
                TypeAnnotatedValue::S16(value) => value as i16,
                _ => panic!("Expected s16, found {self:?}"),
            }
        }

        fn unwrap_s32(&self) -> i32 {
            match self.0 {
                TypeAnnotatedValue::S32(value) => value,
                _ => panic!("Expected s32, found {self:?}"),
            }
        }

        fn unwrap_s64(&self) -> i64 {
            match self.0 {
                TypeAnnotatedValue::S64(value) => value,
                _ => panic!("Expected s64, found {self:?}"),
            }
        }

        fn unwrap_u8(&self) -> u8 {
            match self.0 {
                TypeAnnotatedValue::U8(value) => value as u8,
                _ => panic!("Expected u8, found {self:?}"),
            }
        }

        fn unwrap_u16(&self) -> u16 {
            match self.0 {
                TypeAnnotatedValue::U16(value) => value as u16,
                _ => panic!("Expected u16, found {self:?}"),
            }
        }

        fn unwrap_u32(&self) -> u32 {
            match self.0 {
                TypeAnnotatedValue::U32(value) => value,
                _ => panic!("Expected u32, found {self:?}"),
            }
        }

        fn unwrap_u64(&self) -> u64 {
            match self.0 {
                TypeAnnotatedValue::U64(value) => value,
                _ => panic!("Expected u64, found {self:?}"),
            }
        }

        fn unwrap_f32(&self) -> f32 {
            match self.0 {
                TypeAnnotatedValue::F32(value) => value,
                _ => panic!("Expected f32, found {self:?}"),
            }
        }

        fn unwrap_f64(&self) -> f64 {
            match self.0 {
                TypeAnnotatedValue::F64(value) => value,
                _ => panic!("Expected f64, found {self:?}"),
            }
        }

        fn unwrap_char(&self) -> char {
            match self.0 {
                TypeAnnotatedValue::Char(value) => char::from_u32(value as u32).unwrap_or_else(|| panic!("Invalid char value: {value}")),
                _ => panic!("Expected chr, found {self:?}"),
            }
        }

        fn unwrap_string(&self) -> Cow<'_, str> {
            match &self.0 {
                TypeAnnotatedValue::Str(value) => Cow::Borrowed(value),
                _ => panic!("Expected string, found {self:?}"),
            }
        }

        fn unwrap_list(&self) -> Box<dyn Iterator<Item=Cow<'_, Self>> + '_> {
            match &self.0 {
                TypeAnnotatedValue::List(TypedList { typ: _, values }) => {
                    Box::new(values.iter().map(|v| {
                        Cow::Owned(TypeAnnotatedValuePrintable(
                            v.type_annotated_value.as_ref().expect("List item value missing").clone(),
                        ))
                    }))
                }
                _ => panic!("Expected list, found {self:?}"),
            }
        }

        fn unwrap_record(&self) -> Box<dyn Iterator<Item=(Cow<'_, str>, Cow<'_, Self>)> + '_> {
            match &self.0 {
                TypeAnnotatedValue::Record(TypedRecord { typ: _, value }) => {
                    Box::new(value.iter().map(|name_value| {
                        let name = Cow::Borrowed(name_value.name.as_str());
                        let type_annotated_value =
                            name_value.value.as_ref().expect("Record field value missing").type_annotated_value.as_ref().expect("Record field inner value missing").clone();
                        (
                            name,
                            Cow::Owned(TypeAnnotatedValuePrintable(type_annotated_value)),
                        )
                    }))
                }
                _ => panic!("Expected record, found {self:?}"),
            }
        }

        fn unwrap_tuple(&self) -> Box<dyn Iterator<Item=Cow<'_, Self>> + '_> {
            match &self.0 {
                TypeAnnotatedValue::Tuple(TypedTuple { typ: _, value }) => {
                    Box::new(value.iter().map(|x| {
                        if let Some(ref v) = x.type_annotated_value {
                            Cow::Owned(TypeAnnotatedValuePrintable(v.clone()))
                        } else {
                            panic!("Expected value in tuple element, found None")
                        }
                    }))
                }
                _ => panic!("Expected tuple, found {self:?}"),
            }
        }

        fn unwrap_variant(&self) -> (Cow<'_, str>, Option<Cow<'_, Self>>) {
            match &self.0 {
                TypeAnnotatedValue::Variant(variant) => {
                    let case_name = Cow::Borrowed(variant.case_name.as_str());
                    let case_value = variant.case_value.as_ref().map(|v| {
                        Cow::Owned(TypeAnnotatedValuePrintable(v.type_annotated_value.as_ref().expect("Variant inner value missing").clone()))
                    });
                    (case_name, case_value)
                }
                _ => panic!("Expected variant, found {self:?}"),
            }
        }

        fn unwrap_enum(&self) -> Cow<'_, str> {
            match &self.0 {
                TypeAnnotatedValue::Enum(TypedEnum { typ: _, value }) => Cow::Borrowed(value),
                _ => panic!("Expected enum, found {self:?}"),
            }
        }

        fn unwrap_option(&self) -> Option<Cow<'_, Self>> {
            match &self.0 {
                TypeAnnotatedValue::Option(option) => option.value.as_ref().and_then(|v| {
                    v.type_annotated_value
                        .as_ref()
                        .map(|inner| Cow::Owned(TypeAnnotatedValuePrintable(inner.clone())))
                }),
                _ => panic!("Expected option, found {self:?}"),
            }
        }

        fn unwrap_result(&self) -> Result<Option<Cow<'_, Self>>, Option<Cow<'_, Self>>> {
            match &self.0 {
                TypeAnnotatedValue::Result(result0) => match result0.result_value.as_ref() {
                    Some(result) => match result {
                        ResultValue::OkValue(ok) => match ok.type_annotated_value.as_ref() {
                            Some(ok_value) => {
                                Ok(Some(Cow::Owned(TypeAnnotatedValuePrintable(ok_value.clone()))))
                            }
                            None => Ok(None),
                        },
                        ResultValue::ErrorValue(error) => match error.type_annotated_value.as_ref() {
                            Some(error_value) => {
                                Err(Some(Cow::Owned(TypeAnnotatedValuePrintable(error_value.clone()))))
                            }
                            None => Err(None),
                        },
                    },
                    None => Ok(None),
                },
                _ => panic!("Expected result, found {self:?}"),
            }
        }

        fn unwrap_flags(&self) -> Box<dyn Iterator<Item=Cow<'_, str>> + '_> {
            match &self.0 {
                TypeAnnotatedValue::Flags(TypedFlags { typ: _, values }) => {
                    Box::new(values.iter().map(|s| Cow::Borrowed(s.as_str())))
                }
                _ => panic!("Expected flags, found {self:?}"),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use test_r::test;

        use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
        use crate::text::parse_type_annotated_value;
        use crate::{print_type_annotated_value, TypeAnnotatedValueConstructors, Value};
        use golem_wasm_ast::analysis::analysed_type::{
            bool, case, chr, f32, f64, field, flags, list, option, r#enum, record, result_err,
            result_ok, s16, s32, s64, s8, str, tuple, u16, u32, u64, u8, unit_case, variant,
        };
        use golem_wasm_ast::analysis::AnalysedType;

        fn round_trip(value: Value, typ: AnalysedType) {
            let typed_value = TypeAnnotatedValue::create(&value, &typ).unwrap();

            let s = print_type_annotated_value(&typed_value).unwrap();
            let round_trip_value: TypeAnnotatedValue =
                parse_type_annotated_value(&AnalysedType::try_from(&typed_value).unwrap(), &s)
                    .unwrap();
            let result: Value = Value::try_from(round_trip_value).unwrap();
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
}
