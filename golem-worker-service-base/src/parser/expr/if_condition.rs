use crate::expression::Expr;
use crate::parser::expr_parser::parse_code;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

pub(crate) fn create_if_condition(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    // Parse the predicate expression
    let predicate_str = tokenizer
        .capture_string_until_and_skip_end(&Token::then())
        .ok_or_else(|| ParseError::Message("Expecting a valid expression after if".to_string()))?;

    let predicate_expr = parse_code(predicate_str.as_str())?;

    // Parse the 'then' branch expression
    let then_str = tokenizer
        .capture_string_until_and_skip_end(&Token::else_token())
        .ok_or_else(|| {
            ParseError::Message("Expecting a valid expression after then".to_string())
        })?;

    let then_expr = parse_code(then_str.as_str())?;

    // Parse the 'else' branch expression
    let else_condition = tokenizer
        .capture_string_until_and_skip_end(&Token::SemiColon)
        .or_else(|| Some(tokenizer.consume_rest().to_string()));

    let else_expr = match else_condition {
        Some(else_condition) => parse_code(else_condition.as_str())?,
        None => {
            return Err(ParseError::Message(
                "Expecting a valid expression after then".to_string(),
            ))
        }
    };

    // Return the conditional expression
    Ok(Expr::Cond(
        Box::new(predicate_expr),
        Box::new(then_expr),
        Box::new(else_expr),
    ))
}
