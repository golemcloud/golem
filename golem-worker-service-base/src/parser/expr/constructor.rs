use crate::expression::Expr;
use crate::parser::{expr_parser::parse_code, ParseError};
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// To parse expressions such as some(x), foo(bar)
pub(crate) fn create_constructor(
    tokenizer: &mut Tokenizer,
    constructor_name: &str,
) -> Result<Expr, ParseError> {
    if tokenizer.peek_next_non_empty_token_is(&Token::LParen) {
        tokenizer.skip_next_non_empty_token(); // Skip LParen
        match tokenizer.capture_string_until_and_skip_end(&Token::RParen) {
            Some(value) => {
                let expr = parse_code(value)?;
                if constructor_name == "err" {
                    Ok(Expr::Result(Err(Box::new(expr))))
                } else if constructor_name == "ok" {
                    Ok(Expr::Result(Ok(Box::new(expr))))
                } else if constructor_name == "some" {
                    Ok(Expr::Option(Some(Box::new(expr))))
                } else {
                    Err(ParseError::Message(format!(
                        "Unknown constructor {}",
                        constructor_name
                    )))
                }
            }
            None => Err(ParseError::Message(format!(
                "Empty value inside the constructor {}",
                constructor_name
            ))),
        }
    } else if constructor_name == "none" {
        Ok(Expr::Option(None))
    } else {
        Err(format!("Unknown constructor. {}", constructor_name).into())
    }
}
