use combine::stream::easy;
use combine::{Stream};
use crate::rib::expr::Expr;
use combine::parser;

// Intention was to support nested selected field - which stack overflows unlike Scala
parser! {
    pub(crate) fn select_field['t]()(easy::Stream<&'t str>) -> Expr
    where [easy::Stream<&'t str>: Stream<Token = char>,]{
        internal::select_field_()
    }
}

mod internal {

    use combine::{ choice, many1, parser::char::char as char_, Parser, attempt};

    use combine::parser::char::letter;
    use combine::stream::easy;
    use crate::rib::expr::Expr;
    use crate::rib::parser::record::record;
    use combine::parser::char::spaces;

    pub fn select_field_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        (
            parent_root().skip(spaces()),
            char_('.').skip(spaces()),
            field_name().skip(spaces())

        ).map(|(select_field_root, _, field)| {
            Expr::SelectField(Box::new(select_field_root), field)
        })
    }

    pub fn field_name<'t>() -> impl Parser<easy::Stream<&'t str>, Output = String> {
        many1(letter().or(char_('_')))
            .map(|s: Vec<char>| s.into_iter().collect())
            .message("Unable to parse field name")
    }

    pub fn parent_root<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        choice((
            attempt(record()),
            attempt(field_name().map(Expr::Identifier)),
        ))
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rib::parser::rib_expr::rib_expr;
    use combine::EasyParser;

    #[test]
    fn test_select_field() {
        let input = "foo.bar";
        let result = select_field().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::SelectField(
                    Box::new(Expr::Identifier("foo".to_string())),
                    "bar".to_string()
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_select_field_from_record() {
        let input = "{foo: bar}.foo";
        let result = select_field().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::SelectField(
                    Box::new(Expr::Record(vec![(
                        "foo".to_string(),
                        Box::new(Expr::Identifier("bar".to_string()))
                    )])),
                    "foo".to_string()
                ),
                ""
            ))
        );
    }

    // TODO; Nested selection doesn't work - stack overflow
    #[ignore]
    #[test]
    fn test_nested_field_selection() {
        let input = "foo.bar.baz";
        let result = select_field().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::SelectField(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("foo".to_string())),
                        "bar".to_string()
                    )),
                    "baz".to_string()
                ),
                ""
            ))
        );
    }
}
