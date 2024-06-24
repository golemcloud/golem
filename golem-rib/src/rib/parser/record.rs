use combine::{
    between, many1,
    parser::char::{char as char_, letter, spaces},
    sep_by1, Parser,
};

use crate::rib::expr::Expr;

use super::rib_expr::rib_expr;
use combine::stream::easy;

pub fn record<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    between(
        char_('{'),
        char_('}'),
        sep_by1(field(), char_(',').skip(spaces())),
    )
    .map(|fields: Vec<Field>| {
        Expr::Record(
            fields
                .iter()
                .map(|f| (f.key.clone(), Box::new(f.value.clone())))
                .collect::<Vec<_>>(),
        )
    })
}

fn field_key<'t>() -> impl Parser<easy::Stream<&'t str>, Output = String> {
    many1(letter().or(char_('_')))
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

    // Empty Records can be empty flags or empty records and has to return a possibility which can be deduded only during evaluation phase

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
