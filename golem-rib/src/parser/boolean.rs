// Copyright 2024-2025 Golem Cloud
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

use combine::parser::char::spaces;
use combine::parser::char::string;
use combine::{attempt, ParseError, Parser};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;

pub fn boolean_literal<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    attempt(string("true"))
        .map(|_| Expr::boolean(true))
        .or(attempt(string("false")).map(|_| Expr::boolean(false)))
        .skip(spaces())
        .message("Unable to parse boolean literal")
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use combine::EasyParser;

    use crate::parser::rib_expr::rib_expr;

    use super::*;

    #[test]
    fn test_boolean_true() {
        let input = "true";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::boolean(true), "")));
    }

    #[test]
    fn test_boolean_false() {
        let input = "false";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::boolean(false), "")));
    }

    #[test]
    fn test_boolean_with_spaces() {
        let input = "true ";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::boolean(true), "")));
    }
}
