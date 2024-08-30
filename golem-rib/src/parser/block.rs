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

use combine::{
    between,
    parser::char::{char as char_, spaces},
    Parser,
};

use crate::expr::Expr;

use crate::parser::rib_expr::rib_program;
use combine::stream::easy;

pub fn block<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(between(
        char_('{').skip(spaces()),
        char_('}').skip(spaces()),
        rib_program().skip(spaces()),
    ))
}

#[cfg(test)]
mod tests {
    use crate::expr::Expr;
    use crate::{ArmPattern, MatchArm, ParsedFunctionName};

    #[test]
    fn test_block_parse() {
        let rib_expr = r#"
          {
            let x = 1;
            let y = 2;
            foo(x);
            foo(y)
          }
        "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let expected = Expr::multiple(vec![
            Expr::let_binding("x", Expr::number(1f64)),
            Expr::let_binding("y", Expr::number(2f64)),
            Expr::call(
                ParsedFunctionName::parse("foo").unwrap(),
                vec![Expr::identifier("x")],
            ),
            Expr::call(
                ParsedFunctionName::parse("foo").unwrap(),
                vec![Expr::identifier("y")],
            ),
        ]);

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_block_in_if_expr() {
        let rib_expr = r#"
          if true then {
            let x = 1;
            let y = 2;
            foo(x);
            foo(y)
          } else 1
        "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let expected = Expr::cond(
            Expr::boolean(true),
            Expr::multiple(vec![
                Expr::let_binding("x", Expr::number(1f64)),
                Expr::let_binding("y", Expr::number(2f64)),
                Expr::call(
                    ParsedFunctionName::parse("foo").unwrap(),
                    vec![Expr::identifier("x")],
                ),
                Expr::call(
                    ParsedFunctionName::parse("foo").unwrap(),
                    vec![Expr::identifier("y")],
                ),
            ]),
            Expr::number(1f64),
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_block_in_match_expr() {
        let rib_expr = r#"
          match foo {
           some(x) => {
              let x = 1;
              let y = 2;
              foo(x);
              foo(y)
            }
          }
        "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let expected = Expr::pattern_match(
            Expr::identifier("foo"),
            vec![MatchArm::new(
                ArmPattern::Literal(Box::new(Expr::option(Some(Expr::identifier("x"))))),
                Expr::multiple(vec![
                    Expr::let_binding("x", Expr::number(1f64)),
                    Expr::let_binding("y", Expr::number(2f64)),
                    Expr::call(
                        ParsedFunctionName::parse("foo").unwrap(),
                        vec![Expr::identifier("x")],
                    ),
                    Expr::call(
                        ParsedFunctionName::parse("foo").unwrap(),
                        vec![Expr::identifier("y")],
                    ),
                ]),
            )],
        );

        assert_eq!(expr, expected);
    }

    #[test]
    fn test_nested_block() {
        let rib_expr = r#"
          let foo = some(1);
          match foo {
           some(x) => {
              let x = 1;
              let y = 2;
              foo(x);
              foo(y)
            }
          }
        "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let expected = Expr::multiple(vec![
            Expr::let_binding("foo", Expr::option(Some(Expr::number(1f64)))),
            Expr::pattern_match(
                Expr::identifier("foo"),
                vec![MatchArm::new(
                    ArmPattern::Literal(Box::new(Expr::option(Some(Expr::identifier("x"))))),
                    Expr::multiple(vec![
                        Expr::let_binding("x", Expr::number(1f64)),
                        Expr::let_binding("y", Expr::number(2f64)),
                        Expr::call(
                            ParsedFunctionName::parse("foo").unwrap(),
                            vec![Expr::identifier("x")],
                        ),
                        Expr::call(
                            ParsedFunctionName::parse("foo").unwrap(),
                            vec![Expr::identifier("y")],
                        ),
                    ]),
                )],
            ),
        ]);

        assert_eq!(expr, expected);
    }
}
