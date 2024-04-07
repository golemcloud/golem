use crate::expression;
use crate::expression::Expr;
use crate::parser::expr_parser::{parse_with_context, Context};
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// Assuming the tokenizer already consumed `let` token
pub(crate) fn create_let_statement(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    dbg!(tokenizer.peek_next_non_empty_token());
    let captured_string = tokenizer.capture_string_until_and_skip_end(
        &Token::LetEqual, // Wave does this
    );

    dbg!(captured_string.clone());

    if let Some(let_variable_str) = captured_string {
        let expr = parse_with_context(let_variable_str.as_str(), Context::Code)?;
        match expr {
            Expr::Variable(variable_name) => {
                dbg!("here it goes {}", variable_name.clone());
                dbg!(tokenizer.peek_next_non_empty_token());
                let captured_string =
                    tokenizer.capture_string_until_and_skip_end(&Token::SemiColon);

                match captured_string {
                    Some(captured_string) => {
                        let expr = parse_with_context(captured_string.as_str(), Context::Code)?;
                        Ok(Expr::Let(variable_name, Box::new(expr)))
                    }
                    None => Err(ParseError::Message(
                        "Expecting a value after let variable".to_string(),
                    )),
                }
            }
            expr => Err(ParseError::Message(format!(
                "Expecting a variable name after let. But found {}",
                expression::to_string(&expr).unwrap()
            ))),
        }
    } else {
        Err(ParseError::Message(
            "Expecting a variable name after let".to_string(),
        ))
    }
}
