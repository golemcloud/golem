use crate::expression::Expr;
use crate::parser::expr::util;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

// Assuming the tokenizer already consumed `{` token, indicating the start of flags
pub(crate) fn create_flags(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    fn go(tokenizer: &mut Tokenizer, flags: &mut Vec<String>) -> Result<(), ParseError> {
        // We already skipped the first curly brace
        // We expect either a flag or curly brace, else fail
        match tokenizer.next_non_empty_token() {
            Some(Token::RCurly) => Ok(()), // Nothing more to do
            Some(Token::MultiChar(MultiCharTokens::Other(next_str))) => {
                flags.push(next_str);
                // If comma exists again, go again!
                match tokenizer.next_non_empty_token() {
                    Some(Token::Comma) => go(tokenizer, flags),
                    // Otherwise it has to be curly brace, and this consumes all flags
                    Some(Token::RCurly) => Ok(()),
                    _ => Err(ParseError::Message(
                        "Expecting a comma or closing curly brace".to_string(),
                    )),
                }
            }
            _ => Err(ParseError::Message(
                "Expecting a flag or closing curly brace".to_string(),
            )),
        }
    }
    let mut flags = vec![];
    go(tokenizer, &mut flags)?;
    Ok(Expr::Flags(flags))
}

// Assuming the tokenizer already consumed `{` token, indicating the start of flags
pub(crate) fn is_flags(tokenizer: &mut Tokenizer) -> bool {
    !empty_record(tokenizer) && !util::is_next_token_complex_type(tokenizer) && {
        let colon_index = tokenizer.index_of_end_token(&Token::Colon);
        let comma_index = tokenizer.index_of_end_token(&Token::Comma);
        match (comma_index, colon_index) {
            (Some(comma_index), Some(colon_index)) => comma_index < colon_index, // Comma exists before colon
            (None, Some(_)) => false, // Colon exists but no commas, meaning it can be record
            (Some(_), None) => true,  // Comma exists but no colons, meaning its not a record
            (None, None) => true, // No commas, no colons, but just strings between indicate flags
        }
    }
}

// Assuming the tokenizer already consumed `{` token, indicating the start of flags
fn empty_record(tokenizer: &mut Tokenizer) -> bool {
    tokenizer.peek_next_non_empty_token_is(&Token::RCurly)
}
