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

use crate::parser::block_without_return::block_without_return;
use crate::parser::errors::RibParseError;
use crate::parser::identifier::identifier_text;
use crate::parser::rib_expr::rib_expr;
use crate::rib_source_span::GetSourcePosition;
use crate::{Expr, VariableId};
use combine::parser::char::{alpha_num, char, spaces, string};
use combine::{attempt, not_followed_by, optional, ParseError, Parser, Stream};

pub fn list_comprehension<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    (
        attempt(
            string("for")
                .skip(not_followed_by(alpha_num().or(char('-')).or(char('_'))).skip(spaces())),
        ),
        identifier_text()
            .skip(spaces())
            .map(VariableId::list_comprehension_identifier),
        string("in").skip(spaces()),
        rib_expr(),
        char('{').skip(spaces()),
        optional(block_without_return().skip(spaces())),
        string("yield").skip(spaces()),
        rib_expr(),
        char(';').skip(spaces()),
        char('}'),
    )
        .map(|(_, var, _, iterable, _, opt_block, _, yield_expr, _, _)| {
            let expr = opt_block
                .map(|mut block| {
                    block.push(yield_expr.clone());
                    Expr::expr_block(block)
                })
                .unwrap_or(yield_expr);
            Expr::list_comprehension(var, iterable, expr)
        })
}

#[cfg(test)]
mod tests {
    use crate::Expr;
    use crate::VariableId;
    use test_r::test;

    #[test]
    fn test_list_comprehension1() {
        let input = "for x in [\"foo\", \"bar\"] { yield x; }";
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::list_comprehension(
                VariableId::list_comprehension_identifier("x"),
                Expr::sequence(vec![Expr::literal("foo"), Expr::literal("bar")], None),
                Expr::expr_block(vec![Expr::identifier_global("x", None)]),
            )
        );
    }

    #[test]
    fn test_list_comprehension2() {
        let input = r#"
           let x = ["foo", "bar"];

           for p in x {
              yield p;
           }
        "#;
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::expr_block(vec![
                Expr::let_binding(
                    "x",
                    Expr::sequence(vec![Expr::literal("foo"), Expr::literal("bar")], None),
                    None
                ),
                Expr::list_comprehension(
                    VariableId::list_comprehension_identifier("p"),
                    Expr::identifier_global("x", None),
                    Expr::expr_block(vec![Expr::identifier_global("p", None)]),
                )
            ])
        );
    }
}
