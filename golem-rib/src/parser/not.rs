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
use crate::parser::rib_expr::rib_expr;
use combine::parser::char::{spaces, string};
use combine::stream::easy;
use combine::Parser;

pub fn not<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        (string("!").skip(spaces()), rib_expr())
            .map(|(_, expr)| Expr::Not(Box::new(expr)))
            .message("Unable to parse not"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_not_identifier() {
        let input = "!foo";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::Not(Box::new(Expr::Identifier("foo".to_string()))), ""))
        );
    }

    #[test]
    fn test_not_sequence() {
        let input = "![foo, bar]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Not(Box::new(Expr::Sequence(vec![
                    Expr::Identifier("foo".to_string()),
                    Expr::Identifier("bar".to_string())
                ]))),
                ""
            ))
        );
    }

    #[test]
    fn test_not_not() {
        let input = "! !foo";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Not(Box::new(Expr::Not(Box::new(Expr::Identifier(
                    "foo".to_string()
                ))))),
                ""
            ))
        );
    }
}
