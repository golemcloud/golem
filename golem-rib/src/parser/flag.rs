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

use combine::parser::char::digit;
use combine::{attempt, sep_by};
use combine::{
    many1,
    parser::char::{char as char_, letter, spaces},
    ParseError, Parser,
};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::rib_source_span::GetSourcePosition;

pub fn flag<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    let flag_name = many1(letter().or(char_('_')).or(digit()).or(char_('-')))
        .map(|s: Vec<char>| s.into_iter().collect());

    (
        char_('{').skip(spaces().silent()),
        sep_by(
            attempt(flag_name.skip(spaces().silent())),
            char_(',').skip(spaces().silent()),
        ),
        char_('}').skip(spaces().silent()),
    )
        .map(|(_, flags, _): (_, Vec<String>, _)| Expr::flags(flags))
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;

    #[test]
    fn test_empty_flag() {
        let input = "{}";
        let result = Expr::from_text(input);
        assert_eq!(result, Ok(Expr::flags(vec![])));
    }

    #[test]
    fn test_flag_singleton() {
        let input = "{foo}";
        let result = Expr::from_text(input);
        assert_eq!(result, Ok(Expr::flags(vec!["foo".to_string()])));
    }

    #[test]
    fn test_flag() {
        let input = "{ foo, bar}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::flags(vec!["foo".to_string(), "bar".to_string()]))
        );
    }

    #[test]
    fn test_bool_str_flags() {
        let input = "{true, false}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::flags(vec!["true".to_string(), "false".to_string()]))
        );
    }
}
