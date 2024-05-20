use crate::expression::Expr;
use crate::parser::expr::util;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

// Assuming the tokenizer already consumed `{` token, indicating the start of flags
pub(crate) fn create_flags(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let mut flags = vec![];
    consume_flags(tokenizer, &mut flags)?;
    Ok(Expr::Flags(flags))
}

fn consume_flags(tokenizer: &mut Tokenizer, flags: &mut Vec<String>) -> Result<(), ParseError> {
    while let Some(token) = tokenizer.next_non_empty_token() {
        match token {
            Token::RCurly => return Ok(()),
            Token::MultiChar(MultiCharTokens::StringLiteral(next_str)) => {
                flags.push(next_str);
                if tokenizer.peek_next_non_empty_token_is(&Token::Comma) {
                    tokenizer.skip_next_non_empty_token(); // Consume comma
                }
            }
            _ => {
                return Err(ParseError::Message(
                    "Expecting a flag or closing curly brace".to_string(),
                ))
            }
        }
    }
    Err(ParseError::Message("Unexpected end of input".to_string()))
}

pub(crate) fn is_flags(tokenizer: &mut Tokenizer) -> bool {
    !empty_record(tokenizer) && !util::is_next_token_complex_type(tokenizer) && {
        let colon_index = tokenizer.index_of_end_token(&Token::Colon);
        let comma_index = tokenizer.index_of_end_token(&Token::Comma);
        match (comma_index, colon_index) {
            (Some(comma_index), Some(colon_index)) => comma_index < colon_index,
            (None, Some(_)) => false,
            (Some(_), None) => true,
            (None, None) => true,
        }
    }
}

fn empty_record(tokenizer: &mut Tokenizer) -> bool {
    tokenizer.peek_next_non_empty_token_is(&Token::RCurly)
}
