use combine::{
    between, choice,
    parser::char::{char, spaces, string},
    Parser,
};

use crate::expr::Expr;

use super::rib_expr::rib_expr;
use combine::stream::easy;

pub fn option<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    choice((
        spaces().with(between(string("some("), char(')'), rib_expr())
            .map(|expr| Expr::Option(Some(Box::new(expr))))),
        spaces().with(string("none").map(|_| Expr::Option(None))),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_some() {
        let input = "some(foo)";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Option(Some(Box::new(Expr::Identifier("foo".to_string())))),
                ""
            ))
        );
    }

    #[test]
    fn test_none() {
        let input = "none";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Option(None), "")));
    }

    #[test]
    fn test_nested_some() {
        let input = "some(some(foo))";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Option(Some(Box::new(Expr::Option(Some(Box::new(
                    Expr::Identifier("foo".to_string())
                )))))),
                ""
            ))
        );
    }

    #[test]
    fn test_some_of_sequence() {
        let input = "some([foo, bar])";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Option(Some(Box::new(Expr::Sequence(vec![
                    Expr::Identifier("foo".to_string()),
                    Expr::Identifier("bar".to_string())
                ])))),
                ""
            ))
        );
    }

    #[test]
    fn test_some_of_literal() {
        let input = "some(\"foo\")";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Option(Some(Box::new(Expr::Literal("foo".to_string())))),
                ""
            ))
        );
    }
}
