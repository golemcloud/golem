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

use combine::parser::char;
use combine::parser::char::{char, spaces};
use combine::{eof, ParseError, Parser};
use combine::{parser, sep_by};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;

use super::binary_comparison::BinaryOp;

// Parse a full Rib Program, and we expect the parser to fully consume the stream
// unlike rib block expression
pub fn rib_program<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    spaces().with(
        sep_by(rib_expr().skip(spaces()), char(';').skip(spaces()))
            .map(|expressions: Vec<Expr>| {
                if expressions.len() == 1 {
                    expressions.first().unwrap().clone()
                } else {
                    Expr::multiple(expressions)
                }
            })
            .skip(eof()),
    )
}

// A rib expression := (simple_expr, rib_expr_rest*)
parser! {
    pub fn rib_expr[Input]()(Input) -> Expr
    where [Input: combine::Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,]
    {
       rib_expr_()
    }
}

pub fn rib_expr_<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    spaces()
        .with(
            (internal::simple_expr(), internal::rib_expr_rest()).map(|(expr, rest)| {
                // FIXME: Respect operator precedence
                rest.into_iter().fold(expr, |acc, (op, next)| match op {
                    BinaryOp::GreaterThan => Expr::greater_than(acc, next),
                    BinaryOp::LessThan => Expr::less_than(acc, next),
                    BinaryOp::LessThanOrEqualTo => Expr::less_than_or_equal_to(acc, next),
                    BinaryOp::GreaterThanOrEqualTo => Expr::greater_than_or_equal_to(acc, next),
                    BinaryOp::EqualTo => Expr::equal_to(acc, next),
                })
            }),
        )
        .skip(spaces())
}

mod internal {
    use crate::parser::binary_comparison::{binary_op, BinaryOp};
    use crate::parser::boolean::boolean_literal;
    use crate::parser::call::call;
    use crate::parser::cond::conditional;
    use crate::parser::errors::RibParseError;
    use crate::parser::flag::flag;
    use crate::parser::identifier::identifier;
    use crate::parser::let_binding::let_binding;
    use crate::parser::literal::literal;
    use crate::parser::multi_line_code_block::multi_line_block;
    use crate::parser::not::not;
    use crate::parser::number::number;
    use crate::parser::optional::option;
    use crate::parser::pattern_match::pattern_match;
    use crate::parser::record::record;
    use crate::parser::result::result;

    use crate::parser::select_field::select_field;
    use crate::parser::select_index::select_index;
    use crate::parser::sequence::sequence;
    use crate::parser::tuple::tuple;
    use crate::Expr;
    use combine::parser::char::spaces;
    use combine::{attempt, choice, many, parser, ParseError, Parser, Stream};

    // A simple expression is a composition of all parsers that doesn't involve left recursion
    pub fn simple_expr_<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        spaces()
            .with(choice((
                pattern_match(),
                let_binding(),
                conditional(),
                selection_expr(),
                flag_or_record(),
                multi_line_block(),
                tuple(),
                sequence(),
                boolean_literal(),
                literal(),
                not(),
                option(),
                result(),
                attempt(call()),
                identifier(),
                number(),
            )))
            .skip(spaces())
    }

    parser! {
        pub(crate) fn simple_expr[Input]()(Input) -> Expr
        where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,]
        {
            simple_expr_()
        }
    }

    pub fn rib_expr_rest_<Input>() -> impl Parser<Input, Output = Vec<(BinaryOp, Expr)>>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        many((binary_op(), simple_expr()))
    }

    parser! {
        pub(crate) fn rib_expr_rest[Input]()(Input) -> Vec<(BinaryOp, Expr)>
        where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,]
        {
            rib_expr_rest_()
        }
    }

    fn flag_or_record<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        choice((attempt(flag()), attempt(record()))).message("Unable to parse flag or record")
    }

    fn selection_expr<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        choice((attempt(select_field()), attempt(select_index())))
            .message("Unable to parse selection expression")
    }
}

#[cfg(test)]
mod tests {
    use combine::EasyParser;

    use crate::expr::ArmPattern;
    use crate::expr::MatchArm;
    use crate::function_name::ParsedFunctionSite::PackagedInterface;
    use crate::function_name::{DynamicParsedFunctionName, DynamicParsedFunctionReference};

    use super::*;

    fn program() -> String {
        r#"
         let x = 1;
         let y = 2;
         let result = x > y;
         let foo = some(result);
         let bar = ok(result);

         let baz = match foo {
           some(x) => x,
           none => false
         };

         let qux = match bar {
           ok(x) => x,
           err(msg) => false
         };

         let result = ns:name/interface.{[static]resource1.do-something-static}(baz, qux);

         result
       "#
        .to_string()
    }

    fn expected() -> Expr {
        Expr::multiple(vec![
            Expr::let_binding("x", Expr::number(1f64)),
            Expr::let_binding("y", Expr::number(2f64)),
            Expr::let_binding(
                "result",
                Expr::greater_than(Expr::identifier("x"), Expr::identifier("y")),
            ),
            Expr::let_binding("foo", Expr::option(Some(Expr::identifier("result")))),
            Expr::let_binding("bar", Expr::ok(Expr::identifier("result"))),
            Expr::let_binding(
                "baz",
                Expr::pattern_match(
                    Expr::identifier("foo"),
                    vec![
                        MatchArm::new(
                            ArmPattern::Literal(Box::new(Expr::option(Some(Expr::identifier(
                                "x",
                            ))))),
                            Expr::identifier("x"),
                        ),
                        MatchArm::new(
                            ArmPattern::Literal(Box::new(Expr::option(None))),
                            Expr::boolean(false),
                        ),
                    ],
                ),
            ),
            Expr::let_binding(
                "qux",
                Expr::pattern_match(
                    Expr::identifier("bar"),
                    vec![
                        MatchArm::new(
                            ArmPattern::Literal(Box::new(Expr::ok(Expr::identifier("x")))),
                            Expr::identifier("x"),
                        ),
                        MatchArm::new(
                            ArmPattern::Literal(Box::new(Expr::err(Expr::identifier("msg")))),
                            Expr::boolean(false),
                        ),
                    ],
                ),
            ),
            Expr::let_binding(
                "result",
                Expr::call(
                    DynamicParsedFunctionName {
                        site: PackagedInterface {
                            namespace: "ns".to_string(),
                            package: "name".to_string(),
                            interface: "interface".to_string(),
                            version: None,
                        },
                        function: DynamicParsedFunctionReference::RawResourceStaticMethod {
                            resource: "resource1".to_string(),
                            method: "do-something-static".to_string(),
                        },
                    },
                    vec![Expr::identifier("baz"), Expr::identifier("qux")],
                ),
            ),
            Expr::identifier("result"),
        ])
    }

    #[test]
    fn test_rib_expr() {
        let input = "let x = 1";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::let_binding("x", Expr::number(1f64)), "")));
    }

    #[test]
    fn test_rib_program() {
        let input = "let x = 1; let y = 2";
        let result = rib_program().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::multiple(vec![
                    Expr::let_binding("x", Expr::number(1f64)),
                    Expr::let_binding("y", Expr::number(2f64))
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_rib_program_multiline() {
        let input = "let x = 1;\nlet y = 2";
        let result = rib_program().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::multiple(vec![
                    Expr::let_binding("x", Expr::number(1f64)),
                    Expr::let_binding("y", Expr::number(2f64))
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_complex_rib_program() {
        let binding = program();
        let result = Expr::from_text(binding.as_ref());
        assert_eq!(result, Ok(expected()));
    }

    #[test]
    fn interpolated_program() {
        let program_interpolated = format!("\"${{{}}}\"", program());
        let result = rib_program().easy_parse(program_interpolated.as_str());
        assert_eq!(result, Ok((expected(), "")));
    }
}
