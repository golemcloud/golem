use combine::{between, optional, sep_by, ParseError, Parser};
use combine::parser::char::{char, spaces};
use crate::Expr;
use crate::parser::generic_type_parameter::generic_type_parameter;
use crate::parser::identifier::{identifier, identifier_text};
use crate::parser::rib_expr::rib_expr;
use crate::parser::RibParseError;

pub fn worker_function_invoke<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    (
        identifier().skip(spaces()),
        char('.'),
        identifier_text().skip(spaces()),
        // Specify the type parameter to disambiguate function calls
        optional(between(
            char('[').skip(spaces()),
            char(']').skip(spaces()),
            generic_type_parameter().skip(spaces()),
        )),
        between(
            char('(').skip(spaces()),
            char(')').skip(spaces()),
            sep_by(rib_expr().skip(spaces()), char(',').skip(spaces())),
        ),
    )
        .map(|(worker_variable, _, function_name, type_parameter, args)| {
            Expr::invoke_worker_function(worker_variable, function_name, type_parameter, args)
        })
        .message("Invalid function call")
}

#[cfg(test)]
mod tests {
    use test_r::test;
    use crate::generic_type_parameter::GenericTypeParameter;
    use super::*;

    #[test]
    fn test_worker_function_invoke_1(){
        let expr = Expr::from_text("worker.function-name()").unwrap();
        let worker_variable = Expr::identifier("worker", None);
        let function_name = "function-name".to_string();

        assert_eq!(expr, Expr::invoke_worker_function(worker_variable, function_name, None, vec![]));
    }

    #[test]
    fn test_worker_function_invoke_2(){
        let expr = Expr::from_text("worker.function-name[foo]()").unwrap();
        let worker_variable = Expr::identifier("worker", None);
        let function_name = "function-name".to_string();
        let type_parameter = GenericTypeParameter {
            value: "foo".to_string()
        };

        assert_eq!(expr, Expr::invoke_worker_function(worker_variable, function_name, Some(type_parameter), vec![]));
    }
}

