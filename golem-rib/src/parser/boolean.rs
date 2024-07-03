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
use combine::parser::char::spaces;
use combine::parser::char::string;
use combine::{choice, easy, Parser};

pub fn boolean_literal<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    choice((
        string("true").map(|_| Expr::Boolean(true)),
        string("false").map(|_| Expr::Boolean(false)),
    ))
    .skip(spaces())
    .message("Unable to parse boolean literal")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::rib_expr::rib_expr;
    use combine::EasyParser;

    #[test]
    fn test_boolean_true() {
        let input = "true";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Boolean(true), "")));
    }

    #[test]
    fn test_boolean_false() {
        let input = "false";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Boolean(false), "")));
    }

    #[test]
    fn test_boolean_with_spaces() {
        let input = "true ";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Boolean(true), "")));
    }
}
