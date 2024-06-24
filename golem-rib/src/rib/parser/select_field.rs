use combine::parser::char::{char as char_, letter, spaces, digit};
use combine::stream::easy;
use combine::{attempt, choice, many1, optional, Parser, Stream};

use crate::rib::expr::Expr;
use crate::rib::parser::record::record;
use combine::parser;

parser! {
    pub(crate) fn select_field['t]()(easy::Stream<&'t str>) -> Expr
    where [easy::Stream<&'t str>: Stream<Token = char>,] {
        internal::select_field_()
    }
}

mod internal {
    use combine::parser::char::char;
    use crate::rib::parser::rib_expr::rib_expr;
    use super::*;

    pub fn select_field_<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        (base_expr(), char('.').skip(spaces()), rib_expr())
            .map(|(base, _, opt)| match opt {
                Expr::SelectField(expr, field) => match *expr {
                    Expr::Identifier(first_field) => Expr::SelectField(Box::new(Expr::SelectField(Box::new(base), first_field)), field),
                    _ => panic!("Failed to parse")
                }
                Expr::Identifier(str) => Expr::SelectField(Box::new(base), str),
                _ => panic!("failed")
            })
    }

    fn base_expr<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        choice((
            attempt(record()),
            attempt(field_name().map(Expr::Identifier)),
        ))
    }

    fn field_name<'t>() -> impl Parser<easy::Stream<&'t str>, Output = String> {
        many1(letter().or(char_('_')))
            .map(|s: Vec<char>| s.into_iter().collect())
            .message("Unable to parse field name")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rib::parser::rib_expr::{rib_expr, rib_expr_};
    use combine::EasyParser;

    #[test]
    fn test_select_field() {
        let input = "foo.bar";
        let result = rib_expr().easy_parse(input);
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
        let result = rib_expr().easy_parse(input);
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

    #[test]
    fn test_nested_field_selection() {
        let input = "foo.bar.baz";
        let result = rib_expr().easy_parse(input);
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
