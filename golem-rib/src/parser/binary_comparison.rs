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
use crate::InferredType;
use combine::parser::char::{spaces, string};
use combine::{attempt, choice, Parser};

pub fn binary<Input>(
    left_expr: impl Parser<Input, Output = Expr>,
    right_expr: impl Parser<Input, Output = Expr>,
) -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
{
    spaces().with(
        (
            left_expr.skip(spaces()),
            choice((
                attempt(string(">=")),
                attempt(string("<=")),
                string("<"),
                string(">"),
                string("=="),
            ))
            .skip(spaces()),
            right_expr.skip(spaces()),
        )
            .message("Expected a valid expression of the form x > y")
            .map(|(left, str, right)| match str {
                ">" => Expr::GreaterThan(Box::new(left), Box::new(right), InferredType::Bool),
                "<" => Expr::LessThan(Box::new(left), Box::new(right), InferredType::Bool),
                "==" => Expr::EqualTo(Box::new(left), Box::new(right), InferredType::Bool),
                ">=" => {
                    Expr::GreaterThanOrEqualTo(Box::new(left), Box::new(right), InferredType::Bool)
                }
                "<=" => {
                    Expr::LessThanOrEqualTo(Box::new(left), Box::new(right), InferredType::Bool)
                }
                _ => unreachable!(),
            }),
    )
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::parser::rib_expr::rib_expr;
    use combine::EasyParser;

    #[test]
    fn test_greater_than() {
        let input = "foo > bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::greater_than(Expr::identifier("foo"), Expr::identifier("bar")),
                ""
            ))
        );
    }

    #[test]
    fn test_greater_than_or_equal_to() {
        let input = "foo >= bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::greater_than_or_equal_to(Expr::identifier("foo"), Expr::identifier("bar")),
                ""
            ))
        );
    }

    #[test]
    fn test_less_than() {
        let input = "foo < bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::less_than(Expr::identifier("foo"), Expr::identifier("bar")),
                ""
            ))
        );
    }

    #[test]
    fn test_less_than_or_equal_to() {
        let input = "foo <= bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::less_than_or_equal_to(Expr::identifier("foo"), Expr::identifier("bar")),
                ""
            ))
        );
    }

    #[test]
    fn test_equal_to() {
        let input = "foo == bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::equal_to(Expr::identifier("foo"), Expr::identifier("bar")),
                ""
            ))
        );
    }

    #[test]
    fn test_binary_op_in_if_condition() {
        let input = "if true then foo > bar  else  bar == foo";
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::cond(
                Expr::boolean(true),
                Expr::greater_than(Expr::identifier("foo"), Expr::identifier("bar")),
                Expr::equal_to(Expr::identifier("bar"), Expr::identifier("foo")),
            ),
        );
    }

    #[test]
    fn test_binary_op_in_sequence() {
        let input = "[foo >= bar, foo < bar]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::sequence(vec![
                    Expr::greater_than_or_equal_to(
                        Expr::identifier("foo"),
                        Expr::identifier("bar")
                    ),
                    Expr::less_than(Expr::identifier("foo"), Expr::identifier("bar"))
                ]),
                ""
            ))
        );
    }
}
