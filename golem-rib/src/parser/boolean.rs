use combine::{choice, easy, Parser};
use combine::parser::char::spaces;
use combine::parser::char::string;
use crate::expr::Expr;


pub fn boolean_literal<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    choice((
        string("true").map(|_| Expr::Boolean(true)),
        string("false").map(|_| Expr::Boolean(false)),
    ))
        .skip(spaces())
        .message("Unable to parse boolean literal")
}

#[cfg(test)]
mod tests {
    use super::*;
    use combine::EasyParser;
    use crate::parser::rib_expr::rib_expr;

    #[test]
    fn test_boolean_true() {
        let input = "true";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Boolean(true), "")));
    }

    #[test]
    fn test_boolean_false() {
        let input = "false";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Boolean(false), "")));
    }

    #[test]
    fn test_boolean_with_spaces() {
        let input = "true ";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Boolean(true), "")));
    }
}