use crate::InferredType;
use bincode::{Decode, Encode};
use combine::parser;
use combine::parser::char;
use combine::parser::char::{char, spaces, string};
use combine::parser::choice::choice;
use combine::{attempt, between, easy, sep_by, Parser, Stream};
use golem_api_grpc::proto::golem::rib::type_name::Kind as InnerTypeName;
use golem_api_grpc::proto::golem::rib::{
    BasicTypeName, ListType, OptionType, TupleType, TypeName as ProtoTypeName,
};
use std::fmt::Display;
use std::ops::Deref;

#[derive(Debug, Hash, Clone, Eq, PartialEq, Encode, Decode)]
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
            TypeName::Chr => write!(f, "chr"),
            TypeName::Str => write!(f, "str"),
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
        }
    }
}

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
            },
            None => Err("No type kind provided".to_string()),
        }
    }
}

impl From<TypeName> for InferredType {
    fn from(type_name: TypeName) -> Self {
        match type_name {
            TypeName::Bool => InferredType::Bool,
            TypeName::S8 => InferredType::S8,
            TypeName::U8 => InferredType::U8,
            TypeName::S16 => InferredType::S16,
            TypeName::U16 => InferredType::U16,
            TypeName::S32 => InferredType::S32,
            TypeName::U32 => InferredType::U32,
            TypeName::S64 => InferredType::S64,
            TypeName::U64 => InferredType::U64,
            TypeName::F32 => InferredType::F32,
            TypeName::F64 => InferredType::F64,
            TypeName::Chr => InferredType::Chr,
            TypeName::Str => InferredType::Str,
            TypeName::List(inner_type) => {
                InferredType::List(Box::new(inner_type.deref().clone().into()))
            }
            TypeName::Tuple(inner_types) => {
                InferredType::Tuple(inner_types.into_iter().map(|t| t.into()).collect())
            }
            TypeName::Option(type_name) => {
                InferredType::Option(Box::new(type_name.deref().clone().into()))
            }
        }
    }
}

pub fn parse_basic_type<'t>() -> impl Parser<easy::Stream<&'t str>, Output = TypeName> {
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
        attempt(string("chr").map(|_| TypeName::Chr)),
        attempt(string("str").map(|_| TypeName::Str)),
    ))
    .skip(spaces())
}

pub fn parse_list_type<'t>() -> impl Parser<easy::Stream<&'t str>, Output = TypeName> {
    string("list")
        .skip(spaces())
        .with(between(
            char('<').skip(spaces()),
            char('>').skip(spaces()),
            parse_type_name(),
        ))
        .map(|inner_type| TypeName::List(Box::new(inner_type)))
}

pub fn parse_option_type<'t>() -> impl Parser<easy::Stream<&'t str>, Output = TypeName> {
    string("option")
        .skip(spaces())
        .with(between(
            char('<').skip(spaces()),
            char('>').skip(spaces()),
            parse_type_name(),
        ))
        .map(|inner_type| TypeName::Option(Box::new(inner_type)))
}

pub fn parse_tuple_type<'t>() -> impl Parser<easy::Stream<&'t str>, Output = TypeName> {
    string("tuple")
        .skip(spaces())
        .with(between(
            char('<').skip(spaces()),
            char('>').skip(spaces()),
            sep_by(parse_type_name(), char(',').skip(spaces())),
        ))
        .map(TypeName::Tuple)
}

pub fn parse_type_name_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = TypeName> {
    spaces().with(choice((
        attempt(parse_basic_type()),
        attempt(parse_list_type()),
        attempt(parse_tuple_type()),
        attempt(parse_option_type()),
    )))
}

parser! {
    pub fn parse_type_name['t]()(easy::Stream<&'t str>) -> TypeName
    where [
        easy::Stream<&'t str>: Stream<Token = char>,
    ]
    {
       parse_type_name_()
    }
}

#[cfg(test)]
mod type_name_parser_tests {
    use super::*;
    use combine::EasyParser;

    fn parse_and_compare(input: &str, expected: TypeName) {
        let result = parse_type_name().easy_parse(input);
        assert_eq!(result, Ok((expected, "")));
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
        parse_and_compare("chr", TypeName::Chr);
        parse_and_compare("str", TypeName::Str);
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
