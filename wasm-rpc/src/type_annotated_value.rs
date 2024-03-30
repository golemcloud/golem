use std::convert::{TryFrom, TryInto};

use golem_wasm_ast::analysis::{AnalysedResourceId, AnalysedResourceMode, AnalysedType};

use crate::{Uri, Value, WitValue};

use std::ops::Deref;

#[derive(Debug, Clone, PartialEq)]
pub enum TypeAnnotatedValue {
    Bool(bool),
    S8(i8),
    U8(u8),
    S16(i16),
    U16(u16),
    S32(i32),
    U32(u32),
    S64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Chr(char),
    Str(String),
    List {
        typ: AnalysedType,
        values: Vec<TypeAnnotatedValue>,
    },
    Tuple {
        typ: Vec<AnalysedType>,
        value: Vec<TypeAnnotatedValue>,
    },
    Record {
        typ: Vec<(String, AnalysedType)>,
        value: Vec<(String, TypeAnnotatedValue)>,
    },
    Flags {
        typ: Vec<String>,
        values: Vec<String>,
    },
    Variant {
        typ: Vec<(String, Option<AnalysedType>)>,
        case_name: String,
        case_value: Option<Box<TypeAnnotatedValue>>,
    },
    Enum {
        typ: Vec<String>,
        value: String,
    },
    Option {
        typ: AnalysedType,
        value: Option<Box<TypeAnnotatedValue>>,
    },
    Result {
        ok: Option<Box<AnalysedType>>,
        error: Option<Box<AnalysedType>>,
        value: Result<Option<Box<TypeAnnotatedValue>>, Option<Box<TypeAnnotatedValue>>>,
    },
    Handle {
        id: AnalysedResourceId,
        resource_mode: AnalysedResourceMode,
        uri: Uri,
        resource_id: u64,
    },
}

impl TypeAnnotatedValue {
    pub fn from_value(
        val: &Value,
        analysed_type: &AnalysedType,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        match val {
            Value::Bool(bool) => Ok(TypeAnnotatedValue::Bool(*bool)),
            Value::S8(value) => Ok(TypeAnnotatedValue::S8(*value)),
            Value::U8(value) => Ok(TypeAnnotatedValue::U8(*value)),
            Value::U32(value) => Ok(TypeAnnotatedValue::U32(*value)),
            Value::S16(value) => Ok(TypeAnnotatedValue::S16(*value)),
            Value::U16(value) => Ok(TypeAnnotatedValue::U16(*value)),
            Value::S32(value) => Ok(TypeAnnotatedValue::S32(*value)),
            Value::S64(value) => Ok(TypeAnnotatedValue::S64(*value)),
            Value::U64(value) => Ok(TypeAnnotatedValue::U64(*value)),
            Value::F32(value) => Ok(TypeAnnotatedValue::F32(*value)),
            Value::F64(value) => Ok(TypeAnnotatedValue::F64(*value)),
            Value::Char(value) => Ok(TypeAnnotatedValue::Chr(*value)),
            Value::String(value) => Ok(TypeAnnotatedValue::Str(value.clone())),

            Value::Enum(value) => match analysed_type {
                AnalysedType::Enum(names) => match names.get(*value as usize) {
                    Some(str) => Ok(TypeAnnotatedValue::Enum {
                        typ: names.clone(),
                        value: str.to_string(),
                    }),
                    None => Err(vec![format!("Invalid enum {}", value)]),
                },
                _ => Err(vec![format!("Unexpected enum {}", value)]),
            },

            Value::Option(value) => match analysed_type {
                AnalysedType::Option(elem) => Ok(TypeAnnotatedValue::Option {
                    typ: *elem.clone(),
                    value: match value {
                        Some(value) => Some(Box::new(Self::from_value(value, elem)?)),
                        None => None,
                    },
                }),

                _ => Err(vec!["Unexpected type; expected an Option type.".to_string()]),
            },

            Value::Tuple(values) => match analysed_type {
                AnalysedType::Tuple(types) => {
                    if values.len() != types.len() {
                        return Err(vec![format!(
                            "Tuple has unexpected number of elements: {} vs {}",
                            values.len(),
                            types.len(),
                        )]);
                    }

                    let mut errors = vec![];
                    let mut results = vec![];

                    for (value, tpe) in values.iter().zip(types.iter()) {
                        match Self::from_value(value, tpe) {
                            Ok(result) => results.push(result),
                            Err(errs) => errors.extend(errs),
                        }
                    }

                    if errors.is_empty() {
                        Ok(TypeAnnotatedValue::Tuple {
                            typ: types.clone(),
                            value: results,
                        })
                    } else {
                        Err(errors)
                    }
                }

                _ => Err(vec!["Unexpected type; expected a tuple type.".to_string()]),
            },

            Value::List(values) => match analysed_type {
                AnalysedType::List(elem) => {
                    let mut errors = vec![];
                    let mut results = vec![];

                    for value in values {
                        match Self::from_value(value, elem) {
                            Ok(value) => results.push(value),
                            Err(errs) => errors.extend(errs),
                        }
                    }

                    if errors.is_empty() {
                        Ok(TypeAnnotatedValue::List {
                            typ: *elem.clone(),
                            values: results,
                        })
                    } else {
                        Err(errors)
                    }
                }

                _ => Err(vec!["Unexpected type; expected a list type.".to_string()]),
            },

            Value::Record(values) => match analysed_type {
                AnalysedType::Record(fields) => {
                    if values.len() != fields.len() {
                        return Err(vec!["The total number of field values is zero".to_string()]);
                    }

                    let mut errors = vec![];
                    let mut results: Vec<(String, TypeAnnotatedValue)> = vec![];

                    for (value, (field_name, typ)) in values.iter().zip(fields) {
                        match TypeAnnotatedValue::from_value(value, typ) {
                            Ok(res) => {
                                results.push((field_name.clone(), res));
                            }
                            Err(errs) => errors.extend(errs),
                        }
                    }

                    if errors.is_empty() {
                        Ok(TypeAnnotatedValue::Record {
                            typ: fields.clone(),
                            value: results,
                        })
                    } else {
                        Err(errors)
                    }
                }

                _ => Err(vec!["Unexpected type; expected a variant type.".to_string()]),
            },

            Value::Variant {
                case_idx,
                case_value,
            } => match analysed_type {
                AnalysedType::Variant(cases) => {
                    if (*case_idx as usize) < cases.len() {
                        let (case_name, case_type) = match cases.get(*case_idx as usize) {
                            Some(tpe) => Ok(tpe),
                            None => {
                                Err(vec!["Variant not found in the expected types.".to_string()])
                            }
                        }?;

                        match case_type {
                            Some(tpe) => match case_value {
                                Some(case_value) => {
                                    let result = Self::from_value(case_value, tpe)?;
                                    Ok(TypeAnnotatedValue::Variant {
                                        typ: cases.clone(),
                                        case_name: case_name.clone(),
                                        case_value: Some(Box::new(result)),
                                    })
                                }
                                None => Err(vec![format!("Missing value for case {case_name}")]),
                            },
                            None => Ok(TypeAnnotatedValue::Variant {
                                typ: cases.clone(),
                                case_name: case_name.clone(),
                                case_value: None,
                            }),
                        }
                    } else {
                        Err(vec![
                            "Invalid discriminant value for the variant.".to_string()
                        ])
                    }
                }

                _ => Err(vec!["Unexpected type; expected a variant type.".to_string()]),
            },

            Value::Flags(values) => match analysed_type {
                AnalysedType::Flags(names) => {
                    let mut results = vec![];

                    if values.len() != names.len() {
                        Err(vec![format!(
                            "Unexpected number of flag states: {:?} vs {:?}",
                            values, names
                        )])
                    } else {
                        for (enabled, name) in values.iter().zip(names) {
                            if *enabled {
                                results.push(name.clone());
                            }
                        }

                        Ok(TypeAnnotatedValue::Flags {
                            typ: names.clone(),
                            values: results,
                        })
                    }
                }
                _ => Err(vec!["Unexpected type; expected a flags type.".to_string()]),
            },

            Value::Result(value) => match analysed_type {
                golem_wasm_ast::analysis::AnalysedType::Result { ok, error } => {
                    match (value, ok, error) {
                        (Ok(Some(value)), Some(ok_type), _) => {
                            let result = Self::from_value(value, ok_type)?;

                            Ok(TypeAnnotatedValue::Result {
                                value: Ok(Some(Box::new(result))),
                                ok: ok.clone(),
                                error: error.clone(),
                            })
                        }

                        (Ok(None), Some(_), _) => {
                            Err(vec!["Non-unit ok result has no value".to_string()])
                        }

                        (Ok(None), None, _) => Ok(TypeAnnotatedValue::Result {
                            value: Ok(None),
                            ok: ok.clone(),
                            error: error.clone(),
                        }),

                        (Ok(Some(_)), None, _) => {
                            Err(vec!["Unit ok result has a value".to_string()])
                        }

                        (Err(Some(value)), _, Some(err_type)) => {
                            let result = Self::from_value(value, err_type)?;

                            Ok(TypeAnnotatedValue::Result {
                                value: Err(Some(Box::new(result))),
                                ok: ok.clone(),
                                error: error.clone(),
                            })
                        }

                        (Err(None), _, Some(_)) => {
                            Err(vec!["Non-unit error result has no value".to_string()])
                        }

                        (Err(None), _, None) => Ok(TypeAnnotatedValue::Result {
                            value: Err(None),
                            ok: ok.clone(),
                            error: error.clone(),
                        }),

                        (Err(Some(_)), _, None) => {
                            Err(vec!["Unit error result has a value".to_string()])
                        }
                    }
                }

                _ => Err(vec!["Unexpected type; expected a Result type.".to_string()]),
            },
            Value::Handle { uri, resource_id } => match analysed_type {
                AnalysedType::Resource { id, resource_mode } => Ok(TypeAnnotatedValue::Handle {
                    id: id.clone(),
                    resource_mode: resource_mode.clone(),
                    uri: uri.clone(),
                    resource_id: *resource_id,
                }),
                _ => Err(vec!["Unexpected type; expected a Handle type.".to_string()]),
            },
        }
    }
}

impl From<&TypeAnnotatedValue> for AnalysedType {
    fn from(value: &TypeAnnotatedValue) -> Self {
        match value {
            TypeAnnotatedValue::Bool(_) => AnalysedType::Bool,
            TypeAnnotatedValue::S8(_) => AnalysedType::S8,
            TypeAnnotatedValue::U8(_) => AnalysedType::U8,
            TypeAnnotatedValue::S16(_) => AnalysedType::S16,
            TypeAnnotatedValue::U16(_) => AnalysedType::U16,
            TypeAnnotatedValue::S32(_) => AnalysedType::S32,
            TypeAnnotatedValue::U32(_) => AnalysedType::U32,
            TypeAnnotatedValue::S64(_) => AnalysedType::S64,
            TypeAnnotatedValue::U64(_) => AnalysedType::U64,
            TypeAnnotatedValue::F32(_) => AnalysedType::F32,
            TypeAnnotatedValue::F64(_) => AnalysedType::F64,
            TypeAnnotatedValue::Chr(_) => AnalysedType::Chr,
            TypeAnnotatedValue::Str(_) => AnalysedType::Str,
            TypeAnnotatedValue::List { typ, values: _ } => {
                AnalysedType::List(Box::new(typ.clone()))
            }
            TypeAnnotatedValue::Tuple { typ, value: _ } => AnalysedType::Tuple(typ.clone()),
            TypeAnnotatedValue::Record { typ, value: _ } => AnalysedType::Record(typ.clone()),
            TypeAnnotatedValue::Flags { typ, values: _ } => AnalysedType::Flags(typ.clone()),
            TypeAnnotatedValue::Enum { typ, value: _ } => AnalysedType::Enum(typ.clone()),
            TypeAnnotatedValue::Option { typ, value: _ } => {
                AnalysedType::Option(Box::new(typ.clone()))
            }
            TypeAnnotatedValue::Result {
                ok,
                error,
                value: _,
            } => AnalysedType::Result {
                ok: ok.clone(),
                error: error.clone(),
            },
            TypeAnnotatedValue::Handle {
                id,
                resource_mode,
                uri: _,
                resource_id: _,
            } => AnalysedType::Resource {
                id: id.clone(),
                resource_mode: resource_mode.clone(),
            },
            TypeAnnotatedValue::Variant {
                typ,
                case_name: _,
                case_value: _,
            } => AnalysedType::Variant(typ.clone()),
        }
    }
}
impl TryFrom<TypeAnnotatedValue> for WitValue {
    type Error = String;
    fn try_from(value: TypeAnnotatedValue) -> Result<Self, Self::Error> {
        let value: Result<Value, String> = value.try_into();
        value.map(|v| v.into())
    }
}

impl TryFrom<TypeAnnotatedValue> for Value {
    type Error = String;

    fn try_from(value: TypeAnnotatedValue) -> Result<Self, Self::Error> {
        match value {
            TypeAnnotatedValue::Bool(value) => Ok(Value::Bool(value)),
            TypeAnnotatedValue::S8(value) => Ok(Value::S8(value)),
            TypeAnnotatedValue::U8(value) => Ok(Value::U8(value)),
            TypeAnnotatedValue::S16(value) => Ok(Value::S16(value)),
            TypeAnnotatedValue::U16(value) => Ok(Value::U16(value)),
            TypeAnnotatedValue::S32(value) => Ok(Value::S32(value)),
            TypeAnnotatedValue::U32(value) => Ok(Value::U32(value)),
            TypeAnnotatedValue::S64(value) => Ok(Value::S64(value)),
            TypeAnnotatedValue::U64(value) => Ok(Value::U64(value)),
            TypeAnnotatedValue::F32(value) => Ok(Value::F32(value)),
            TypeAnnotatedValue::F64(value) => Ok(Value::F64(value)),
            TypeAnnotatedValue::Chr(value) => Ok(Value::Char(value)),
            TypeAnnotatedValue::Str(value) => Ok(Value::String(value)),
            TypeAnnotatedValue::List { typ: _, values } => {
                let mut list_values = Vec::new();
                for value in values {
                    match value.try_into() {
                        Ok(v) => list_values.push(v),
                        Err(err) => return Err(err),
                    }
                }
                Ok(Value::List(list_values))
            }
            TypeAnnotatedValue::Tuple { typ: _, value } => {
                let mut tuple_values = Vec::new();
                for value in value {
                    match value.try_into() {
                        Ok(v) => tuple_values.push(v),
                        Err(err) => return Err(err),
                    }
                }
                Ok(Value::Tuple(tuple_values))
            }
            TypeAnnotatedValue::Record { typ: _, value } => {
                let mut record_values = Vec::new();
                for (_, value) in value {
                    match value.try_into() {
                        Ok(v) => record_values.push(v),
                        Err(err) => return Err(err),
                    }
                }
                Ok(Value::Record(record_values))
            }
            TypeAnnotatedValue::Flags { typ, values } => {
                let mut bools = Vec::new();

                for expected_flag in typ {
                    if values.contains(&expected_flag) {
                        bools.push(true);
                    } else {
                        bools.push(false);
                    }
                }
                Ok(Value::Flags(bools))
            }
            TypeAnnotatedValue::Enum { typ, value } => typ
                .iter()
                .position(|expected_enum| expected_enum == &value)
                .map(|index| Value::Enum(index as u32))
                .ok_or_else(|| "Enum value not found in the list of expected enums".to_string()),

            TypeAnnotatedValue::Option { typ: _, value } => match value {
                Some(value) => {
                    let value: Value = value.deref().clone().try_into()?;
                    Ok(Value::Option(Some(Box::new(value))))
                }
                None => Ok(Value::Option(None)),
            },
            TypeAnnotatedValue::Result {
                ok: _,
                error: _,
                value,
            } => Ok(Value::Result(match value {
                Ok(value) => match value {
                    Some(v) => {
                        let value: Value = v.deref().clone().try_into()?;
                        Ok(Some(Box::new(value)))
                    }

                    None => Ok(None),
                },
                Err(value) => match value {
                    Some(v) => {
                        let value: Value = v.deref().clone().try_into()?;
                        Err(Some(Box::new(value)))
                    }

                    None => Err(None),
                },
            })),
            TypeAnnotatedValue::Handle {
                id: _,
                resource_mode: _,
                uri,
                resource_id,
            } => Ok(Value::Handle { uri, resource_id }),
            TypeAnnotatedValue::Variant {
                typ,
                case_name,
                case_value,
            } => match case_value {
                Some(value) => {
                    let result: Value = value.deref().clone().try_into()?;
                    Ok(Value::Variant {
                        case_idx: typ.iter().position(|(name, _)| name == &case_name).unwrap()
                            as u32,
                        case_value: Some(Box::new(result)),
                    })
                }
                None => Ok(Value::Variant {
                    case_idx: typ.iter().position(|(name, _)| name == &case_name).unwrap() as u32,
                    case_value: None,
                }),
            },
        }
    }
}
