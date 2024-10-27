use crate::parser::errors::RibParseError;
use crate::parser::identifier::identifier_text;
use crate::parser::rib_expr::rib_expr as expr;
use crate::{Expr, VariableId};
use combine::parser::char::{alpha_num, char, spaces, string};
use combine::{attempt, not_followed_by, optional, ParseError, Parser, Stream};

pub fn list_comprehension<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    (
        attempt(
            string("for").skip(
                not_followed_by(alpha_num().or(char('-')).or(char('_')))
                    .skip(spaces())
                    .skip(spaces()),
            ),
        ),
        identifier_text()
            .skip(spaces())
            .map(|name| VariableId::list_comprehension_identifier(name)),
        string("in").skip(spaces()),
        expr().skip(spaces()),
        char('{').skip(spaces()),
        optional(internal::block().skip(spaces())),
        string("yield").skip(spaces()),
        expr().skip(spaces()),
        char(';').skip(spaces()),
        char('}'),
    )
        .map(
            |(_, var, _, iterable, _, optional_block, _, yield_expr, _, _)| {
                let expr = if let Some(mut block) = optional_block {
                    block.push(yield_expr);
                    Expr::expr_block(block)
                } else {
                    yield_expr
                };
                Expr::list_comprehension(var, iterable, expr)
            },
        )
}

mod internal {
    use crate::parser::errors::RibParseError;
    use crate::parser::rib_expr::rib_expr;
    use crate::Expr;
    use combine::parser::char::{char, spaces};
    use combine::{attempt, sep_end_by, ParseError, Parser};

    pub fn block<Input>() -> impl Parser<Input, Output = Vec<Expr>>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        spaces()
            .with(sep_end_by(
                attempt(rib_expr().skip(spaces())),
                char(';').skip(spaces()),
            ))
            .map(|block: Vec<Expr>| block)
    }
}

#[cfg(test)]
mod tests {
    use crate::Expr;
    use crate::VariableId;
    use test_r::test;

    #[test]
    fn test_list_comprehension1() {
        let input = "for x in [\"foo\", \"bar\"] { yield x; }";
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::list_comprehension(
                VariableId::list_comprehension_identifier("x".to_string()),
                Expr::sequence(vec![Expr::literal("foo"), Expr::literal("bar")]),
                Expr::expr_block(vec![Expr::identifier("x")]),
            )
        );
    }

    #[test]
    fn test_list_comprehension2() {
        let input = r#"
           let x = ["foo", "bar"];

           for p in x {
              yield p;
           }
        "#;
        let result = Expr::from_text(input).unwrap();
        assert_eq!(
            result,
            Expr::expr_block(vec![
                Expr::let_binding(
                    "x",
                    Expr::sequence(vec![Expr::literal("foo"), Expr::literal("bar")])
                ),
                Expr::list_comprehension(
                    VariableId::list_comprehension_identifier("p".to_string()),
                    Expr::identifier("x"),
                    Expr::expr_block(vec![Expr::identifier("p")]),
                )
            ])
        );
    }
}
