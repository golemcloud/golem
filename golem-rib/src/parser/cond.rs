// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use combine::parser::char::{alpha_num, char, spaces, string};
use combine::{attempt, not_followed_by, ParseError, Parser};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::rib_expr::rib_expr;
use crate::rib_source_span::GetSourcePosition;

pub fn conditional<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    // Use attempt only for the initial "if" to resolve ambiguity with identifiers
    attempt(
        string("if")
            .skip(not_followed_by(alpha_num().or(char('-')).or(char('_'))))
            .skip(spaces()),
    )
    .with(
        (
            rib_expr().message("Expected condition expression after `if`"), // Custom message for `rib_expr` after `if`
            string("then").skip(spaces().silent()),
            rib_expr(),
            string("else"),
            spaces().silent(),
            rib_expr().silent().expected("else condition"),
        )
            .map(|(cond, _, lhs, _, _, rhs)| Expr::cond(cond, lhs, rhs)),
    )
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;

    #[test]
    fn test_conditional() {
        let input = "if foo then bar else baz";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::cond(
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None),
                Expr::identifier_global("baz", None)
            ))
        );
    }

    #[test]
    fn test_conditional_of_sequences() {
        let input = "if foo then [bar] else [baz]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::cond(
                Expr::identifier_global("foo", None),
                Expr::sequence(vec![Expr::identifier_global("bar", None)], None),
                Expr::sequence(vec![Expr::identifier_global("baz", None)], None)
            ))
        );
    }

    #[test]
    fn test_if_condition_inside_else() {
        let input = "if foo then bar else if baz then qux else quux";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::cond(
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None),
                Expr::cond(
                    Expr::identifier_global("baz", None),
                    Expr::identifier_global("qux", None),
                    Expr::identifier_global("quux", None)
                )
            ))
        );
    }

    #[test]
    fn test_if_condition_inside_then() {
        let input = "if foo then if bar then baz else qux else quux";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::cond(
                Expr::identifier_global("foo", None),
                Expr::cond(
                    Expr::identifier_global("bar", None),
                    Expr::identifier_global("baz", None),
                    Expr::identifier_global("qux", None)
                ),
                Expr::identifier_global("quux", None)
            ))
        );
    }
}
