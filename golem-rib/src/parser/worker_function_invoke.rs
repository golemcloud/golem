use crate::call_type::CallType;
use crate::parser::call::call;
use crate::parser::identifier::identifier;
use crate::parser::RibParseError;
use crate::Expr;
use combine::parser::char::{char, spaces};
use combine::{attempt, between, optional, sep_by, ParseError, Parser};
use poem_openapi::__private::poem::EndpointExt;

pub fn worker_function_invoke<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    (identifier().skip(spaces()), char('.'), attempt(call()))
        .and_then(|(worker_variable, _, call)| match call {
            Expr::Call(call_type, generic_type_parameter, args, _) => match call_type {
                CallType::Function(name) => {
                    let function_name = name.to_string();
                    Ok(Expr::invoke_worker_function(
                        worker_variable,
                        function_name,
                        generic_type_parameter,
                        args,
                    ))
                }
                _ => Err(RibParseError::Message("Invalid function call".to_string())),
            },
            _ => Err(RibParseError::Message("Invalid function call".to_string())),
        })
        .message("Invalid function call")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generic_type_parameter::GenericTypeParameter;
    use test_r::test;

    #[test]
    fn test_worker_function_invoke_1() {
        let expr = Expr::from_text("worker.function-name()").unwrap();
        let worker_variable = Expr::identifier("worker", None);
        let function_name = "function-name".to_string();

        assert_eq!(
            expr,
            Expr::invoke_worker_function(worker_variable, function_name, None, vec![])
        );
    }

    #[test]
    fn test_worker_function_invoke_2() {
        let expr = Expr::from_text("worker.function-name[foo]()").unwrap();
        let worker_variable = Expr::identifier("worker", None);
        let function_name = "function-name".to_string();
        let type_parameter = GenericTypeParameter {
            value: "foo".to_string(),
        };

        assert_eq!(
            expr,
            Expr::invoke_worker_function(
                worker_variable,
                function_name,
                Some(type_parameter),
                vec![]
            )
        );
    }

    #[test]
    fn test_worker_function_invoke_3() {
        let expr = Expr::from_text(r#"worker.function-name[foo](foo, bar)"#).unwrap();
        let worker_variable = Expr::identifier("worker", None);
        let type_parameter = GenericTypeParameter {
            value: "foo".to_string(),
        };
        let function_name = "function-name".to_string();

        assert_eq!(
            expr,
            Expr::invoke_worker_function(
                worker_variable,
                function_name,
                Some(type_parameter),
                vec![Expr::identifier("foo", None), Expr::identifier("bar", None)]
            )
        );
    }

    #[test]
    fn test_worker_function_invoke_4() {
        let expr = Expr::from_text(r#"worker.function-name(foo, bar)"#).unwrap();
        let worker_variable = Expr::identifier("worker", None);
        let function_name = "function-name".to_string();

        assert_eq!(
            expr,
            Expr::invoke_worker_function(
                worker_variable,
                function_name,
                None,
                vec![Expr::identifier("foo", None), Expr::identifier("bar", None)]
            )
        );
    }
}
