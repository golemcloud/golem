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
use combine::parser::char::{spaces, string};
use combine::stream::easy;
use combine::Parser;

pub fn greater_than<'t>(
    rib_expr1: impl Parser<easy::Stream<&'t str>, Output = Expr>,
    rib_expr2: impl Parser<easy::Stream<&'t str>, Output = Expr>,
) -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        (
            rib_expr1.skip(spaces()),
            string(">").skip(spaces()),
            rib_expr2.skip(spaces()),
        )
            .map(|(left, _, right)| Expr::GreaterThan(Box::new(left), Box::new(right))),
    )
}

pub fn greater_than_or_equal_to<'t>(
    rib_expr1: impl Parser<easy::Stream<&'t str>, Output = Expr>,
    rib_expr2: impl Parser<easy::Stream<&'t str>, Output = Expr>,
) -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        (
            rib_expr1.skip(spaces()),
            string(">=").skip(spaces()),
            rib_expr2,
        )
            .map(|(left, _, right)| Expr::GreaterThanOrEqualTo(Box::new(left), Box::new(right))),
    )
}

pub fn less_than<'t>(
    rib_expr1: impl Parser<easy::Stream<&'t str>, Output = Expr>,
    rib_expr2: impl Parser<easy::Stream<&'t str>, Output = Expr>,
) -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        (
            rib_expr1.skip(spaces()),
            string("<").skip(spaces()),
            rib_expr2,
        )
            .map(|(left, _, right)| Expr::LessThan(Box::new(left), Box::new(right))),
    )
}

pub fn less_than_or_equal_to<'t>(
    rib_expr1: impl Parser<easy::Stream<&'t str>, Output = Expr>,
    rib_expr2: impl Parser<easy::Stream<&'t str>, Output = Expr>,
) -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        (
            rib_expr1.skip(spaces()),
            string("<=").skip(spaces()),
            rib_expr2,
        )
            .map(|(left, _, right)| Expr::LessThanOrEqualTo(Box::new(left), Box::new(right))),
    )
}

pub fn equal_to<'t>(
    rib_expr1: impl Parser<easy::Stream<&'t str>, Output = Expr>,
    rib_expr2: impl Parser<easy::Stream<&'t str>, Output = Expr>,
) -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        (
            rib_expr1.skip(spaces()),
            string("==").skip(spaces()),
            rib_expr2,
        )
            .map(|(left, _, right)| Expr::EqualTo(Box::new(left), Box::new(right))),
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
                Expr::GreaterThan(
                    Box::new(Expr::Identifier("foo".to_string())),
                    Box::new(Expr::Identifier("bar".to_string()))
                ),
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
                Expr::GreaterThanOrEqualTo(
                    Box::new(Expr::Identifier("foo".to_string())),
                    Box::new(Expr::Identifier("bar".to_string()))
                ),
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
                Expr::LessThan(
                    Box::new(Expr::Identifier("foo".to_string())),
                    Box::new(Expr::Identifier("bar".to_string()))
                ),
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
                Expr::LessThanOrEqualTo(
                    Box::new(Expr::Identifier("foo".to_string())),
                    Box::new(Expr::Identifier("bar".to_string()))
                ),
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
                Expr::EqualTo(
                    Box::new(Expr::Identifier("foo".to_string())),
                    Box::new(Expr::Identifier("bar".to_string()))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_binary_op_in_if_condition() {
        let input = "if true then foo > bar  else  bar == foo";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Cond(
                    Box::new(Expr::Boolean(true)),
                    Box::new(Expr::GreaterThan(
                        Box::new(Expr::Identifier("foo".to_string())),
                        Box::new(Expr::Identifier("bar".to_string()))
                    )),
                    Box::new(Expr::EqualTo(
                        Box::new(Expr::Identifier("bar".to_string())),
                        Box::new(Expr::Identifier("foo".to_string()))
                    ))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_binary_op_in_sequence() {
        let input = "[foo >= bar, foo < bar]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Sequence(vec![
                    Expr::GreaterThanOrEqualTo(
                        Box::new(Expr::Identifier("foo".to_string())),
                        Box::new(Expr::Identifier("bar".to_string()))
                    ),
                    Expr::LessThan(
                        Box::new(Expr::Identifier("foo".to_string())),
                        Box::new(Expr::Identifier("bar".to_string()))
                    )
                ]),
                ""
            ))
        );
    }
}
