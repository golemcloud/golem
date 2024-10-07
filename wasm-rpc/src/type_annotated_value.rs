use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
use crate::protobuf::typed_result::ResultValue;
use crate::protobuf::{NameValuePair, TypedOption};
use crate::protobuf::{TypeAnnotatedValue as RootTypeAnnotatedValue, TypedResult};
use crate::protobuf::{
    TypedEnum, TypedFlags, TypedHandle, TypedList, TypedRecord, TypedTuple, TypedVariant,
};
use crate::{Value, WitValue};
use golem_wasm_ast::analysis::analysed_type::{
    bool, chr, f32, f64, list, option, result, result_err, result_ok, s16, s32, s64, s8, str,
    tuple, u16, u32, u64, u8,
};
use golem_wasm_ast::analysis::protobuf::Type;
use golem_wasm_ast::analysis::AnalysedType;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "bincode", derive(::bincode::Encode, ::bincode::Decode))]
pub struct ValueAndType {
    pub value: Value,
    pub typ: AnalysedType,
}

impl ValueAndType {
    pub fn new(value: Value, typ: AnalysedType) -> Self {
        Self { value, typ }
    }
}

impl From<ValueAndType> for Value {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.value
    }
}

impl From<ValueAndType> for AnalysedType {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.typ
    }
}

impl From<ValueAndType> for WitValue {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.value.into()
    }
}

impl TryFrom<ValueAndType> for TypeAnnotatedValue {
    type Error = Vec<String>;

    fn try_from(value_and_type: ValueAndType) -> Result<Self, Self::Error> {
        TypeAnnotatedValue::create(&value_and_type.value, &value_and_type.typ)
    }
}

impl TryFrom<TypeAnnotatedValue> for ValueAndType {
    type Error = String;

    fn try_from(value: TypeAnnotatedValue) -> Result<Self, Self::Error> {
        let typ: AnalysedType = (&value).try_into()?;
        let value: Value = value.try_into()?;
        Ok(Self::new(value, typ))
    }
}

/// Specific trait to convert a type into a `ValueAndType` type.
pub trait IntoValue {
    fn into_value(self) -> Value;
    fn get_type() -> AnalysedType;
}

pub trait IntoValueAndType {
    fn into_value_and_type(self) -> ValueAndType;
}

impl<T: IntoValue + Sized> IntoValueAndType for T {
    fn into_value_and_type(self) -> ValueAndType {
        ValueAndType::new(self.into_value(), Self::get_type())
    }
}

impl IntoValue for u8 {
    fn into_value(self) -> Value {
        Value::U8(self)
    }

    fn get_type() -> AnalysedType {
        u8()
    }
}

impl IntoValue for u16 {
    fn into_value(self) -> Value {
        Value::U16(self)
    }

    fn get_type() -> AnalysedType {
        u16()
    }
}

impl IntoValue for u32 {
    fn into_value(self) -> Value {
        Value::U32(self)
    }

    fn get_type() -> AnalysedType {
        u32()
    }
}

impl IntoValue for u64 {
    fn into_value(self) -> Value {
        Value::U64(self)
    }

    fn get_type() -> AnalysedType {
        u64()
    }
}

impl IntoValue for i8 {
    fn into_value(self) -> Value {
        Value::S8(self)
    }

    fn get_type() -> AnalysedType {
        s8()
    }
}

impl IntoValue for i16 {
    fn into_value(self) -> Value {
        Value::S16(self)
    }

    fn get_type() -> AnalysedType {
        s16()
    }
}

impl IntoValue for i32 {
    fn into_value(self) -> Value {
        Value::S32(self)
    }

    fn get_type() -> AnalysedType {
        s32()
    }
}

impl IntoValue for i64 {
    fn into_value(self) -> Value {
        Value::S64(self)
    }

    fn get_type() -> AnalysedType {
        s64()
    }
}

impl IntoValue for f32 {
    fn into_value(self) -> Value {
        Value::F32(self)
    }

    fn get_type() -> AnalysedType {
        f32()
    }
}

impl IntoValue for f64 {
    fn into_value(self) -> Value {
        Value::F64(self)
    }

    fn get_type() -> AnalysedType {
        f64()
    }
}

impl IntoValue for bool {
    fn into_value(self) -> Value {
        Value::Bool(self)
    }

    fn get_type() -> AnalysedType {
        bool()
    }
}

impl IntoValue for char {
    fn into_value(self) -> Value {
        Value::Char(self)
    }

    fn get_type() -> AnalysedType {
        chr()
    }
}

impl IntoValue for String {
    fn into_value(self) -> Value {
        Value::String(self)
    }

    fn get_type() -> AnalysedType {
        str()
    }
}

impl<S: IntoValue, E: IntoValue> IntoValue for Result<S, E> {
    fn into_value(self) -> Value {
        match self {
            Ok(s) => Value::Result(Ok(Some(Box::new(s.into_value())))),
            Err(e) => Value::Result(Err(Some(Box::new(e.into_value())))),
        }
    }

    fn get_type() -> AnalysedType {
        result(S::get_type(), E::get_type())
    }
}

impl<E: IntoValue> IntoValue for Result<(), E> {
    fn into_value(self) -> Value {
        match self {
            Ok(_) => Value::Result(Ok(None)),
            Err(e) => Value::Result(Err(Some(Box::new(e.into_value())))),
        }
    }

    fn get_type() -> AnalysedType {
        result_err(E::get_type())
    }
}

impl<S: IntoValue> IntoValue for Result<S, ()> {
    fn into_value(self) -> Value {
        match self {
            Ok(s) => Value::Result(Ok(Some(Box::new(s.into_value())))),
            Err(_) => Value::Result(Err(None)),
        }
    }

    fn get_type() -> AnalysedType {
        result_ok(S::get_type())
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> Value {
        match self {
            Some(t) => Value::Option(Some(Box::new(t.into_value()))),
            None => Value::Option(None),
        }
    }

    fn get_type() -> AnalysedType {
        option(T::get_type())
    }
}

impl<T: IntoValue> IntoValue for Vec<T> {
    fn into_value(self) -> Value {
        Value::List(self.into_iter().map(IntoValue::into_value).collect())
    }

    fn get_type() -> AnalysedType {
        list(T::get_type())
    }
}

impl<A: IntoValue, B: IntoValue> IntoValue for (A, B) {
    fn into_value(self) -> Value {
        Value::Tuple(vec![self.0.into_value(), self.1.into_value()])
    }

    fn get_type() -> AnalysedType {
        tuple(vec![A::get_type(), B::get_type()])
    }
}

impl<A: IntoValue, B: IntoValue, C: IntoValue> IntoValue for (A, B, C) {
    fn into_value(self) -> Value {
        Value::Tuple(vec![
            self.0.into_value(),
            self.1.into_value(),
            self.2.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        tuple(vec![A::get_type(), B::get_type(), C::get_type()])
    }
}

impl<K: IntoValue, V: IntoValue> IntoValue for HashMap<K, V> {
    fn into_value(self) -> Value {
        Value::List(
            self.into_iter()
                .map(|(k, v)| Value::Tuple(vec![k.into_value(), v.into_value()]))
                .collect(),
        )
    }

    fn get_type() -> AnalysedType {
        list(tuple(vec![K::get_type(), V::get_type()]))
    }
}

impl IntoValue for Uuid {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

pub trait TypeAnnotatedValueConstructors: Sized {
    fn create<T: Into<Type>>(value: &Value, typ: T) -> Result<Self, Vec<String>>;
}

impl TypeAnnotatedValueConstructors for TypeAnnotatedValue {
    fn create<T: Into<Type>>(value: &Value, typ: T) -> Result<TypeAnnotatedValue, Vec<String>> {
        let tpe: Type = typ.into();
        create_from_type(value, &tpe)
    }
}

fn create_from_type(val: &Value, typ: &Type) -> Result<TypeAnnotatedValue, Vec<String>> {
    match val {
        Value::Bool(bool) => Ok(TypeAnnotatedValue::Bool(*bool)),
        Value::S8(value) => Ok(TypeAnnotatedValue::S8(*value as i32)),
        Value::U8(value) => Ok(TypeAnnotatedValue::U8(*value as u32)),
        Value::U32(value) => Ok(TypeAnnotatedValue::U32(*value)),
        Value::S16(value) => Ok(TypeAnnotatedValue::S16(*value as i32)),
        Value::U16(value) => Ok(TypeAnnotatedValue::U16(*value as u32)),
        Value::S32(value) => Ok(TypeAnnotatedValue::S32(*value)),
        Value::S64(value) => Ok(TypeAnnotatedValue::S64(*value)),
        Value::U64(value) => Ok(TypeAnnotatedValue::U64(*value)),
        Value::F32(value) => Ok(TypeAnnotatedValue::F32(*value)),
        Value::F64(value) => Ok(TypeAnnotatedValue::F64(*value)),
        Value::Char(value) => Ok(TypeAnnotatedValue::Char(*value as i32)),
        Value::String(value) => Ok(TypeAnnotatedValue::Str(value.clone())),

        Value::Enum(value) => match &typ.r#type {
            Some(golem_wasm_ast::analysis::protobuf::r#type::Type::Enum(typ_enum)) => {
                match typ_enum.names.get(*value as usize) {
                    Some(name) => Ok(TypeAnnotatedValue::Enum(TypedEnum {
                        typ: typ_enum.names.clone(),
                        value: name.clone(),
                    })),
                    None => Err(vec![format!("Invalid enum value {}", value)]),
                }
            }
            _ => Err(vec![format!(
                "Unexpected type; expected an Enum type for value {}",
                value
            )]),
        },

        Value::Option(value) => match &typ.r#type {
            Some(golem_wasm_ast::analysis::protobuf::r#type::Type::Option(typ_option)) => {
                match value {
                    Some(value) => {
                        if let Some(inner_type) = &typ_option.elem {
                            let result = create_from_type(value, inner_type)?;
                            Ok(TypeAnnotatedValue::Option(Box::new(TypedOption {
                                typ: Some((**inner_type).clone()),
                                value: Some(Box::new(RootTypeAnnotatedValue {
                                    type_annotated_value: Some(result),
                                })),
                            })))
                        } else {
                            Err(vec!["Unexpected inner type for Option.".to_string()])
                        }
                    }
                    None => Ok(TypeAnnotatedValue::Option(Box::new(TypedOption {
                        typ: typ_option.elem.as_deref().cloned(),
                        value: None,
                    }))),
                }
            }
            _ => Err(vec!["Unexpected type; expected an Option type.".to_string()]),
        },

        Value::Tuple(values) => match &typ.r#type {
            Some(golem_wasm_ast::analysis::protobuf::r#type::Type::Tuple(typ_tuple)) => {
                if values.len() != typ_tuple.elems.len() {
                    return Err(vec![format!(
                        "Tuple has unexpected number of elements: {} vs {}",
                        values.len(),
                        typ_tuple.elems.len(),
                    )]);
                }

                let mut errors = vec![];
                let mut results = vec![];

                for (value, tpe) in values.iter().zip(&typ_tuple.elems) {
                    match create_from_type(value, tpe) {
                        Ok(result) => results.push(result),
                        Err(errs) => errors.extend(errs),
                    }
                }

                if errors.is_empty() {
                    Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                        typ: typ_tuple.elems.clone(),
                        value: results
                            .into_iter()
                            .map(|v| RootTypeAnnotatedValue {
                                type_annotated_value: Some(v),
                            })
                            .collect(),
                    }))
                } else {
                    Err(errors)
                }
            }
            _ => Err(vec!["Unexpected type; expected a Tuple type.".to_string()]),
        },

        Value::List(values) => match &typ.r#type {
            Some(golem_wasm_ast::analysis::protobuf::r#type::Type::List(typ_list)) => {
                if let Some(inner_type) = &typ_list.elem {
                    let mut errors = vec![];
                    let mut results = vec![];

                    for value in values {
                        match create_from_type(value, inner_type) {
                            Ok(value) => results.push(value),
                            Err(errs) => errors.extend(errs),
                        }
                    }

                    if errors.is_empty() {
                        Ok(TypeAnnotatedValue::List(TypedList {
                            typ: Some((**inner_type).clone()),
                            values: results
                                .into_iter()
                                .map(|v| RootTypeAnnotatedValue {
                                    type_annotated_value: Some(v),
                                })
                                .collect(),
                        }))
                    } else {
                        Err(errors)
                    }
                } else {
                    Err(vec!["Unexpected inner type for List.".to_string()])
                }
            }
            _ => Err(vec!["Unexpected type; expected a List type.".to_string()]),
        },

        Value::Record(values) => match &typ.r#type {
            Some(golem_wasm_ast::analysis::protobuf::r#type::Type::Record(typ_record)) => {
                if values.len() != typ_record.fields.len() {
                    return Err(vec!["The total number of field values is zero".to_string()]);
                }

                let mut errors = vec![];
                let mut results = vec![];

                for (value, field) in values.iter().zip(&typ_record.fields) {
                    if let Some(field_type) = &field.typ {
                        match create_from_type(value, field_type) {
                            Ok(res) => results.push((field.name.clone(), res)),
                            Err(errs) => errors.extend(errs),
                        }
                    } else {
                        errors.push(format!("Missing type for field {}", field.name));
                    }
                }

                if errors.is_empty() {
                    Ok(TypeAnnotatedValue::Record(TypedRecord {
                        typ: typ_record.fields.clone(),
                        value: results
                            .into_iter()
                            .map(|(name, value)| NameValuePair {
                                name,
                                value: Some(RootTypeAnnotatedValue {
                                    type_annotated_value: Some(value),
                                }),
                            })
                            .collect(),
                    }))
                } else {
                    Err(errors)
                }
            }
            _ => Err(vec!["Unexpected type; expected a Record type.".to_string()]),
        },

        Value::Variant {
            case_idx,
            case_value,
        } => match &typ.r#type {
            Some(golem_wasm_ast::analysis::protobuf::r#type::Type::Variant(typ_variant)) => {
                if (*case_idx as usize) < typ_variant.cases.len() {
                    let cases = typ_variant.cases.clone();

                    let (case_name, case_tpe) = match cases.get(*case_idx as usize) {
                        Some(tpe) => Ok((tpe.name.clone(), tpe.typ.clone())),
                        None => Err(vec!["Variant not found in the expected types.".to_string()]),
                    }?;

                    match case_tpe {
                        Some(tpe) => match case_value {
                            Some(case_value) => {
                                let result = create_from_type(case_value, &tpe)?;

                                Ok(TypeAnnotatedValue::Variant(Box::new(TypedVariant {
                                    typ: Some(golem_wasm_ast::analysis::protobuf::TypeVariant {
                                        cases,
                                    }),
                                    case_name: case_name.clone(),
                                    case_value: Some(Box::new(RootTypeAnnotatedValue {
                                        type_annotated_value: Some(result),
                                    })),
                                })))
                            }
                            None => Err(vec![format!("Missing value for case {case_name}")]),
                        },
                        None => Ok(TypeAnnotatedValue::Variant(Box::new(TypedVariant {
                            typ: Some(golem_wasm_ast::analysis::protobuf::TypeVariant { cases }),
                            case_name: case_name.clone(),
                            case_value: None,
                        }))),
                    }
                } else {
                    Err(vec![
                        "Invalid discriminant value for the variant.".to_string()
                    ])
                }
            }
            _ => Err(vec!["Unexpected type; expected a Variant type.".to_string()]),
        },

        Value::Flags(values) => match &typ.r#type {
            Some(golem_wasm_ast::analysis::protobuf::r#type::Type::Flags(typ_flags)) => {
                if values.len() != typ_flags.names.len() {
                    return Err(vec![format!(
                        "Unexpected number of flag states: {:?} vs {:?}",
                        values.len(),
                        typ_flags.names.len()
                    )]);
                }

                let enabled_flags: Vec<String> = values
                    .iter()
                    .zip(typ_flags.names.iter())
                    .filter_map(|(enabled, name)| if *enabled { Some(name.clone()) } else { None })
                    .collect();

                Ok(TypeAnnotatedValue::Flags(TypedFlags {
                    typ: typ_flags.names.clone(),
                    values: enabled_flags,
                }))
            }
            _ => Err(vec!["Unexpected type; expected a Flags type.".to_string()]),
        },

        Value::Result(value) => match &typ.r#type {
            Some(golem_wasm_ast::analysis::protobuf::r#type::Type::Result(typ_result)) => {
                match (value, &typ_result.ok, &typ_result.err) {
                    (Ok(Some(value)), Some(ok_type), _) => {
                        let result = create_from_type(value, ok_type)?;

                        Ok(TypeAnnotatedValue::Result(Box::new(TypedResult {
                            ok: Some(ok_type.as_ref().clone()),
                            error: typ_result.err.clone().map(|t| (*t).clone()),
                            result_value: Some(ResultValue::OkValue(Box::new(
                                RootTypeAnnotatedValue {
                                    type_annotated_value: Some(result),
                                },
                            ))),
                        })))
                    }
                    (Ok(None), Some(_), _) => {
                        Err(vec!["Non-unit ok result has no value".to_string()])
                    }

                    (Ok(None), None, _) => Ok(TypeAnnotatedValue::Result(Box::new(TypedResult {
                        ok: typ_result.ok.clone().map(|t| (*t).clone()),
                        error: typ_result.err.clone().map(|t| (*t).clone()),
                        result_value: Some(ResultValue::OkValue(Box::new(
                            RootTypeAnnotatedValue {
                                type_annotated_value: None,
                            },
                        ))),
                    }))),

                    (Ok(Some(_)), None, _) => Err(vec!["Unit ok result has a value".to_string()]),

                    (Err(Some(value)), _, Some(err_type)) => {
                        let result = create_from_type(value, err_type)?;

                        Ok(TypeAnnotatedValue::Result(Box::new(TypedResult {
                            ok: typ_result.ok.clone().map(|t| (*t).clone()),
                            error: typ_result.err.clone().map(|t| (*t).clone()),
                            result_value: Some(ResultValue::ErrorValue(Box::new(
                                RootTypeAnnotatedValue {
                                    type_annotated_value: Some(result),
                                },
                            ))),
                        })))
                    }

                    (Err(None), _, Some(_)) => {
                        Err(vec!["Non-unit error result has no value".to_string()])
                    }

                    (Err(None), _, None) => Ok(TypeAnnotatedValue::Result(Box::new(TypedResult {
                        ok: typ_result.ok.clone().map(|t| (*t).clone()),
                        error: typ_result.err.clone().map(|t| (*t).clone()),
                        result_value: Some(ResultValue::ErrorValue(Box::new(
                            RootTypeAnnotatedValue {
                                type_annotated_value: None,
                            },
                        ))),
                    }))),

                    (Err(Some(_)), _, None) => {
                        Err(vec!["Unit error result has a value".to_string()])
                    }
                }
            }

            _ => Err(vec!["Unexpected type; expected a Result type.".to_string()]),
        },

        Value::Handle { uri, resource_id } => match &typ.r#type {
            Some(golem_wasm_ast::analysis::protobuf::r#type::Type::Handle(typ_handle)) => {
                let handle = TypedHandle {
                    uri: uri.value.clone(),
                    resource_id: *resource_id,
                    typ: Some(typ_handle.clone()),
                };
                Ok(TypeAnnotatedValue::Handle(handle))
            }
            _ => Err(vec![
                "Unexpected type; expected a Resource type.".to_string()
            ]),
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
    use crate::{TypeAnnotatedValueConstructors, Value};
    use golem_wasm_ast::analysis::analysed_type::u32;
    use golem_wasm_ast::analysis::protobuf::{r#type, PrimitiveType, TypePrimitive};

    #[test]
    fn test_type_annotated_value_from_analysed_type() {
        let analysed_type = u32();

        let result = TypeAnnotatedValue::create(&Value::U32(1), &analysed_type);

        let expected = TypeAnnotatedValue::U32(1);

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_type_annotated_value_from_type() {
        let typ0 = r#type::Type::Primitive(TypePrimitive {
            primitive: PrimitiveType::Bool as i32,
        });

        let typ = golem_wasm_ast::analysis::protobuf::Type { r#type: Some(typ0) };

        let result = TypeAnnotatedValue::create(&Value::U32(1), typ);

        let expected = TypeAnnotatedValue::U32(1);

        assert_eq!(result, Ok(expected));
    }
}
