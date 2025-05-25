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

use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
use crate::protobuf::typed_result::ResultValue;
use crate::protobuf::{NameValuePair, TypedOption};
use crate::protobuf::{TypeAnnotatedValue as RootTypeAnnotatedValue, TypedResult};
use crate::protobuf::{
    TypedEnum, TypedFlags, TypedHandle, TypedList, TypedRecord, TypedTuple, TypedVariant,
};
use crate::{Value, ValueAndType};
use golem_wasm_ast::analysis::protobuf::Type;
use golem_wasm_ast::analysis::AnalysedType;

impl TryFrom<ValueAndType> for TypeAnnotatedValue {
    type Error = Vec<String>;

    fn try_from(value_and_type: ValueAndType) -> Result<Self, Self::Error> {
        TypeAnnotatedValue::create(&value_and_type.value, &value_and_type.typ)
    }
}

impl TryFrom<&ValueAndType> for TypeAnnotatedValue {
    type Error = Vec<String>;

    fn try_from(value_and_type: &ValueAndType) -> Result<Self, Self::Error> {
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

impl TryFrom<crate::protobuf::TypeAnnotatedValue> for ValueAndType {
    type Error = String;

    fn try_from(value: crate::protobuf::TypeAnnotatedValue) -> Result<Self, Self::Error> {
        let inner = value
            .type_annotated_value
            .ok_or("Missing type_annotated_value field")?;
        let typ: AnalysedType = (&inner).try_into()?;
        let value: Value = inner.try_into()?;
        Ok(Self::new(value, typ))
    }
}

impl TryFrom<ValueAndType> for crate::protobuf::TypeAnnotatedValue {
    type Error = Vec<String>;

    fn try_from(value_and_type: ValueAndType) -> Result<Self, Self::Error> {
        Ok(crate::protobuf::TypeAnnotatedValue {
            type_annotated_value: Some(value_and_type.try_into()?),
        })
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
                    uri: uri.clone(),
                    resource_id: *resource_id,
                    typ: Some(*typ_handle),
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
    use test_r::test;

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
