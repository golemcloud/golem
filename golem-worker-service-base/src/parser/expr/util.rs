use crate::expression::{Expr, InnerNumber};
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

pub(crate) fn get_primitive_expr(primitive: &str) -> Expr {
    if let Ok(u64) = primitive.parse::<u64>() {
        Expr::Number(InnerNumber::UnsignedInteger(u64))
    } else if let Ok(i64_value) = primitive.parse::<i64>() {
        Expr::Number(InnerNumber::Integer(i64_value))
    } else if let Ok(f64_value) = primitive.parse::<f64>() {
        Expr::Number(InnerNumber::Float(f64_value))
    } else if let Ok(boolean) = primitive.parse::<bool>() {
        Expr::Boolean(boolean)
    } else {
        Expr::Variable(primitive.to_string())
    }
}
