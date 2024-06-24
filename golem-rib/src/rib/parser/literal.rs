use crate::rib::expr::Expr;
use combine::parser::char::{char, letter};
use combine::parser::repeat::many;
use combine::stream::easy;
use combine::{between, Parser};

pub fn literal<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    between(char('\"'), char('\"'), many(letter()))
        .map(|s: String| Expr::Literal(s))
        .message("Unable to parse literal")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rib::parser::rib_expr::rib_expr;
    use combine::EasyParser;

    #[test]
    fn test_empty_literal() {
        let input = "\"\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Literal("".to_string()), "")));
    }

    #[test]
    fn test_literal() {
        let input = "\"foo\"";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Literal("foo".to_string()), "")));
    }
}
