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

use combine::{many1, optional, Parser};

use crate::expr::Expr;
use combine::parser::char::{char, digit, spaces};

use combine::stream::easy;

use crate::parser::type_binding;
use crate::parser::type_name::{parse_basic_type, TypeName};
use combine::error::StreamError;

pub fn number<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        (
            many1(digit().or(char('-')).or(char('.'))),
            optional(parse_basic_type()),
        )
            .map(|(s, typ_name): (Vec<char>, Option<TypeName>)| {
                let primitive = s.into_iter().collect::<String>();

                if let Ok(f64) = primitive.parse::<f64>() {
                    if let Some(typ_name) = typ_name {
                        Ok(type_binding::bind(&Expr::number_with_type_name(f64, typ_name.clone()), Some(typ_name)))
                    } else {
                        Ok(Expr::number(f64))
                    }

                } else {
                    Err(easy::Error::message_static_message(
                        "Unable to parse number",
                    ))
                }
            })
            .and_then(|result| result) // Unwrap the result from the map closure
            .message("Unable to parse number"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InferredType, Number};
    use combine::EasyParser;

    #[test]
    fn test_number() {
        let input = "123";
        let result = number().easy_parse(input);
        assert_eq!(result, Ok((Expr::number(123f64), "")));
    }

    #[test]
    fn test_negative_number() {
        let input = "-123";
        let result = number().easy_parse(input);
        assert_eq!(result, Ok((Expr::number(-123f64), "")));
    }

    #[test]
    fn test_float_number() {
        let input = "123.456";
        let result = number().easy_parse(input);
        assert_eq!(result, Ok((Expr::number(123.456f64), "")));
    }

    #[test]
    fn test_number_with_binding_positive() {
        let input = "123u32";
        let result = number().easy_parse(input);
        let expected = Expr::Number(Number { value: 123f64 }, Some(TypeName::U32), InferredType::U32);
        assert_eq!(result, Ok((expected, "")));
    }

    #[test]
    fn test_number_with_binding_negative() {
        let input = "-123s64";
        let result = number().easy_parse(input);
        let expected = Expr::Number(Number { value: -123f64 }, Some(TypeName::S64), InferredType::S64);
        assert_eq!(result, Ok((expected, "")));
    }

    #[test]
    fn test_number_with_binding_float() {
        let input = "-123.0f64";
        let result = number().easy_parse(input);
        let expected = Expr::Number(Number { value: -123.0f64 }, Some(TypeName::F64), InferredType::F64);
        assert_eq!(result, Ok((expected, "")));
    }
}
