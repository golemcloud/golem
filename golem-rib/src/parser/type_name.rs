// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::Display;
use std::ops::Deref;

use bincode::{Decode, Encode};
use combine::parser::char;
use combine::parser::char::{char, spaces, string};
use combine::{attempt, between, choice, optional, sep_by, Parser};
use combine::{parser, ParseError};
use golem_wasm_ast::analysis::{AnalysedType, TypeResult};

use crate::parser::errors::RibParseError;
use crate::rib_source_span::GetSourcePosition;
use crate::{InferredNumber, InferredType, TypeInternal};

// Rib grammar uses it's own `TypeName` instead of relying from any other crates to annotate types (Example: 1: u32, let x: u32 = 1;),
// and sticks on to the  Display instance that aligns with what we see in WIT.
// Usage of TypeName, InferredType and AnalysedType:
// The Rib compiler uses `InferredType` - the output of type inference. The `TypeName` used in type annotations may help with this type inference.
// The Rib-IR which is close to running Rib code uses `AnalysedType`, that there won't be either `TypeName` or `InferredType` in the Rib-IR.
// Any compilation or interpreter error messages will also be using `TypeName` to show the type of the expression
// for which we convert AnalysedType or InferredType back to TypeName. If `InferredType` cannot be converted to `TypeName`, we explain the error displaying
// the original expression, and there is no point displaying `InferredType` to the user.
#[derive(Debug, Hash, Clone, Eq, PartialEq, Encode, Decode, Ord, PartialOrd)]
pub enum TypeName {
    Bool,
    S8,
    U8,
    S16,
    U16,
    S32,
    U32,
    S64,
    U64,
    F32,
    F64,
    Chr,
    Str,
    List(Box<TypeName>),
    Tuple(Vec<TypeName>),
    Option(Box<TypeName>),
    Result {
        ok: Option<Box<TypeName>>,
        error: Option<Box<TypeName>>,
    },
    Record(Vec<(String, Box<TypeName>)>),
    Flags(Vec<String>),
    Enum(Vec<String>),
    Variant {
        cases: Vec<(String, Option<Box<TypeName>>)>,
    },
}

impl Display for TypeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeName::Bool => write!(f, "bool"),
            TypeName::S8 => write!(f, "s8"),
            TypeName::U8 => write!(f, "u8"),
            TypeName::S16 => write!(f, "s16"),
            TypeName::U16 => write!(f, "u16"),
            TypeName::S32 => write!(f, "s32"),
            TypeName::U32 => write!(f, "u32"),
            TypeName::S64 => write!(f, "s64"),
            TypeName::U64 => write!(f, "u64"),
            TypeName::F32 => write!(f, "f32"),
            TypeName::F64 => write!(f, "f64"),
            TypeName::Chr => write!(f, "char"),
            TypeName::Str => write!(f, "string"),
            TypeName::List(inner_type) => write!(f, "list<{}>", inner_type),
            TypeName::Tuple(inner_types) => {
                write!(f, "tuple<")?;
                for (i, inner_type) in inner_types.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", inner_type)?;
                }
                write!(f, ">")
            }
            TypeName::Option(inner_type) => write!(f, "option<{}>", inner_type),
            // https://component-model.bytecodealliance.org/design/wit.html#results
            TypeName::Result { ok, error } => match (ok, error) {
                (Some(ok), Some(error)) => {
                    write!(f, "result<{}, {}>", ok, error)
                }
                (Some(ok), None) => {
                    write!(f, "result<{}>", ok)
                }
                (None, Some(error)) => {
                    write!(f, "result<_, {}>", error)
                }
                (None, None) => {
                    write!(f, "result")
                }
            },
            TypeName::Record(fields) => {
                write!(f, "record {{ ")?;
                for (i, (field, typ)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", field, typ)?;
                }
                write!(f, " }}")
            }
            TypeName::Flags(flags) => {
                write!(f, "flags<")?;
                for (i, flag) in flags.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", flag)?;
                }
                write!(f, ">")
            }
            TypeName::Enum(cases) => {
                write!(f, "enum {{ ")?;
                for (i, case) in cases.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", case)?;
                }
                write!(f, " }}")
            }
            TypeName::Variant { cases } => {
                write!(f, "variant {{")?;
                for (i, (case, typ)) in cases.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", case)?;
                    if let Some(typ) = typ {
                        write!(f, "({})", typ)?;
                    }
                }
                write!(f, " }}")
            }
        }
    }
}

impl From<&InferredNumber> for TypeName {
    fn from(value: &InferredNumber) -> Self {
        match value {
            InferredNumber::S8 => TypeName::S8,
            InferredNumber::U8 => TypeName::U8,
            InferredNumber::S16 => TypeName::S16,
            InferredNumber::U16 => TypeName::U16,
            InferredNumber::S32 => TypeName::S32,
            InferredNumber::U32 => TypeName::U32,
            InferredNumber::S64 => TypeName::S64,
            InferredNumber::U64 => TypeName::U64,
            InferredNumber::F32 => TypeName::F32,
            InferredNumber::F64 => TypeName::F64,
        }
    }
}

impl TryFrom<AnalysedType> for TypeName {
    type Error = String;
    fn try_from(analysed_type: AnalysedType) -> Result<Self, Self::Error> {
        match analysed_type {
            AnalysedType::Bool(_) => Ok(TypeName::Bool),
            AnalysedType::S8(_) => Ok(TypeName::S8),
            AnalysedType::U8(_) => Ok(TypeName::U8),
            AnalysedType::S16(_) => Ok(TypeName::S16),
            AnalysedType::U16(_) => Ok(TypeName::U16),
            AnalysedType::S32(_) => Ok(TypeName::S32),
            AnalysedType::U32(_) => Ok(TypeName::U32),
            AnalysedType::S64(_) => Ok(TypeName::S64),
            AnalysedType::U64(_) => Ok(TypeName::U64),
            AnalysedType::F32(_) => Ok(TypeName::F32),
            AnalysedType::F64(_) => Ok(TypeName::F64),
            AnalysedType::Chr(_) => Ok(TypeName::Chr),
            AnalysedType::Str(_) => Ok(TypeName::Str),
            AnalysedType::List(inner_type) => Ok(TypeName::List(Box::new(
                inner_type.inner.deref().clone().try_into()?,
            ))),
            AnalysedType::Tuple(inner_type) => Ok(TypeName::Tuple(
                inner_type
                    .items
                    .into_iter()
                    .map(|x| x.try_into())
                    .collect::<Result<_, _>>()?,
            )),
            AnalysedType::Option(type_option) => Ok(TypeName::Option(Box::new(
                type_option.inner.deref().clone().try_into()?,
            ))),
            AnalysedType::Result(TypeResult { ok, err }) => match (ok, err) {
                (Some(ok), Some(err)) => Ok(TypeName::Result {
                    ok: Some(Box::new(ok.deref().clone().try_into()?)),
                    error: Some(Box::new(err.deref().clone().try_into()?)),
                }),
                (Some(ok), None) => Ok(TypeName::Result {
                    ok: Some(Box::new(ok.deref().clone().try_into()?)),
                    error: None,
                }),
                (None, Some(err)) => Ok(TypeName::Result {
                    ok: None,
                    error: Some(Box::new(err.deref().clone().try_into()?)),
                }),
                (None, None) => Ok(TypeName::Result {
                    ok: None,
                    error: None,
                }),
            },
            AnalysedType::Record(type_record) => {
                let mut fields = vec![];
                for field in type_record.fields {
                    let name = field.name.clone();
                    let typ = field.typ.clone();
                    let type_name = typ.try_into()?;
                    fields.push((name, Box::new(type_name)));
                }

                Ok(TypeName::Record(fields))
            }
            AnalysedType::Flags(flags) => Ok(TypeName::Flags(flags.names)),
            AnalysedType::Enum(cases) => Ok(TypeName::Enum(cases.cases)),
            AnalysedType::Variant(cases) => {
                let mut variant_cases = vec![];
                for case in cases.cases {
                    let name = case.name.clone();
                    let typ = case.typ.clone();
                    match typ {
                        Some(typ) => {
                            let type_name = typ.try_into()?;
                            variant_cases.push((name, Some(Box::new(type_name))));
                        }
                        None => {
                            variant_cases.push((name, None));
                        }
                    }
                }
                Ok(TypeName::Variant {
                    cases: variant_cases,
                })
            }
            AnalysedType::Handle(type_handle) => {
                Err(format!("Handle type not supported: {:?}", type_handle))
            }
        }
    }
}

impl From<&TypeName> for InferredType {
    fn from(type_name: &TypeName) -> Self {
        match type_name {
            TypeName::Bool => InferredType::bool(),
            TypeName::S8 => InferredType::s8(),
            TypeName::U8 => InferredType::u8(),
            TypeName::S16 => InferredType::s16(),
            TypeName::U16 => InferredType::u16(),
            TypeName::S32 => InferredType::s32(),
            TypeName::U32 => InferredType::u32(),
            TypeName::S64 => InferredType::s64(),
            TypeName::U64 => InferredType::u64(),
            TypeName::F32 => InferredType::f32(),
            TypeName::F64 => InferredType::f64(),
            TypeName::Chr => InferredType::char(),
            TypeName::Str => InferredType::string(),
            TypeName::List(inner_type) => InferredType::list(inner_type.deref().into()),
            TypeName::Tuple(inner_types) => {
                InferredType::tuple(inner_types.iter().map(|t| t.into()).collect())
            }
            TypeName::Option(type_name) => InferredType::option(type_name.deref().into()),
            TypeName::Result { ok, error } => InferredType::result(
                ok.as_deref().map(|x| x.into()),
                error.as_deref().map(|x| x.into()),
            ),
            TypeName::Record(fields) => InferredType::record(
                fields
                    .iter()
                    .map(|(field, typ)| (field.clone(), typ.deref().into()))
                    .collect(),
            ),
            TypeName::Flags(flags) => InferredType::flags(flags.clone()),
            TypeName::Enum(cases) => InferredType::enum_(cases.clone()),
            TypeName::Variant { cases } => InferredType::from_variant_cases(
                cases
                    .iter()
                    .map(|(case_name, typ)| (case_name.clone(), typ.as_deref().map(|x| x.into())))
                    .collect(),
            ),
        }
    }
}

impl TryFrom<InferredType> for TypeName {
    type Error = String;

    fn try_from(value: InferredType) -> Result<Self, Self::Error> {
        match value.inner.deref() {
            TypeInternal::Bool => Ok(TypeName::Bool),
            TypeInternal::S8 => Ok(TypeName::S8),
            TypeInternal::U8 => Ok(TypeName::U8),
            TypeInternal::S16 => Ok(TypeName::S16),
            TypeInternal::U16 => Ok(TypeName::U16),
            TypeInternal::S32 => Ok(TypeName::S32),
            TypeInternal::U32 => Ok(TypeName::U32),
            TypeInternal::S64 => Ok(TypeName::S64),
            TypeInternal::U64 => Ok(TypeName::U64),
            TypeInternal::F32 => Ok(TypeName::F32),
            TypeInternal::F64 => Ok(TypeName::F64),
            TypeInternal::Chr => Ok(TypeName::Chr),
            TypeInternal::Str => Ok(TypeName::Str),
            TypeInternal::List(inferred_type) => {
                let verified = inferred_type.clone().try_into()?;
                Ok(TypeName::List(Box::new(verified)))
            }
            TypeInternal::Tuple(inferred_types) => {
                let mut verified_types = vec![];
                for typ in inferred_types {
                    let verified = typ.clone().try_into()?;
                    verified_types.push(verified);
                }
                Ok(TypeName::Tuple(verified_types))
            }
            TypeInternal::Record(name_and_types) => {
                let mut fields = vec![];
                for (field, typ) in name_and_types {
                    fields.push((field.clone(), Box::new(typ.clone().try_into()?)));
                }
                Ok(TypeName::Record(fields))
            }
            TypeInternal::Flags(flags) => Ok(TypeName::Flags(flags.clone())),
            TypeInternal::Enum(enums) => Ok(TypeName::Enum(enums.clone())),
            TypeInternal::Option(inferred_type) => {
                let result = inferred_type.clone().try_into()?;
                Ok(TypeName::Option(Box::new(result)))
            }
            TypeInternal::Result { ok, error } => {
                let ok_unified = ok.as_ref().map(|ok| ok.clone().try_into()).transpose()?;
                let err_unified = error
                    .as_ref()
                    .map(|err| err.clone().try_into())
                    .transpose()?;
                Ok(TypeName::Result {
                    ok: ok_unified.map(Box::new),
                    error: err_unified.map(Box::new),
                })
            }
            TypeInternal::Variant(variant) => {
                let mut cases = vec![];
                for (case, typ) in variant {
                    let verified = typ.clone().map(TypeName::try_from).transpose()?;
                    cases.push((case.clone(), verified.map(Box::new)));
                }
                Ok(TypeName::Variant { cases })
            }
            TypeInternal::Resource { .. } => {
                Err("Cannot convert a resource type to a type name".to_string())
            }
            TypeInternal::AllOf(_) => {
                Err("Cannot convert a all of type to a type name".to_string())
            }
            TypeInternal::Unknown => {
                Err("Cannot convert an unknown type to a type name".to_string())
            }
            TypeInternal::Sequence(_) => {
                Err("Cannot convert a sequence type to a type name".to_string())
            }
            TypeInternal::Instance { .. } => {
                Err("Cannot convert an instance type to a type name".to_string())
            }
            TypeInternal::Range { .. } => {
                Err("Cannot convert a range type to a type name".to_string())
            }
        }
    }
}

pub fn parse_basic_type<Input>() -> impl Parser<Input, Output = TypeName>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    choice((
        attempt(string("bool").map(|_| TypeName::Bool)),
        attempt(string("s8").map(|_| TypeName::S8)),
        attempt(string("u8").map(|_| TypeName::U8)),
        attempt(string("s16").map(|_| TypeName::S16)),
        attempt(string("u16").map(|_| TypeName::U16)),
        attempt(string("s32").map(|_| TypeName::S32)),
        attempt(string("u32").map(|_| TypeName::U32)),
        attempt(string("s64").map(|_| TypeName::S64)),
        attempt(string("u64").map(|_| TypeName::U64)),
        attempt(string("f32").map(|_| TypeName::F32)),
        attempt(string("f64").map(|_| TypeName::F64)),
        attempt(string("char").map(|_| TypeName::Chr)),
        attempt(string("string").map(|_| TypeName::Str)),
    ))
    .skip(spaces().silent())
}

pub fn parse_list_type<Input>() -> impl Parser<Input, Output = TypeName>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    string("list")
        .skip(spaces().silent())
        .with(between(
            char('<').skip(spaces().silent().silent()),
            char('>').skip(spaces().silent().silent()),
            type_name(),
        ))
        .map(|inner_type| TypeName::List(Box::new(inner_type)))
}

pub fn parse_option_type<Input>() -> impl Parser<Input, Output = TypeName>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    string("option")
        .skip(spaces().silent())
        .with(between(
            char('<').skip(spaces().silent().silent()),
            char('>').skip(spaces().silent().silent()),
            type_name(),
        ))
        .map(|inner_type| TypeName::Option(Box::new(inner_type)))
}

enum ResultSuccess {
    NoType,
    WithType(TypeName),
}

// https://component-model.bytecodealliance.org/design/wit.html#results
pub fn parse_result_type<Input>() -> impl Parser<Input, Output = TypeName>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    string("result")
        .skip(spaces().silent())
        .with(optional(between(
            char('<').skip(spaces().silent()),
            char('>').skip(spaces().silent()),
            (
                choice!(
                    string("_")
                        .skip(spaces().silent())
                        .map(|_| ResultSuccess::NoType),
                    type_name()
                        .skip(spaces().silent())
                        .map(ResultSuccess::WithType)
                ),
                optional(
                    char(',')
                        .skip(spaces().silent())
                        .with(type_name().skip(spaces().silent())),
                ),
            ),
        )))
        .map(|result| match result {
            None => TypeName::Result {
                ok: None,
                error: None,
            },
            Some((ResultSuccess::NoType, None)) => TypeName::Result {
                ok: None,
                error: None,
            },
            Some((ResultSuccess::NoType, Some(error))) => TypeName::Result {
                ok: None,
                error: Some(Box::new(error)),
            },
            Some((ResultSuccess::WithType(ok), None)) => TypeName::Result {
                ok: Some(Box::new(ok)),
                error: None,
            },
            Some((ResultSuccess::WithType(ok), Some(error))) => TypeName::Result {
                ok: Some(Box::new(ok)),
                error: Some(Box::new(error)),
            },
        })
}

pub fn parse_tuple_type<Input>() -> impl Parser<Input, Output = TypeName>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    string("tuple")
        .skip(spaces().silent())
        .with(between(
            char('<').skip(spaces().silent()),
            char('>').skip(spaces().silent()),
            sep_by(type_name(), char(',').skip(spaces().silent())),
        ))
        .map(TypeName::Tuple)
}

pub fn type_name_<Input>() -> impl Parser<Input, Output = TypeName>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    spaces().silent().with(choice((
        attempt(parse_basic_type()),
        attempt(parse_list_type()),
        attempt(parse_tuple_type()),
        attempt(parse_option_type()),
        attempt(parse_result_type()),
    )))
}

parser! {
    pub fn type_name[Input]()(Input) -> TypeName
     where [Input: combine::Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>, Input::Position: GetSourcePosition]
    {
       type_name_()
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use golem_api_grpc::proto::golem::rib::type_name::Kind as InnerTypeName;
    use golem_api_grpc::proto::golem::rib::{
        BasicTypeName, EnumType, FlagType, KeyValue, ListType, OptionType, RecordType, ResultType,
        TupleType, TypeName as ProtoTypeName, VariantCase, VariantType,
    };
    use std::ops::Deref;

    use crate::TypeName;

    impl From<TypeName> for ProtoTypeName {
        fn from(value: TypeName) -> Self {
            let inner = match value {
                TypeName::Bool => InnerTypeName::BasicType(BasicTypeName::Bool as i32),
                TypeName::S8 => InnerTypeName::BasicType(BasicTypeName::S8 as i32),
                TypeName::U8 => InnerTypeName::BasicType(BasicTypeName::U8 as i32),
                TypeName::S16 => InnerTypeName::BasicType(BasicTypeName::S16 as i32),
                TypeName::U16 => InnerTypeName::BasicType(BasicTypeName::U16 as i32),
                TypeName::S32 => InnerTypeName::BasicType(BasicTypeName::S32 as i32),
                TypeName::U32 => InnerTypeName::BasicType(BasicTypeName::U32 as i32),
                TypeName::S64 => InnerTypeName::BasicType(BasicTypeName::S64 as i32),
                TypeName::U64 => InnerTypeName::BasicType(BasicTypeName::U64 as i32),
                TypeName::F32 => InnerTypeName::BasicType(BasicTypeName::F32 as i32),
                TypeName::F64 => InnerTypeName::BasicType(BasicTypeName::F64 as i32),
                TypeName::Chr => InnerTypeName::BasicType(BasicTypeName::Chr as i32),
                TypeName::Str => InnerTypeName::BasicType(BasicTypeName::Str as i32),
                TypeName::List(inner_type) => InnerTypeName::ListType(Box::new(ListType {
                    inner_type: Some(Box::new(inner_type.deref().clone().into())),
                })),
                TypeName::Tuple(inner_types) => InnerTypeName::TupleType(TupleType {
                    types: inner_types.into_iter().map(|t| t.into()).collect(),
                }),
                TypeName::Option(type_name) => InnerTypeName::OptionType(Box::new(OptionType {
                    inner_type: Some(Box::new(type_name.deref().clone().into())),
                })),
                TypeName::Result { ok, error } => InnerTypeName::ResultType(Box::new(ResultType {
                    ok_type: ok.map(|ok| Box::new(ok.deref().clone().into())),
                    err_type: error.map(|error| Box::new(error.deref().clone().into())),
                })),
                TypeName::Record(fields) => InnerTypeName::RecordType(RecordType {
                    fields: fields
                        .into_iter()
                        .map(|(field, typ)| KeyValue {
                            key: field,
                            value: Some(typ.deref().clone().into()),
                        })
                        .collect(),
                }),
                TypeName::Flags(flags) => InnerTypeName::FlagType(FlagType {
                    flags: flags.into_iter().collect(),
                }),
                TypeName::Enum(cases) => InnerTypeName::EnumType(EnumType {
                    cases: cases.into_iter().collect(),
                }),
                TypeName::Variant { cases } => InnerTypeName::VariantType(VariantType {
                    cases: cases
                        .into_iter()
                        .map(|(case, typ)| VariantCase {
                            case_name: case,
                            variant_arg: typ.map(|x| x.deref().clone().into()),
                        })
                        .collect(),
                }),
            };

            ProtoTypeName { kind: Some(inner) }
        }
    }

    impl TryFrom<ProtoTypeName> for TypeName {
        type Error = String;

        fn try_from(value: ProtoTypeName) -> Result<Self, Self::Error> {
            match value.kind {
                Some(inner) => match inner {
                    InnerTypeName::BasicType(value) => match BasicTypeName::try_from(value) {
                        Ok(BasicTypeName::Bool) => Ok(TypeName::Bool),
                        Ok(BasicTypeName::S8) => Ok(TypeName::S8),
                        Ok(BasicTypeName::U8) => Ok(TypeName::U8),
                        Ok(BasicTypeName::S16) => Ok(TypeName::S16),
                        Ok(BasicTypeName::U16) => Ok(TypeName::U16),
                        Ok(BasicTypeName::S32) => Ok(TypeName::S32),
                        Ok(BasicTypeName::U32) => Ok(TypeName::U32),
                        Ok(BasicTypeName::S64) => Ok(TypeName::S64),
                        Ok(BasicTypeName::U64) => Ok(TypeName::U64),
                        Ok(BasicTypeName::F32) => Ok(TypeName::F32),
                        Ok(BasicTypeName::F64) => Ok(TypeName::F64),
                        Ok(BasicTypeName::Chr) => Ok(TypeName::Chr),
                        Ok(BasicTypeName::Str) => Ok(TypeName::Str),
                        _ => Err(format!("Unknown basic type: {:?}", value)),
                    },
                    InnerTypeName::ListType(inner_type) => {
                        let proto_list_type = inner_type
                            .inner_type
                            .ok_or("No inner type for list provided")?;
                        let list_type = proto_list_type.deref().clone().try_into()?;
                        Ok(TypeName::List(Box::new(list_type)))
                    }
                    InnerTypeName::TupleType(inner_types) => {
                        let tuple_type = inner_types
                            .types
                            .into_iter()
                            .map(|t| t.try_into())
                            .collect::<Result<Vec<TypeName>, String>>()?;
                        Ok(TypeName::Tuple(tuple_type))
                    }
                    InnerTypeName::OptionType(type_name) => {
                        let proto_option_type = type_name
                            .inner_type
                            .ok_or("No inner type for option provided")?;
                        let option_type = proto_option_type.deref().clone().try_into()?;
                        Ok(TypeName::Option(Box::new(option_type)))
                    }
                    InnerTypeName::ResultType(result_type) => {
                        let ok = result_type
                            .ok_type
                            .map(|ok| ok.deref().clone().try_into())
                            .transpose()?;
                        let error = result_type
                            .err_type
                            .map(|error| error.deref().clone().try_into())
                            .transpose()?;
                        Ok(TypeName::Result {
                            ok: ok.map(Box::new),
                            error: error.map(Box::new),
                        })
                    }
                    InnerTypeName::RecordType(fields) => {
                        let record_type = fields
                            .fields
                            .into_iter()
                            .map(|key_value| {
                                key_value
                                    .value
                                    .ok_or("Field type missing")?
                                    .try_into()
                                    .map(|typ| (key_value.key, Box::new(typ)))
                            })
                            .collect::<Result<Vec<(String, Box<TypeName>)>, String>>()?;
                        Ok(TypeName::Record(record_type))
                    }
                    InnerTypeName::FlagType(flag_type) => Ok(TypeName::Flags(flag_type.flags)),
                    InnerTypeName::EnumType(enum_type) => Ok(TypeName::Enum(enum_type.cases)),
                    InnerTypeName::VariantType(variant_type) => {
                        let mut cases = vec![];
                        for variant_case in variant_type.cases {
                            let case = variant_case.case_name;
                            let typ = match variant_case.variant_arg {
                                Some(typ) => Some(Box::new(TypeName::try_from(typ)?)),
                                None => None,
                            };
                            cases.push((case, typ));
                        }

                        Ok(TypeName::Variant { cases })
                    }
                },
                None => Err("No type kind provided".to_string()),
            }
        }
    }
}

#[cfg(test)]
mod type_name_tests {
    use combine::stream::position;
    use combine::EasyParser;
    use test_r::test;

    use super::*;

    fn parse_and_compare(input: &str, expected: TypeName) {
        let written = format!("{}", expected);
        let result1 = type_name()
            .easy_parse(position::Stream::new(input))
            .map(|x| x.0);
        let result2 = type_name()
            .easy_parse(position::Stream::new(written.as_str()))
            .map(|x| x.0);
        assert_eq!(result1, Ok(expected.clone()));
        assert_eq!(result2, Ok(expected));
    }

    #[test]
    fn test_basic_types() {
        parse_and_compare("bool", TypeName::Bool);
        parse_and_compare("s8", TypeName::S8);
        parse_and_compare("u8", TypeName::U8);
        parse_and_compare("s16", TypeName::S16);
        parse_and_compare("u16", TypeName::U16);
        parse_and_compare("s32", TypeName::S32);
        parse_and_compare("u32", TypeName::U32);
        parse_and_compare("s64", TypeName::S64);
        parse_and_compare("u64", TypeName::U64);
        parse_and_compare("f32", TypeName::F32);
        parse_and_compare("f64", TypeName::F64);
        parse_and_compare("char", TypeName::Chr);
        parse_and_compare("string", TypeName::Str);
    }

    #[test]
    fn test_list_type_name() {
        parse_and_compare("list<u8>", TypeName::List(Box::new(TypeName::U8)));
        parse_and_compare(
            "list<list<f32>>",
            TypeName::List(Box::new(TypeName::List(Box::new(TypeName::F32)))),
        );
    }

    #[test]
    fn test_tuple_type_name() {
        parse_and_compare(
            "tuple<u8, u16>",
            TypeName::Tuple(vec![TypeName::U8, TypeName::U16]),
        );
        parse_and_compare(
            "tuple<s32, list<u8>>",
            TypeName::Tuple(vec![TypeName::S32, TypeName::List(Box::new(TypeName::U8))]),
        );
        parse_and_compare(
            "tuple<tuple<s8, s16>, u32>",
            TypeName::Tuple(vec![
                TypeName::Tuple(vec![TypeName::S8, TypeName::S16]),
                TypeName::U32,
            ]),
        );
    }

    #[test]
    fn test_option_type_name() {
        parse_and_compare("option<u8>", TypeName::Option(Box::new(TypeName::U8)));
        parse_and_compare(
            "option<list<f32>>",
            TypeName::Option(Box::new(TypeName::List(Box::new(TypeName::F32)))),
        );
    }

    #[test]
    fn test_nested_types() {
        parse_and_compare(
            "list<tuple<u8, s8>>",
            TypeName::List(Box::new(TypeName::Tuple(vec![TypeName::U8, TypeName::S8]))),
        );
        parse_and_compare(
            "tuple<list<u16>, list<f64>>",
            TypeName::Tuple(vec![
                TypeName::List(Box::new(TypeName::U16)),
                TypeName::List(Box::new(TypeName::F64)),
            ]),
        );
    }

    #[test]
    fn test_spaces_around_types() {
        parse_and_compare("  u8  ", TypeName::U8);
        parse_and_compare("list< u8 >", TypeName::List(Box::new(TypeName::U8)));
        parse_and_compare(
            "tuple< s32 , list< u8 > >",
            TypeName::Tuple(vec![TypeName::S32, TypeName::List(Box::new(TypeName::U8))]),
        );
    }
}
