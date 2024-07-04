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
use combine::{
    between, many1,
    parser::char::{char, letter, spaces},
    Parser,
};

use crate::expr::Expr;
use combine::sep_by;
use combine::stream::easy;

pub fn flag<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    let flag_name = many1(letter().or(char('_')).or(digit()).or(char('-')))
        .map(|s: Vec<char>| s.into_iter().collect());

    spaces().with(
        between(
            char('{').skip(spaces()),
            char('}').skip(spaces()),
            sep_by(flag_name.skip(spaces()), char(',').skip(spaces())),
        )
        .map(Expr::Flags)
        .message("Unable to parse flag"),
    )
}

#[cfg(test)]
mod tests {
    use crate::parser::rib_expr::rib_expr;

    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_empty_flag() {
        let input = "{}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Flags(vec![]), "")));
    }

    #[test]
    fn test_flag_singleton() {
        let input = "{foo}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Flags(vec!["foo".to_string()]), "")));
    }

    #[test]
    fn test_flag() {
        let input = "{ foo, bar}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((Expr::Flags(vec!["foo".to_string(), "bar".to_string()]), ""))
        );
    }

    #[test]
    fn test_bool_str_flags() {
        let input = "{true, false}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Flags(vec!["true".to_string(), "false".to_string()]),
                ""
            ))
        );
    }
}
