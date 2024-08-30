use crate::InferredType;
use bincode::{Decode, Encode};
use combine::parser;
use combine::parser::char;
use combine::parser::char::{char, spaces, string};
use combine::parser::choice::choice;
use combine::{attempt, between, easy, sep_by, Parser, Stream};
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
                InferredType::List(Box::new((inner_type.deref().clone().into())))
            }
            TypeName::Tuple(inner_types) => {
                InferredType::Tuple(inner_types.into_iter().map(|t| t.into()).collect())
            }
        }
    }
}

pub fn parse_basic_type<'t>() -> impl Parser<easy::Stream<&'t str>, Output = TypeName> {
    spaces()
        .with(choice((
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
        )))
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
    fn test_tuple_type() {
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
