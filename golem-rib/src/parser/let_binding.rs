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

use combine::error::StreamError;
use combine::parser::char::digit;
use combine::{
    many1, optional,
    parser::char::{char as char_, letter, spaces, string},
    Parser,
};

use crate::expr::Expr;
use crate::parser::rib_expr::rib_expr;
use crate::parser::type_name::parse_type_name;
use combine::stream::easy;

pub fn let_binding<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        (
            string("let").skip(spaces()),
            let_variable().skip(spaces()),
            optional(
                // Optionally match and parse the type annotation
                char_(':')
                    .skip(spaces()) // Match the colon
                    .with(parse_type_name()) // Parse the type
                    .skip(spaces()), // Consume any trailing spaces
            ),
            char_('=').skip(spaces()),
            rib_expr(),
        )
            .map(|(_, var, optional_type, _, expr)| {
                let new_expr = type_binding::bind(&expr, optional_type);
                Expr::let_binding(var.as_str(), new_expr)
            }),
    )
}

fn let_variable<'t>() -> impl Parser<easy::Stream<&'t str>, Output = String> {
    many1(letter().or(digit()).or(char_('_')))
        .and_then(|s: Vec<char>| {
            if s.first().map_or(false, |&c| c.is_alphabetic()) {
                Ok(s)
            } else {
                Err(easy::Error::message_static_message(
                    "Let binding variable must start with a letter",
                ))
            }
        })
        .map(|s: Vec<char>| s.into_iter().collect())
        .message("Unable to parse let binding variable")
}

// TODO; Revisit and clean up
mod type_binding {
    use crate::parser::type_name::TypeName;
    use crate::{Expr, InferredType};

    pub(crate) fn bind(expr: &Expr, type_name: Option<TypeName>) -> Expr {
        if let Some(type_name) = type_name {
            let mut expr = expr.clone();
            override_type(&mut expr, type_name.into());
            expr
        } else {
            expr.clone()
        }
    }

    pub(crate) fn override_type(expr: &mut Expr, new_type: InferredType) {
        match expr {
            Expr::Identifier(_, inferred_type)
            | Expr::Let(_, _, inferred_type)
            | Expr::SelectField(_, _, inferred_type)
            | Expr::SelectIndex(_, _, inferred_type)
            | Expr::Sequence(_, inferred_type)
            | Expr::Record(_, inferred_type)
            | Expr::Tuple(_, inferred_type)
            | Expr::Literal(_, inferred_type)
            | Expr::Number(_, inferred_type)
            | Expr::Flags(_, inferred_type)
            | Expr::Boolean(_, inferred_type)
            | Expr::Concat(_, inferred_type)
            | Expr::Multiple(_, inferred_type)
            | Expr::Not(_, inferred_type)
            | Expr::GreaterThan(_, _, inferred_type)
            | Expr::GreaterThanOrEqualTo(_, _, inferred_type)
            | Expr::LessThanOrEqualTo(_, _, inferred_type)
            | Expr::EqualTo(_, _, inferred_type)
            | Expr::LessThan(_, _, inferred_type)
            | Expr::Cond(_, _, _, inferred_type)
            | Expr::PatternMatch(_, _, inferred_type)
            | Expr::Option(_, inferred_type)
            | Expr::Result(_, inferred_type)
            | Expr::Unwrap(_, inferred_type)
            | Expr::Throw(_, inferred_type)
            | Expr::Tag(_, inferred_type)
            | Expr::Call(_, _, inferred_type) => {
                *inferred_type = new_type;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InferredType, VariableId};
    use combine::EasyParser;

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
        let result = let_binding().easy_parse(input);
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
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::let_binding("foo", Expr::literal("bar")), ""))
        );
    }

    #[test]
    fn test_let_binding_with_record() {
        let input = "let foo = { bar : baz }";
        let result = let_binding().easy_parse(input);
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
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::U8)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u16() {
        let input = "let foo: u16 = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::U16)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u32() {
        let input = "let foo: u32 = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::U32)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u64() {
        let input = "let foo: u64 = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::U64)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s8() {
        let input = "let foo: s8 = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::S8)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s16() {
        let input = "let foo: s16 = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::S16)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s32() {
        let input = "let foo: s32 = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::S32)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s64() {
        let input = "let foo: s64 = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::S64)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_f32() {
        let input = "let foo: f32 = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::F32)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_f64() {
        let input = "let foo: f64 = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::F64)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_chr() {
        let input = "let foo: chr = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Chr)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_str() {
        let input = "let foo: str = bar";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Identifier(VariableId::global("bar".to_string()), InferredType::Str)
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_list_u8() {
        let input = "let foo: list<u8> = []";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::let_binding(
                    "foo",
                    Expr::Sequence(vec![], InferredType::List(Box::new(InferredType::U8)))
                ),
                ""
            ))
        );
    }
}
