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
use combine::error::StreamError;
use combine::parser::char::digit;
use combine::parser::char::{char as char_, letter, spaces};
use combine::parser::repeat::many1;
use combine::stream::easy;
use combine::Parser;

pub fn identifier<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        many1(letter().or(digit()).or(char_('_').or(char_('-'))))
            .and_then(|s: Vec<char>| {
                if s.first().map_or(false, |&c| c.is_alphabetic()) {
                    Ok(s)
                } else {
                    Err(easy::Error::message_static_message(
                        "Identifier must start with a letter",
                    ))
                }
            })
            .map(|s: Vec<char>| Expr::Identifier(s.into_iter().collect()))
            .message("Unable to parse identifier"),
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
        assert_eq!(result, Ok((Expr::Identifier("foo".to_string()), "")));
    }
}
