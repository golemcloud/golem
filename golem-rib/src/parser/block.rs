use crate::parser::errors::RibParseError;
use crate::parser::rib_expr::rib_expr;
use crate::Expr;
use combine::parser::char::{char, spaces};
use combine::{sep_by, ParseError, Parser};

pub fn block<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    sep_by(rib_expr().skip(spaces()), char(';').skip(spaces())).map(|expressions: Vec<Expr>| {
        if expressions.len() == 1 {
            expressions.first().unwrap().clone()
        } else {
            Expr::expr_block(expressions)
        }
    })
}

#[cfg(test)]
mod tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use super::*;
    use combine::EasyParser;

    #[test]
    fn test_block() {
        let input = "\"foo\"; \"bar\"";
        let result = block().easy_parse(input);
        assert!(result.is_ok());
        let (expr, _) = result.unwrap();
        assert_eq!(
            expr,
            Expr::expr_block(vec![Expr::literal("foo"), Expr::literal("bar")])
        );
    }

    #[test]
    fn test_block_multiline() {
        let input = r#"
        let x = 1;
        let y = 2;
        x + y
        "#;
        let expr = block().easy_parse(input).unwrap().0;

        let expected = Expr::expr_block(vec![
            Expr::let_binding("x", Expr::untyped_number(BigDecimal::from(1)), None),
            Expr::let_binding("y", Expr::untyped_number(BigDecimal::from(2)), None),
            Expr::plus(
                Expr::identifier_global("x", None),
                Expr::identifier_global("y", None),
            ),
        ]);
        assert_eq!(expr, expected);
    }
}
