use crate::expression::Expr;
use crate::parser::expr_parser::{parse_text};
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// Assuming the first `quote` is consumed
pub(crate) fn create_expr_between_quotes(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    // We assume the first Quote is already consumed
    let non_code_string = tokenizer.capture_string_until_and_skip_end(&Token::Quote);

    match non_code_string {
        Some(string) => {
            parse_text(string.as_str())
        }
        None => Ok(Expr::Literal("".to_string())),
    }
}