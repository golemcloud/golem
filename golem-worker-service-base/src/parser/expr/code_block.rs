use crate::expression::Expr;
use crate::parser::expr_parser::parse_code;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// Assuming ${ token is already consumed
pub fn create_code_block(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let code_block = tokenizer.capture_string_until(&Token::RCurly);

    match code_block {
        Some(expr_under_evaluation) => {
            let code_block_expr = parse_code(expr_under_evaluation.as_str())?;

            Ok(code_block_expr)
        }

        None => Err(ParseError::Message(
            "Expecting a valid expression after code block".to_string(),
        )),
    }
}
