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
use combine::parser::char::digit;
use combine::parser::char::{char as char_, letter, spaces};
use combine::{many, Parser};

pub fn identifier<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
{
    identifier_text()
        .map(Expr::identifier)
        .message("Invalid identifier")
}

pub fn identifier_text<Input>() -> impl Parser<Input, Output = String>
where
    Input: combine::Stream<Token = char>,
{
    spaces().with(
        (
            letter(),
            many(letter().or(digit()).or(char_('_').or(char_('-')))),
        )
            .map(|(a, s): (char, Vec<char>)| {
                let mut vec = vec![a];
                vec.extend(s);
                vec.iter().collect::<String>()
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::rib_expr::rib_expr;
    use combine::EasyParser;

    #[test]
    fn test_identifier() {
        let input = "foo";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::identifier("foo"), "")));
    }
}
