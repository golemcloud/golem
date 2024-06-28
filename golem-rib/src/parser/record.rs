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

use combine::{
    between, many1,
    parser::char::{char as char_, letter, spaces},
    sep_by1, Parser,
};

use crate::expr::Expr;

use super::rib_expr::rib_expr;
use combine::stream::easy;

pub fn record<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        between(
            char_('{').skip(spaces()),
            char_('}').skip(spaces()),
            sep_by1(field().skip(spaces()), char_(',').skip(spaces())),
        )
        .map(|fields: Vec<Field>| {
            Expr::Record(
                fields
                    .iter()
                    .map(|f| (f.key.clone(), Box::new(f.value.clone())))
                    .collect::<Vec<_>>(),
            )
        }),
    )
}

fn field_key<'t>() -> impl Parser<easy::Stream<&'t str>, Output = String> {
    many1(letter().or(char_('_').or(char_('-'))))
        .map(|s: Vec<char>| s.into_iter().collect())
        .message("Unable to parse identifier")
}

struct Field {
    key: String,
    value: Expr,
}

fn field<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Field> {
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
    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_singleton_record() {
        let input = "{foo: bar}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Record(vec![(
                    "foo".to_string(),
                    Box::new(Expr::Identifier("bar".to_string()))
                )]),
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
                Expr::Record(vec![
                    (
                        "foo".to_string(),
                        Box::new(Expr::Identifier("bar".to_string()))
                    ),
                    (
                        "baz".to_string(),
                        Box::new(Expr::Identifier("qux".to_string()))
                    )
                ]),
                ""
            ))
        );
    }

    #[test]
    fn test_nested_records() {
        let input = "{foo: {bar: baz}}";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Record(vec![(
                    "foo".to_string(),
                    Box::new(Expr::Record(vec![(
                        "bar".to_string(),
                        Box::new(Expr::Identifier("baz".to_string()))
                    )]))
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
                Expr::Record(vec![(
                    "foo".to_string(),
                    Box::new(Expr::Tuple(vec![
                        Expr::Identifier("bar".to_string()),
                        Expr::Identifier("baz".to_string())
                    ]))
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
                Expr::Record(vec![(
                    "foo".to_string(),
                    Box::new(Expr::Sequence(vec![
                        Expr::Identifier("bar".to_string()),
                        Expr::Identifier("baz".to_string())
                    ]))
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
                Expr::Record(vec![(
                    "foo".to_string(),
                    Box::new(Expr::Result(Ok(Box::new(Expr::Identifier(
                        "bar".to_string()
                    )))))
                )]),
                ""
            ))
        );
    }
}
