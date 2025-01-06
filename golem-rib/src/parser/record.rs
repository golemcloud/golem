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

use combine::{
    between, many1, parser,
    parser::char::{char as char_, letter, spaces},
    sep_by1, ParseError, Parser, Stream,
};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;

use super::rib_expr::rib_expr;

parser! {
    pub fn record[Input]()(Input) -> Expr
    where [
        Input: Stream<Token = char>,
        RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,
    ]
    {
       record_()
    }
}

pub fn record_<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    spaces()
        .with(
            between(
                char_('{').skip(spaces()),
                char_('}').skip(spaces()),
                sep_by1(field().skip(spaces()), char_(',').skip(spaces())),
            )
            .map(|fields: Vec<Field>| {
                Expr::record(
                    fields
                        .iter()
                        .map(|f| (f.key.clone(), f.value.clone()))
                        .collect::<Vec<_>>(),
                )
            }),
        )
        .message("Invalid syntax for record type")
}

fn field_key<Input>() -> impl Parser<Input, Output = String>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    many1(letter().or(char_('_').or(char_('-'))))
        .map(|s: Vec<char>| s.into_iter().collect())
        .message("Invalid identifier")
}

struct Field {
    key: String,
    value: Expr,
}

fn field<Input>() -> impl Parser<Input, Output = Field>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    (
        field_key().skip(spaces()),
        char_(':').skip(spaces()),
        rib_expr(),
    )
        .map(|(var, _, expr)| Field {
            key: var,
            value: expr,
        })
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use combine::EasyParser;

    use super::*;

    #[test]
    fn test_singleton_record() {
        let input = "{foo: bar}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::record(vec![("foo".to_string(), Expr::identifier("bar"))]),
                ""
            ))
        );
    }

    #[test]
    fn test_record() {
        let input = "{foo: bar, baz: qux}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::record(vec![
                    ("foo".to_string(), Expr::identifier("bar")),
                    ("baz".to_string(), Expr::identifier("qux"))
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_record_with_values() {
        let input = "{ foo: \"bar\" }";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::record(vec![("foo".to_string(), Expr::literal("bar"))]),
                ""
            ))
        );
    }

    #[test]
    fn test_record_with_invalid_values() {
        let input = "{ foo: 'bar' }";
        let result = rib_expr().easy_parse(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_nested_records() {
        let input = "{foo: {bar: baz}}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::record(vec![(
                    "foo".to_string(),
                    Expr::record(vec![("bar".to_string(), Expr::identifier("baz"))])
                )]),
                ""
            ))
        );
    }

    #[test]
    fn test_record_of_tuple() {
        let input = "{foo: (bar, baz)}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::record(vec![(
                    "foo".to_string(),
                    Expr::tuple(vec![Expr::identifier("bar"), Expr::identifier("baz")])
                )]),
                ""
            ))
        );
    }

    #[test]
    fn test_record_of_sequence() {
        let input = "{foo: [bar, baz]}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::record(vec![(
                    "foo".to_string(),
                    Expr::sequence(vec![Expr::identifier("bar"), Expr::identifier("baz")])
                )]),
                ""
            ))
        );
    }

    #[test]
    fn test_record_of_result() {
        let input = "{foo: ok(bar)}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::record(vec![("foo".to_string(), Expr::ok(Expr::identifier("bar")))]),
                ""
            ))
        );
    }

    #[test]
    fn test_record_keys_can_be_key_words() {
        let input = "{err: bar}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::record(vec![("err".to_string(), Expr::identifier("bar"))]),
                ""
            ))
        );
    }
}
