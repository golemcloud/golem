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
    many1,
    parser::char::{char as char_, letter, spaces, string},
    Parser,
};

use crate::expr::Expr;
use crate::parser::rib_expr::rib_expr;
use combine::stream::easy;

pub fn let_binding<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        (
            string("let").skip(spaces()),
            let_variable().skip(spaces()),
            char_('=').skip(spaces()),
            rib_expr(),
        )
            .map(|(_, var, _, expr)| Expr::Let(var, Box::new(expr))),
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

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_let_binding() {
        let input = "let foo = bar";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Identifier("bar".to_string()))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_sequence() {
        let input = "let foo = [bar, baz]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Sequence(vec![
                        Expr::Identifier("bar".to_string()),
                        Expr::Identifier("baz".to_string())
                    ]))
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
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::EqualTo(
                        Box::new(Expr::Identifier("bar".to_string())),
                        Box::new(Expr::Identifier("baz".to_string()))
                    ))
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
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Option(Some(Box::new(Expr::Identifier(
                        "bar".to_string()
                    )))))
                ),
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
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Result(Ok(Box::new(Expr::Identifier(
                        "bar".to_string()
                    )))))
                ),
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
            Ok((
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Literal("bar".to_string()))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_record() {
        let input = "let foo = { bar : baz }";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Record(vec![(
                        "bar".to_string(),
                        Box::new(Expr::Identifier("baz".to_string()))
                    )]))
                ),
                ""
            ))
        );
    }
}
