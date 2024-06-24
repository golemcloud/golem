use combine::{
    many1,
    parser::char::{char as char_, letter, spaces, string},
    Parser,
};

use crate::rib::expr::Expr;
use crate::rib::parser::rib_expr::rib_expr;
use combine::stream::easy;

pub fn let_binding<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    (
        string("let").skip(spaces()),
        let_variable().skip(spaces()),
        char_('=').skip(spaces()),
        rib_expr(),
        char_(';'),
    )
        .map(|(_, var, _, expr, _)| Expr::Let(var, Box::new(expr)))
}

fn let_variable<'t>() -> impl Parser<easy::Stream<&'t str>, Output = String> {
    many1(letter().or(char_('_')))
        .map(|s: Vec<char>| s.into_iter().collect())
        .message("Unable to parse identifier")
}

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_let_binding() {
        let input = "let foo = bar;";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Identifier("bar".to_string()))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_sequence() {
        let input = "let foo = [bar, baz];";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Sequence(vec![
                        Expr::Identifier("bar".to_string()),
                        Expr::Identifier("baz".to_string())
                    ]))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_binary_comparisons() {
        let input = "let foo = bar == baz;";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::EqualTo(
                        Box::new(Expr::Identifier("bar".to_string())),
                        Box::new(Expr::Identifier("baz".to_string()))
                    ))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_option() {
        let input = "let foo = some(bar);";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Option(Some(Box::new(Expr::Identifier(
                        "bar".to_string()
                    )))))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_result() {
        let input = "let foo = ok(bar);";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Result(Ok(Box::new(Expr::Identifier(
                        "bar".to_string()
                    )))))
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_let_binding_with_literal() {
        let input = "let foo = \"bar\";";
        let result = let_binding().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::Let(
                    "foo".to_string(),
                    Box::new(Expr::Literal("bar".to_string()))
                ),
                ""
            ))
        );
    }
}
