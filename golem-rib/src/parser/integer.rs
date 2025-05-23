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

use bigdecimal::BigDecimal;
use combine::parser::char::{char as char_, digit};
use combine::{many1, optional, ParseError, Parser};
use std::str::FromStr;

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::rib_source_span::GetSourcePosition;

pub fn integer<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    let digits = many1(digit());

    (optional(char_('-')), digits).map(|(sign, num_str): (Option<char>, String)| {
        let num = BigDecimal::from_str(&num_str).unwrap(); // Convert to BigDecimal

        let big_decimal = if sign.is_some() { -num } else { num };

        Expr::number(big_decimal)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TypeName;
    use test_r::test;

    #[test]
    fn test_number() {
        let input = "123";
        let result = Expr::from_text(input);
        assert_eq!(result, Ok(Expr::number(BigDecimal::from(123))));
    }

    #[test]
    fn test_negative_number() {
        let input = "-123";
        let result = Expr::from_text(input);
        assert_eq!(result, Ok(Expr::number(BigDecimal::from(-123))));
    }

    #[test]
    fn test_number_with_binding_positive() {
        let input = "123u32";
        let result = Expr::from_text(input);
        let expected = Expr::number(BigDecimal::from(123)).with_type_annotation(TypeName::U32);
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_number_with_binding_negative() {
        let input = "-123s64";
        let result = Expr::from_text(input);
        let expected = Expr::number(BigDecimal::from(-123)).with_type_annotation(TypeName::S64);
        assert_eq!(result, Ok(expected));
    }
}
