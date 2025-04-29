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

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::identifier::identifier_text;
use crate::parser::rib_expr::rib_expr;
use crate::parser::type_name::type_name;
use crate::rib_source_span::GetSourcePosition;
use combine::parser::char::{alpha_num, char};
use combine::{
    attempt, not_followed_by, optional,
    parser::char::{char as char_, spaces, string},
    ParseError, Parser,
};

pub fn let_binding<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    attempt(
        string("let").skip(not_followed_by(alpha_num().or(char('-')).or(char('_'))).skip(spaces())),
    )
    .with(
        (
            let_variable().skip(spaces()),
            optional(char_(':').skip(spaces()).with(type_name()).skip(spaces())),
            char_('=').skip(spaces()),
            rib_expr(),
        )
            .map(|(var, optional_type, _, expr)| Expr::let_binding(var, expr, optional_type)),
    )
}

fn let_variable<Input>() -> impl Parser<Input, Output = String>
where
    Input: combine::Stream<Token = char>,
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    identifier_text().message("Unable to parse binding variable")
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::parser::type_name::TypeName;
    use crate::{InferredType, VariableId};

    use super::*;

    #[test]
    fn test_let_binding() {
        let input = "let foo = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_global("bar", None),
                None
            ))
        );
    }

    #[test]
    fn test_let_binding_with_sequence() {
        let input = "let foo = [bar, baz]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::sequence(
                    vec![
                        Expr::identifier_global("bar", None),
                        Expr::identifier_global("baz", None)
                    ],
                    None
                ),
                None
            ))
        );
    }

    #[test]
    fn test_let_binding_with_binary_comparisons() {
        let input = "let foo = bar == baz";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::equal_to(
                    Expr::identifier_global("bar", None),
                    Expr::identifier_global("baz", None)
                ),
                None
            ))
        );
    }

    #[test]
    fn test_let_binding_with_option() {
        let input = "let foo = some(bar)";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::option(Some(Expr::identifier_global("bar", None))),
                None
            ))
        );
    }

    #[test]
    fn test_let_binding_with_result() {
        let input = "let foo = ok(bar)";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::ok(Expr::identifier_global("bar", None), None),
                None
            ))
        );
    }

    #[test]
    fn test_let_binding_with_literal() {
        let input = "let foo = \"bar\"";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding("foo", Expr::literal("bar"), None))
        );
    }

    #[test]
    fn test_let_binding_with_record() {
        let input = "let foo = { bar : baz }";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::record(vec![(
                    "bar".to_string(),
                    Expr::identifier_global("baz", None)
                )]),
                None
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u8() {
        let input = "let foo: u8 = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::U8)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u16() {
        let input = "let foo: u16 = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::U16)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u32() {
        let input = "let foo: u32 = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::U32)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_u64() {
        let input = "let foo: u64 = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::U64)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s8() {
        let input = "let foo: s8 = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::S8,)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s16() {
        let input = "let foo: s16 = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::S16)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s32() {
        let input = "let foo: s32 = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::S32)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_s64() {
        let input = "let foo: s64 = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::S64,)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_f32() {
        let input = "let foo: f32 = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::F32)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_f64() {
        let input = "let foo: f64 = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::F64)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_chr() {
        let input = "let foo: char = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::Chr)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_str() {
        let input = "let foo: string = bar";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::identifier_with_variable_id(VariableId::global("bar".to_string()), None,),
                Some(TypeName::Str)
            ))
        );
    }

    #[test]
    fn test_let_binding_with_type_name_list_u8() {
        let input = "let foo: list<u8> = []";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::let_binding(
                "foo",
                Expr::sequence(vec![], None)
                    .with_inferred_type(InferredType::list(InferredType::unknown())),
                Some(TypeName::List(Box::new(TypeName::U8)))
            ))
        );
    }
}
