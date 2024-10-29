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

use crate::parser::errors::RibParseError;
use crate::parser::identifier::identifier_text;
use crate::parser::partial_block_expr::partial_block;
use crate::parser::rib_expr::rib_expr as expr;
use crate::{Expr, VariableId};
use combine::parser::char::{alpha_num, char, spaces, string};
use combine::{attempt, not_followed_by, optional, ParseError, Parser, Stream};

pub fn list_aggregation<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    (
        attempt(
            string("reduce").skip(
                not_followed_by(alpha_num().or(char('-')).or(char('_')))
                    .skip(spaces())
                    .skip(spaces()),
            ),
        ),
        identifier_text()
            .skip(spaces())
            .map(VariableId::list_reduce_identifier)
            .skip(spaces()),
        char(',').skip(spaces()),
        identifier_text()
            .skip(spaces())
            .map(VariableId::list_comprehension_identifier)
            .skip(spaces()),
        string("in").skip(spaces()),
        expr().skip(spaces()),
        string("from").skip(spaces()),
        expr().skip(spaces()),
        char('{').skip(spaces()),
        optional(partial_block().skip(spaces())),
        string("yield").skip(spaces()),
        expr().skip(spaces()),
        char(';').skip(spaces()),
        char('}'),
    )
        .map(
            |(
                _,
                reduced_variable,
                _,
                reduce_variable,
                _,
                iterable_expr,
                _,
                init_value_expr,
                _,
                optional_block,
                _,
                yield_expr,
                _,
                _,
            )| {
                let expr = if let Some(mut block) = optional_block {
                    block.push(yield_expr);
                    Expr::expr_block(block)
                } else {
                    yield_expr
                };
                Expr::list_reduce(
                    reduced_variable,
                    reduce_variable,
                    iterable_expr,
                    init_value_expr,
                    expr,
                )
            },
        )
}

#[cfg(test)]
mod tests {
    use crate::VariableId;
    use crate::{Expr, TypeName};
    use test_r::test;

    #[test]
    fn test_list_aggregation() {
        let input = "reduce z, p in [1, 2] from 0 { yield z + p; }";
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::list_reduce(
                VariableId::list_reduce_identifier("z"),
                VariableId::list_comprehension_identifier("p"),
                Expr::sequence(vec![Expr::untyped_number(1f64), Expr::untyped_number(2f64)]),
                Expr::untyped_number(0f64),
                Expr::expr_block(vec![Expr::plus(
                    Expr::identifier("z"),
                    Expr::identifier("p")
                )]),
            )
        );
    }

    #[test]
    fn test_list_aggregation2() {
        let input = r#"
           let ages: list<u16> = [1, 2, 3];
           reduce z, a in ages from 0 {
              yield z + a;
           }
        "#;
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::expr_block(vec![
                Expr::let_binding_with_type(
                    "ages",
                    TypeName::List(Box::new(TypeName::U16)),
                    Expr::sequence(vec![
                        Expr::untyped_number(1f64),
                        Expr::untyped_number(2f64),
                        Expr::untyped_number(3f64)
                    ])
                ),
                Expr::list_reduce(
                    VariableId::list_reduce_identifier("z"),
                    VariableId::list_comprehension_identifier("a"),
                    Expr::identifier("ages"),
                    Expr::untyped_number(0f64),
                    Expr::expr_block(vec![Expr::plus(
                        Expr::identifier("z"),
                        Expr::identifier("a")
                    )]),
                )
            ])
        );
    }
}
