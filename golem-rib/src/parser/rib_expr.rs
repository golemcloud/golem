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

use crate::expr::Expr;
use crate::parser::identifier::identifier;
use crate::parser::literal::literal;
use crate::parser::not::not;
use crate::parser::sequence::sequence;
use combine::parser::char;
use combine::parser::char::{char, spaces};
use combine::parser::choice::choice;
use combine::{attempt, Parser, Stream};
use combine::{parser, sep_by};

use super::binary_comparison::{
    equal_to, greater_than, greater_than_or_equal_to, less_than, less_than_or_equal_to,
};
use super::cond::conditional;
use super::flag::flag;
use super::let_binding::let_binding;
use super::number::number;
use super::optional::option;
use super::pattern_match::pattern_match;
use super::record::record;
use super::result::result;
use super::select_field::select_field;
use super::select_index::select_index;
use super::tuple::tuple;
use crate::parser::boolean::boolean_literal;
use crate::parser::call::call;
use crate::parser::multi_line_code_block::multi_line_block;
use combine::stream::easy;

// Parse a full Rib Program.
// This is kept outside for a reason, to avoid the conditions that lead to stack over-flow
// Please don't refactor and inline this with `parser!` macros below.
pub fn rib_program<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    sep_by(rib_expr().skip(spaces()), char(';').skip(spaces())).map(|expressions: Vec<Expr>| {
        if expressions.len() == 1 {
            expressions.first().unwrap().clone()
        } else {
            Expr::multiple(expressions)
        }
    })
}

// To handle recursion based on docs
// Also note that, the immediate parsers on the sides of a binary expression can result in stack overflow
// Therefore we copy the parser without these binary parsers in the attempt list to build the binary comparison parsers.
// This may not be intuitive however will work!
parser! {
    pub fn rib_expr['t]()(easy::Stream<&'t str>) -> Expr
    where [
        easy::Stream<&'t str>: Stream<Token = char>,
    ]
    {
       rib_expr_()
    }
}

pub fn rib_expr_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    choice((
        attempt(pattern_match()),
        attempt(let_binding()),
        attempt(conditional()),
        attempt(greater_than_or_equal_to(
            comparison_operands(),
            comparison_operands(),
        )),
        attempt(greater_than(comparison_operands(), comparison_operands())),
        attempt(less_than_or_equal_to(
            comparison_operands(),
            comparison_operands(),
        )),
        attempt(less_than(comparison_operands(), comparison_operands())),
        attempt(equal_to(comparison_operands(), comparison_operands())),
        selection_expr(),
        attempt(flag()),
        attempt(record()),
        attempt(multi_line_block()),
        attempt(tuple()),
        attempt(sequence()),
        attempt(literal()),
        attempt(not()),
        attempt(option()),
        attempt(result()),
        attempt(call()),
        attempt(number()),
        attempt(boolean_literal()),
        attempt(identifier()),
    ))
}

fn selection_expr_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    choice((attempt(select_field()), attempt(select_index())))
}

parser! {
    fn selection_expr['t]()(easy::Stream<&'t str>) -> Expr
    where [
        easy::Stream<&'t str>: Stream<Token = char>,
    ]
    {
        selection_expr_()
    }
}

fn simple_expr_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    choice((
        attempt(literal()),
        attempt(not()),
        attempt(number()),
        attempt(boolean_literal()),
        attempt(identifier()),
    ))
}

parser! {
    fn simple_expr['t]()(easy::Stream<&'t str>) -> Expr
    where [
        easy::Stream<&'t str>: Stream<Token = char>,
    ]
    {
        simple_expr_()
    }
}

fn comparison_operands_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    selection_expr().or(simple_expr())
}

parser! {
    fn comparison_operands['t]()(easy::Stream<&'t str>) -> Expr
    where [
        easy::Stream<&'t str>: Stream<Token = char>,
    ]
    {
        comparison_operands_()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ArmPattern;
    use crate::expr::MatchArm;
    use crate::function_name::ParsedFunctionName;
    use crate::function_name::ParsedFunctionReference::RawResourceStaticMethod;
    use crate::function_name::ParsedFunctionSite::PackagedInterface;
    use combine::EasyParser;

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
                    ParsedFunctionName {
                        site: PackagedInterface {
                            namespace: "ns".to_string(),
                            package: "name".to_string(),
                            interface: "interface".to_string(),
                            version: None,
                        },
                        function: RawResourceStaticMethod {
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
        let result = rib_program().easy_parse(binding.as_ref());
        assert_eq!(result, Ok((expected(), "")));
    }

    #[test]
    fn interpolated_program() {
        let program_interpolated = format!("\"${{{}}}\"", program());
        let result = rib_program().easy_parse(program_interpolated.as_str());
        assert_eq!(result, Ok((expected(), "")));
    }
}
