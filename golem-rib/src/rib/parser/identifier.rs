use crate::rib::expr::Expr;
use combine::parser::char::{char as char_, letter};
use combine::parser::repeat::many1;
use combine::stream::easy;
use combine::Parser;

pub fn identifier<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    many1(letter().or(char_('_')))
        .map(|s: Vec<char>| Expr::Identifier(s.into_iter().collect()))
        .message("Unable to parse identifier")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rib::parser::rib_expr::rib_expr;
    use combine::EasyParser;

    #[test]
    fn test_identifier() {
        let input = "foo";
        let result = rib_expr().easy_parse(input);
        assert_eq!(result, Ok((Expr::Identifier("foo".to_string()), "")));
    }
}
