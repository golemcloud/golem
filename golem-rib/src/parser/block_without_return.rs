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

use crate::parser::errors::RibParseError;
use crate::parser::rib_expr::rib_expr;
use crate::Expr;
use combine::parser::char::{char, spaces};
use combine::{attempt, sep_end_by, ParseError, Parser};

// Get all expressions in a block
// that doesn't have a return type
// It is not a valid rib by itself, unless we resolve the return collection type
// aligning to Rib grammar spec
pub fn block_without_return<Input>() -> impl Parser<Input, Output = Vec<Expr>>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    spaces()
        .with(sep_end_by(
            attempt(rib_expr().skip(spaces())),
            char(';').skip(spaces()),
        ))
        .map(|block: Vec<Expr>| block)
}

#[cfg(test)]
mod tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_block_without_return() {
        let input = r#"
        let x = 1;
        let y = 2;
        x + y;
        "#;
        let expr = block_without_return().easy_parse(input).unwrap().0;

        let expected = vec![
            Expr::let_binding("x", Expr::untyped_number(BigDecimal::from(1))),
            Expr::let_binding("y", Expr::untyped_number(BigDecimal::from(2))),
            Expr::plus(Expr::identifier("x"), Expr::identifier("y")),
        ];
        assert_eq!(expr, expected);
    }
}
