use crate::rib::expr::Expr;
use crate::rib::parser::rib_expr::rib_expr;
use combine::parser::char::{spaces, string};
use combine::stream::easy;
use combine::Parser;

pub fn conditional<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with(
        (
            string("if").skip(spaces()),
            rib_expr().skip(spaces()),
            string("then").skip(spaces()),
            rib_expr().skip(spaces()),
            string("else").skip(spaces()),
            rib_expr(),
        )
            .map(|(_, cond, _, then_expr, _, else_expr)| {
                Expr::Cond(Box::new(cond), Box::new(then_expr), Box::new(else_expr))
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_conditional() {
        let input = "if foo then bar else baz";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Cond(
                    Box::new(Expr::Identifier("foo".to_string())),
                    Box::new(Expr::Identifier("bar".to_string())),
                    Box::new(Expr::Identifier("baz".to_string()))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_conditional_of_sequences() {
        let input = "if foo then [bar] else [baz]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Cond(
                    Box::new(Expr::Identifier("foo".to_string())),
                    Box::new(Expr::Sequence(vec![Expr::Identifier("bar".to_string())])),
                    Box::new(Expr::Sequence(vec![Expr::Identifier("baz".to_string())]))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_if_condition_inside_else() {
        let input = "if foo then bar else if baz then qux else quux";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Cond(
                    Box::new(Expr::Identifier("foo".to_string())),
                    Box::new(Expr::Identifier("bar".to_string())),
                    Box::new(Expr::Cond(
                        Box::new(Expr::Identifier("baz".to_string())),
                        Box::new(Expr::Identifier("qux".to_string())),
                        Box::new(Expr::Identifier("quux".to_string()))
                    ))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_if_condition_inside_then() {
        let input = "if foo then if bar then baz else qux else quux";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Cond(
                    Box::new(Expr::Identifier("foo".to_string())),
                    Box::new(Expr::Cond(
                        Box::new(Expr::Identifier("bar".to_string())),
                        Box::new(Expr::Identifier("baz".to_string())),
                        Box::new(Expr::Identifier("qux".to_string()))
                    )),
                    Box::new(Expr::Identifier("quux".to_string()))
                ),
                ""
            ))
        );
    }
}
