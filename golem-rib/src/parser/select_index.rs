use crate::expr::Expr;
use crate::parser::identifier::identifier;
use combine::parser::char::{char as char_, letter, spaces};
use combine::stream::easy;
use combine::{attempt, choice, many1, optional, Parser, Stream};
use internal::*;

pub fn select_index<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
    spaces().with((
        base_expr().skip(spaces()),
        char_('[').skip(spaces()),
        pos_num().skip(spaces()),
        char_(']').skip(spaces()),
        optional(nested_indices()),
    )
        .map(
            |(expr, _, number, _, possible_indices)| match possible_indices {
                Some(indices) => {
                    build_select_index_from(Expr::SelectIndex(Box::new(expr), number), indices)
                }
                None => Expr::SelectIndex(Box::new(expr), number),
            },
        ))
}

mod internal {
    use super::*;
    use crate::expr::Number;
    use crate::parser::number::number;
    use crate::parser::sequence::sequence;
    use combine::error::StreamError;
    use combine::parser::char::char as char_;

    pub(crate) fn build_select_index_from(base_expr: Expr, indices: Vec<usize>) -> Expr {
        let mut result = base_expr;
        for index in indices {
            result = Expr::SelectIndex(Box::new(result), index);
        }
        result
    }

    pub(crate) fn nested_indices<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Vec<usize>> {
        many1(
            (
                char_('[').skip(spaces()),
                pos_num().skip(spaces()),
                char_(']').skip(spaces()),
            )
                .map(|(_, number, _)| number),
        )
        .map(|result: Vec<usize>| result)
    }

    pub(crate) fn pos_num<'t>() -> impl Parser<easy::Stream<&'t str>, Output = usize> {
        number().and_then(|s: Expr| match s {
            Expr::Number(number) => match number {
                Number::Signed(_) => Err(easy::Error::message_static_message(
                    "Cannot use a negative number to index",
                )),
                Number::Float(_) => Err(easy::Error::message_static_message(
                    "Cannot use a float number to index",
                )),
                Number::Unsigned(u64) => Ok(u64 as usize),
            },
            _ => Err(easy::Error::message_static_message(
                "Cannot use a float number to index",
            )),
        })
    }

    pub(crate) fn base_expr<'t>() -> impl Parser<easy::Stream<&'t str>, Output = Expr> {
        choice((attempt(sequence()), attempt(identifier())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::*;
    use crate::parser::rib_expr::{rib_expr, rib_expr_};
    use combine::EasyParser;

    #[test]
    fn test_select_index() {
        let input = "foo[0]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::SelectIndex(Box::new(Expr::Identifier("foo".to_string())), 0),
                ""
            ))
        );
    }

    #[test]
    fn test_recursive_select_index() {
        let input = "foo[0][1]";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::SelectIndex(
                    Box::new(Expr::SelectIndex(
                        Box::new(Expr::Identifier("foo".to_string())),
                        0
                    )),
                    1
                ),
                ""
            ))
        );
    }
}
