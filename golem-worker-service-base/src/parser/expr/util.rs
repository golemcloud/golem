use crate::tokeniser::tokenizer::{Token, Tokenizer};

pub(crate) fn get_closing_token(opening_token: &Token) -> Option<Token> {
    match opening_token {
        Token::LCurly => Some(Token::RCurly),
        Token::LSquare => Some(Token::RSquare),
        Token::LParen => Some(Token::RParen),
        _ => None,
    }
}

pub(crate) fn is_next_token_complex_type(tokenizer: &mut Tokenizer) -> bool {
    let next_token = tokenizer.peek_next_non_empty_token();

    matches!(
        next_token,
        Some(Token::LSquare) | Some(Token::LParen) | Some(Token::LCurly)
    )
}
