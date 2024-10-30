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

use combine::parser::char::digit;
use combine::parser::char::{char as char_, letter};
use combine::{many, ParseError, Parser, Stream};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;

const RESERVED_KEYWORDS: &[&str] = &[
    "if", "then", "else", "match", "ok", "some", "err", "none", "let", "for", "yield", "reduce",
];

pub fn identifier<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: Stream<Token = char>,
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    identifier_text()
        .map(Expr::identifier)
        .message("Invalid identifier")
}
pub fn identifier_text<Input>() -> impl Parser<Input, Output = String>
where
    Input: Stream<Token = char>,
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    (
        letter(),
        many(letter().or(digit()).or(char_('_').or(char_('-')))),
    )
        .map(|(a, s): (char, Vec<char>)| {
            let mut vec = vec![a];
            vec.extend(s);
            vec.iter().collect::<String>()
        })
        .and_then(|ident: String| {
            if RESERVED_KEYWORDS.contains(&ident.as_str()) {
                Err(RibParseError::Message(format!("{} is a keyword", ident)))
            } else {
                Ok(ident)
            }
        })
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;

    #[test]
    fn test_identifier() {
        let input = "foo";
        let result = Expr::from_text(input);
        assert_eq!(result, Ok(Expr::identifier("foo")));
    }

    #[test]
    fn test_identifiers_containing_key_words() {
        let inputs = RESERVED_KEYWORDS.iter().flat_map(|k| {
            vec![
                format!("{}foo", k),
                format!("{}_foo", k),
                format!("{}-foo", k),
                format!("foo{}", k),
                format!("foo_{}", k),
                format!("foo-{}", k),
            ]
        });

        for input in inputs {
            let result = Expr::from_text(&input);
            assert!(result.is_ok())
        }
    }
}
