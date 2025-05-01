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

use super::rib_expr::rib_expr;
use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::rib_source_span::{GetSourcePosition, SourceSpan};
use combine::parser::char::digit;
use combine::{
    between, many, parser,
    parser::char::{char as char_, letter, spaces},
    position, sep_by1, ParseError, Parser, Stream,
};

parser! {
    pub fn record[Input]()(Input) -> Expr
    where [
        Input: Stream<Token = char>,
        RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,
        Input::Position: GetSourcePosition
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
    Input::Position: GetSourcePosition,
{
    between(
        char_('{').skip(spaces().silent()),
        char_('}').skip(spaces().silent()),
        sep_by1(
            field().skip(spaces().silent()),
            char_(',').skip(spaces().silent()),
        ),
    )
    .and_then(|fields: Vec<Field>| {
        let duplicate_keys = find_duplicate_keys(&fields);

        if !duplicate_keys.is_empty() {
            Err(RibParseError::Message(format!(
                "duplicate keys found in record: {}",
                duplicate_keys.join(", ")
            )))
        } else {
            Ok(Expr::record(
                fields
                    .iter()
                    .map(|f| (f.key.clone(), f.value.clone()))
                    .collect::<Vec<_>>(),
            ))
        }
    })
}

fn find_duplicate_keys(fields: &[Field]) -> Vec<String> {
    let mut keys = std::collections::HashMap::new();
    let mut duplicates = vec![];

    for field in fields {
        if keys.contains_key(&field.key) {
            duplicates.push(field.key.clone());
        } else {
            keys.insert(field.key.clone(), true);
        }
    }

    duplicates
}

fn field_key<Input>() -> impl Parser<Input, Output = String>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    letter()
        .and(many(letter().or(digit()).or(char_('_')).or(char_('-'))))
        .map(|(first, rest): (char, Vec<char>)| {
            let mut chars = vec![first];
            chars.extend(rest);
            chars.into_iter().collect::<String>()
        })
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
    Input::Position: GetSourcePosition,
{
    (
        field_key().skip(spaces().silent()),
        char_(':').skip(spaces().silent()),
        position(),
        rib_expr(),
        position(),
    )
        .map(|(var, _, start, expr, end)| {
            let start: Input::Position = start;
            let start = start.get_source_position();
            let end: Input::Position = end;
            let end = end.get_source_position();
            let span = SourceSpan::new(start, end);

            Field {
                key: var,
                value: expr.with_source_span(span),
            }
        })
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;

    #[test]
    fn test_singleton_record() {
        let input = "{foo: bar}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::record(vec![(
                "foo".to_string(),
                Expr::identifier_global("bar", None)
            )]))
        );
    }

    #[test]
    fn test_record() {
        let input = "{foo: bar, baz: qux}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::record(vec![
                ("foo".to_string(), Expr::identifier_global("bar", None)),
                ("baz".to_string(), Expr::identifier_global("qux", None))
            ]))
        );
    }

    #[test]
    fn test_record_with_values() {
        let input = "{ foo: \"bar\" }";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::record(vec![(
                "foo".to_string(),
                Expr::literal("bar")
            )]))
        );
    }

    #[test]
    fn test_record_with_invalid_values() {
        let input = "{ foo: 'bar' }";
        let result = Expr::from_text(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_nested_records() {
        let input = "{foo: {bar: baz}}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::record(vec![(
                "foo".to_string(),
                Expr::record(vec![(
                    "bar".to_string(),
                    Expr::identifier_global("baz", None)
                )])
            )]))
        );
    }

    #[test]
    fn test_record_of_tuple() {
        let input = "{foo: (bar, baz)}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::record(vec![(
                "foo".to_string(),
                Expr::tuple(vec![
                    Expr::identifier_global("bar", None),
                    Expr::identifier_global("baz", None)
                ])
            )]))
        );
    }

    #[test]
    fn test_record_of_sequence() {
        let input = "{foo: [bar, baz]}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::record(vec![(
                "foo".to_string(),
                Expr::sequence(
                    vec![
                        Expr::identifier_global("bar", None),
                        Expr::identifier_global("baz", None)
                    ],
                    None
                )
            )]))
        );
    }

    #[test]
    fn test_record_of_result() {
        let input = "{foo: ok(bar)}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::record(vec![(
                "foo".to_string(),
                Expr::ok(Expr::identifier_global("bar", None), None)
            )]))
        );
    }

    #[test]
    fn test_record_keys_can_be_key_words() {
        let input = "{err: bar}";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::record(vec![(
                "err".to_string(),
                Expr::identifier_global("bar", None)
            )]))
        );
    }

    #[test]
    fn test_record_nested() {
        let expr = r#"
      {
         headers: { ContentType: "json", userid: "foo" },
         body: "foo",
         status: status
       }
        "#;

        let result = Expr::from_text(expr);

        assert_eq!(
            result,
            Ok(Expr::record(vec![
                (
                    "headers".to_string(),
                    Expr::record(vec![
                        ("ContentType".to_string(), Expr::literal("json")),
                        ("userid".to_string(), Expr::literal("foo"))
                    ])
                ),
                ("body".to_string(), Expr::literal("foo")),
                (
                    "status".to_string(),
                    Expr::identifier_global("status", None)
                )
            ]))
        );
    }
}
