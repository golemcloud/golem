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

use bigdecimal::BigDecimal;
use combine::parser::char::{char, digit, spaces};
use combine::{many1, optional, ParseError, Parser};
use std::str::FromStr;

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::type_name::{parse_basic_type, TypeName};

pub fn number<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    spaces()
        .with(
            (
                many1(digit().or(char('-')).or(char('.'))),
                optional(parse_basic_type()),
            )
                .and_then(|(s, typ_name): (Vec<char>, Option<TypeName>)| {
                    let primitive = s.into_iter().collect::<String>();
                    let big_decimal = BigDecimal::from_str(primitive.as_str());

                    match big_decimal {
                        Ok(big_decimal) => {
                            if let Some(typ_name) = typ_name {
                                Ok(Expr::untyped_number_with_type_name(
                                    big_decimal,
                                    typ_name.clone(),
                                ))
                            } else {
                                Ok(Expr::untyped_number(big_decimal))
                            }
                        }
                        Err(_) => {
                            Err(RibParseError::Message("Unable to parse number".to_string()).into())
                        }
                    }
                }),
        )
        .message("Unable to parse number")
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use combine::EasyParser;

    use super::*;

    #[test]
    fn test_number() {
        let input = "123";
        let result = number().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::untyped_number(BigDecimal::from(123)), ""))
        );
    }

    #[test]
    fn test_negative_number() {
        let input = "-123";
        let result = number().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::untyped_number(BigDecimal::from(-123)), ""))
        );
    }

    #[test]
    fn test_float_number() {
        let input = "123.456";
        let result = number().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::untyped_number(BigDecimal::from_str("123.456").unwrap()),
                ""
            ))
        );
    }

    #[test]
    fn test_number_with_binding_positive() {
        let input = "123u32";
        let result = number().easy_parse(input);
        let expected = Expr::untyped_number_with_type_name(BigDecimal::from(123), TypeName::U32);
        assert_eq!(result, Ok((expected, "")));
    }

    #[test]
    fn test_number_with_binding_negative() {
        let input = "-123s64";
        let result = number().easy_parse(input);
        let expected = Expr::untyped_number_with_type_name(BigDecimal::from(-123), TypeName::S64);
        assert_eq!(result, Ok((expected, "")));
    }

    #[test]
    fn test_number_with_binding_float() {
        let input = "-123.0f64";
        let result = number().easy_parse(input);
        let expected = Expr::untyped_number_with_type_name(
            BigDecimal::from_str("-123.0").unwrap(),
            TypeName::F64,
        );
        assert_eq!(result, Ok((expected, "")));
    }
}
