use crate::parser::errors::RibParseError;
use crate::parser::rib_expr::rib_expr;
use crate::rib_source_span::{GetSourcePosition, SourceSpan};
use crate::Expr;
use combine::parser::char::{char, spaces};
use combine::{position, sep_by, ParseError, Parser};

pub fn block<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    position()
        .and(sep_by(rib_expr().skip(spaces()), char(';').skip(spaces())))
        .and(position())
        .map(
            |((start, expressions), end): ((Input::Position, Vec<Expr>), Input::Position)| {
                let start = start.get_source_position();
                let end = end.get_source_position();
                let span = SourceSpan::new(start, end);

                if expressions.len() == 1 {
                    expressions.first().unwrap().clone()
                } else {
                    Expr::expr_block(expressions).with_source_span(span)
                }
            },
        )
}

#[cfg(test)]
mod tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use super::*;

    #[test]
    fn test_block() {
        let input = "\"foo\"; \"bar\"";
        let expr = Expr::from_text(input);
        assert!(expr.is_ok());
        let expr = expr.unwrap();
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
        let expr = Expr::from_text(input).unwrap();

        let expected = Expr::expr_block(vec![
            Expr::let_binding("x", Expr::number(BigDecimal::from(1)), None),
            Expr::let_binding("y", Expr::number(BigDecimal::from(2)), None),
            Expr::plus(
                Expr::identifier_global("x", None),
                Expr::identifier_global("y", None),
            ),
        ]);
        assert_eq!(expr, expected);
    }
}
