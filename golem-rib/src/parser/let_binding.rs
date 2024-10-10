// Copyright 2024 Golem Cloud
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

use combine::parser::char::{alpha_num, char};
use combine::{
    attempt, not_followed_by, optional,
    parser::char::{char as char_, spaces, string},
    ParseError, Parser,
};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::identifier::identifier_text;
use crate::parser::rib_expr::rib_expr;
use crate::parser::type_name::parse_type_name;

pub fn let_binding<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    attempt(
        string("let").skip(not_followed_by(alpha_num().or(char('-')).or(char('_'))).skip(spaces())),
    )
    .with(
        (
            let_variable().skip(spaces()),
            optional(
                char_(':')
                    .skip(spaces())
                    .with(parse_type_name())
                    .skip(spaces()),
            ),
            char_('=').skip(spaces()),
            rib_expr(),
        )
            .map(|(var, optional_type, _, expr)| {
                if let Some(type_name) = optional_type {
                    Expr::let_binding_with_type(var, type_name, expr)
                } else {
                    Expr::let_binding(var.as_str(), expr)
                }
            }),
    )
}

fn let_variable<Input>() -> impl Parser<Input, Output = String>
where
    Input: combine::Stream<Token = char>,
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    identifier_text().message("Unable to parse binding variable")
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use combine::EasyParser;

    use crate::parser::type_name::TypeName;
    use crate::{InferredType, VariableId};

    use super::*;

    #[test]
    fn test_let_binding() {
        let input = "let foo = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::let_binding("foo", Expr::identifier("bar")), ""))
        );
    }

    #[test]
    fn test_let_binding_with_sequence() {
        let input = "let foo = [bar, baz]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::sequence(vec![Expr::identifier("bar"), Expr::identifier("baz")])
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_binary_comparisons() {
        let input = "let foo = bar == baz";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::equal_to(Expr::identifier("bar"), Expr::identifier("baz"))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_option() {
        let input = "let foo = some(bar)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding("foo", Expr::option(Some(Expr::identifier("bar")))),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_result() {
        let input = "let foo = ok(bar)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding("foo", Expr::ok(Expr::identifier("bar"))),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_literal() {
        let input = "let foo = \"bar\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::let_binding("foo", Expr::literal("bar")), ""))
        );
    }

    #[test]
    fn test_let_binding_with_record() {
        let input = "let foo = { bar : baz }";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::record(vec![("bar".to_string(), Expr::identifier("baz"))])
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u8() {
        let input = "let foo: u8 = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::U8,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u16() {
        let input = "let foo: u16 = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::U16,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u32() {
        let input = "let foo: u32 = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::U32,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u64() {
        let input = "let foo: u64 = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::U64,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s8() {
        let input = "let foo: s8 = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::S8,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s16() {
        let input = "let foo: s16 = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::S16,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s32() {
        let input = "let foo: s32 = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::S32,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s64() {
        let input = "let foo: s64 = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::S64,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_f32() {
        let input = "let foo: f32 = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::F32,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_f64() {
        let input = "let foo: f64 = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::F64,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_chr() {
        let input = "let foo: chr = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::Chr,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_str() {
        let input = "let foo: str = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::Str,
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Unknown)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_list_u8() {
        let input = "let foo: list<u8> = []";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding_with_type(
                    "foo",
                    TypeName::List(Box::new(TypeName::U8)),
                    Expr::Sequence(vec![], InferredType::List(Box::new(InferredType::Unknown)))
                ),
                ""
            ))
        );
    }
}
